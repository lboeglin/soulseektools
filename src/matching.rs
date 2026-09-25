//! Fuzzy name matching. A strong title may carry a weak artist, never the reverse.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use rapidfuzz::fuzz;

use crate::text::normalize::normalize;
use crate::text::romaji::to_romaji;

/// Truncated or extended filenames are the same track more often than their ratio suggests.
pub const CONTAINMENT_SCORE: f64 = 0.85;

const BLOCKING_GRAM_SIZE: usize = 4;

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

    #[must_use]
    pub fn is_present(&self) -> bool {
        !self.normalized.is_empty()
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchScore {
    pub artist: f64,
    pub title: f64,
    pub combined: f64,
}

/// For `token_sort_ratio`, which the rapidfuzz crate doesn't ship.
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

/// Romaji against romaji too: two kanji spellings of one reading only match that way.
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

/// `None` rather than a low score, so an uncertain pair is never taken as a match.
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

/// Names similar enough to match share a gram, so comparing only those loses no matches.
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
    // Sorted for reproducibility; callers treat the result as a set.
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
