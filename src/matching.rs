//! Fuzzy matching between wantlist tracks, library files and search results.
//!
//! Every comparison in the project runs through [`similarity`], which scores a
//! name against another in both its normalised and its romanised form. The
//! asymmetric thresholds in [`MatchThresholds`] encode a deliberate bias: a very
//! strong title match is allowed to carry a weak artist match, because artist
//! readings are the unreliable half (see [`crate::text::romaji`]), but the
//! reverse is never accepted.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use rapidfuzz::fuzz;

use crate::text::normalize::normalize;
use crate::text::romaji::to_romaji;

/// Score awarded when one name is contained in the other.
///
/// Truncated and extended filenames ("Senbonzakura" vs "Senbonzakura -Long Ver-")
/// are the same track far more often than their raw character ratio suggests.
pub const CONTAINMENT_SCORE: f64 = 0.85;

const BLOCKING_GRAM_SIZE: usize = 4;

/// A name in each of the forms the matcher compares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct NameForms {
    pub raw: String,
    pub normalized: String,
    pub romaji: String,
}

impl NameForms {
    #[must_use]
    pub fn of(text: &str) -> Self {
        let normalized = normalize(text);
        let romaji = to_romaji(&normalized);
        Self {
            raw: text.to_owned(),
            normalized,
            romaji,
        }
    }

    /// Whether there is anything here to compare. Empty names never match.
    #[must_use]
    pub fn is_present(&self) -> bool {
        !self.normalized.is_empty()
    }
}

/// Cut-offs deciding whether two names describe the same track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchThresholds {
    pub artist: f64,
    pub artist_weak: f64,
    pub title: f64,
    pub title_strong: f64,
    pub artist_weight: f64,
}

impl Default for MatchThresholds {
    fn default() -> Self {
        Self {
            artist: 0.6,
            artist_weak: 0.35,
            title: 0.62,
            title_strong: 0.85,
            artist_weight: 0.4,
        }
    }
}

impl MatchThresholds {
    #[must_use]
    pub fn title_weight(&self) -> f64 {
        1.0 - self.artist_weight
    }
}

/// The outcome of comparing an (artist, title) pair against another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchScore {
    pub artist: f64,
    pub title: f64,
    pub combined: f64,
}

/// rapidfuzz's `token_sort_ratio`, which the Rust crate does not ship.
///
/// It is `ratio` over the whitespace-separated tokens sorted and rejoined; the
/// Python side applies no processor, and both inputs are already normalised.
fn token_sort(text: &str) -> String {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    tokens.sort_unstable();
    tokens.join(" ")
}

fn pair_ratio(left: &str, right: &str) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let plain = fuzz::ratio(left.chars(), right.chars());
    let sorted = fuzz::ratio(token_sort(left).chars(), token_sort(right).chars());
    let mut best = plain.max(sorted);

    if left.contains(right) || right.contains(left) {
        best = best.max(CONTAINMENT_SCORE);
    }
    best
}

/// Best similarity in `[0, 1]` across the normalised and romanised forms.
///
/// All four combinations are compared, including romaji against romaji: two
/// different kanji spellings that share a reading (`稲葉曇` / `いなば くもり`)
/// only line up once both sides are transliterated.
#[must_use]
pub fn similarity(left: &NameForms, right: &NameForms) -> f64 {
    if !left.is_present() || !right.is_present() {
        return 0.0;
    }
    [
        pair_ratio(&left.normalized, &right.normalized),
        pair_ratio(&left.romaji, &right.normalized),
        pair_ratio(&left.normalized, &right.romaji),
        pair_ratio(&left.romaji, &right.romaji),
    ]
    .into_iter()
    .fold(0.0, f64::max)
}

/// Score two tracks against each other, or return `None` if they differ.
///
/// Returning `None` rather than a low score keeps callers conservative by
/// construction: an uncertain pair is never silently treated as a match.
#[must_use]
pub fn score_track(
    left_artist: &NameForms,
    left_title: &NameForms,
    right_artist: &NameForms,
    right_title: &NameForms,
    thresholds: &MatchThresholds,
) -> Option<MatchScore> {
    let artist = similarity(left_artist, right_artist);
    let title = similarity(left_title, right_title);

    let strong_title_match = title >= thresholds.title_strong && artist >= thresholds.artist_weak;
    let ordinary_match = artist >= thresholds.artist && title >= thresholds.title;
    if !(strong_title_match || ordinary_match) {
        return None;
    }

    Some(MatchScore {
        artist,
        title,
        combined: artist * thresholds.artist_weight + title * thresholds.title_weight(),
    })
}

/// Character n-grams used to avoid comparing every name against every other.
///
/// Two names similar enough to pass [`MatchThresholds`] overlap by well over
/// [`BLOCKING_GRAM_SIZE`] characters, so restricting comparisons to names that
/// share at least one gram turns a quadratic scan into a near-linear one without
/// losing matches.
#[must_use]
pub fn blocking_keys(forms: &[&NameForms]) -> HashSet<String> {
    let mut grams = HashSet::new();
    for form in forms {
        for variant in [&form.normalized, &form.romaji] {
            let compact: Vec<char> = variant.chars().filter(|c| *c != ' ').collect();
            if compact.len() < BLOCKING_GRAM_SIZE {
                if !compact.is_empty() {
                    grams.insert(compact.into_iter().collect());
                }
                continue;
            }
            for window in compact.windows(BLOCKING_GRAM_SIZE) {
                grams.insert(window.iter().collect());
            }
        }
    }
    grams
}

/// Index pairs sharing at least one blocking key, each pair once.
#[must_use]
pub fn candidate_pairs<S: BuildHasher>(
    keyed: &[(usize, HashSet<String, S>)],
) -> Vec<(usize, usize)> {
    let mut by_gram: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, grams) in keyed {
        for gram in grams {
            by_gram.entry(gram.as_str()).or_default().push(*index);
        }
    }

    let mut seen = HashSet::new();
    let mut pairs = Vec::new();
    // Grams are visited in the order the keys were inserted, which a hash map
    // does not preserve; the callers all treat the result as a set, and sorting
    // keeps it reproducible for tests and reports.
    let mut grams: Vec<&Vec<usize>> = by_gram.values().collect();
    grams.sort_unstable();
    for indices in grams {
        for (position, left) in indices.iter().enumerate() {
            for right in &indices[position + 1..] {
                let pair = if left < right {
                    (*left, *right)
                } else {
                    (*right, *left)
                };
                if pair.0 != pair.1 && seen.insert(pair) {
                    pairs.push(pair);
                }
            }
        }
    }
    pairs.sort_unstable();
    pairs
}
