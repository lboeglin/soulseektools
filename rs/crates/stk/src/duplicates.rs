//! Finding the same track twice in a library.
//!
//! Two independent passes, because they fail in different directions:
//!
//! - **by title**, which catches the same song filed under two spellings of the
//!   artist (`Inabakumori` and `稲葉曇`) but is blind to untagged files with
//!   meaningless names;
//! - **by audio**, which catches those but needs decoding, so a cheap duration
//!   prefilter narrows the field before any fingerprint is computed.
//!
//! The title pass compares only entries sharing an n-gram (see
//! [`crate::matching::blocking_keys`]) rather than every pair against every
//! other, which is what makes it usable on a library of any size.

use std::collections::{HashMap, HashSet};

use crate::audio::fingerprint::{self, Fingerprint};
use crate::library::index::LibraryIndex;
use crate::matching::{blocking_keys, candidate_pairs, score_track, MatchThresholds};
use crate::models::LibraryEntry;

pub const DEFAULT_AUDIO_THRESHOLD: f64 = 0.9;
pub const DEFAULT_DURATION_TOLERANCE_SECONDS: f64 = 2.0;

/// Two files that look like the same recording, and the evidence for it.
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicatePair {
    pub left: LibraryEntry,
    pub right: LibraryEntry,
    pub score: f64,
    pub evidence: String,
}

impl DuplicatePair {
    /// The copy worth keeping, all else being equal.
    #[must_use]
    pub fn larger(&self) -> &LibraryEntry {
        if self.left.size >= self.right.size {
            &self.left
        } else {
            &self.right
        }
    }

    #[must_use]
    pub fn smaller(&self) -> &LibraryEntry {
        if self.left.size >= self.right.size {
            &self.right
        } else {
            &self.left
        }
    }
}

/// Pairs whose artist and title match, across the whole library.
#[must_use]
pub fn find_by_title(entries: &[LibraryEntry], thresholds: &MatchThresholds) -> Vec<DuplicatePair> {
    let keyed: Vec<(usize, HashSet<String>)> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (index, blocking_keys(&[&entry.artist, &entry.title])))
        .collect();

    let mut pairs: Vec<DuplicatePair> = candidate_pairs(&keyed)
        .into_iter()
        .filter_map(|(left_index, right_index)| {
            let (left, right) = (&entries[left_index], &entries[right_index]);
            if left.path == right.path {
                return None;
            }
            let score = score_track(
                &left.artist,
                &left.title,
                &right.artist,
                &right.title,
                thresholds,
            )?;
            Some(DuplicatePair {
                left: left.clone(),
                right: right.clone(),
                score: score.combined,
                evidence: format!("artist {:.2}, title {:.2}", score.artist, score.title),
            })
        })
        .collect();

    sort_by_score(&mut pairs);
    pairs
}

/// How a fingerprint is obtained, so the cache can be supplied by the caller.
///
/// The index owns the cache table; keeping the lookup behind a closure means
/// this module never has to know about SQLite.
pub type FingerprintSource<'a> = dyn FnMut(&LibraryEntry) -> Option<Fingerprint> + 'a;

/// Pairs whose audio matches, whatever they are named or tagged.
///
/// Duration is only a prefilter here. Treating a duration and size match as
/// proof, which the original script did, produces coincidental hits between
/// unrelated tracks of similar length; the fingerprint is what decides.
pub fn find_by_audio(
    entries: &[LibraryEntry],
    threshold: f64,
    duration_tolerance_seconds: f64,
    fingerprint_of: &mut FingerprintSource<'_>,
) -> Vec<DuplicatePair> {
    let comparable: Vec<&LibraryEntry> = entries
        .iter()
        .filter(|entry| entry.duration_seconds.is_some_and(|seconds| seconds != 0.0))
        .collect();
    let candidates = pairs_within_duration(&comparable, duration_tolerance_seconds);
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut needed: Vec<usize> = candidates
        .iter()
        .flat_map(|(left, right)| [*left, *right])
        .collect();
    needed.sort_unstable();
    needed.dedup();

    let mut computed: HashMap<usize, Fingerprint> = HashMap::new();
    for position in needed {
        if let Some(value) = fingerprint_of(comparable[position]) {
            if !value.is_empty() {
                computed.insert(position, value);
            }
        }
    }

    let mut pairs: Vec<DuplicatePair> = candidates
        .into_iter()
        .filter_map(|(left_index, right_index)| {
            let left_print = computed.get(&left_index)?;
            let right_print = computed.get(&right_index)?;
            let score = fingerprint::similarity(left_print, right_print).ok()?;
            if score < threshold {
                return None;
            }
            Some(DuplicatePair {
                left: comparable[left_index].clone(),
                right: comparable[right_index].clone(),
                score,
                evidence: format!("{} similarity {score:.2}", left_print.kind()),
            })
        })
        .collect();

    sort_by_score(&mut pairs);
    pairs
}

/// Best score first, as the Python's `sorted(key=lambda pair: -pair.score)`.
fn sort_by_score(pairs: &mut [DuplicatePair]) {
    pairs.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn pairs_within_duration(entries: &[&LibraryEntry], tolerance_seconds: f64) -> Vec<(usize, usize)> {
    let duration = |index: usize| entries[index].duration_seconds.unwrap_or(0.0);
    let mut ordered: Vec<usize> = (0..entries.len()).collect();
    ordered.sort_by(|left, right| {
        duration(*left)
            .partial_cmp(&duration(*right))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut pairs = Vec::new();
    for (offset, left) in ordered.iter().enumerate() {
        for right in &ordered[offset + 1..] {
            // The list is sorted, so the first entry out of tolerance ends the run.
            if duration(*right) - duration(*left) > tolerance_seconds {
                break;
            }
            pairs.push(if left < right {
                (*left, *right)
            } else {
                (*right, *left)
            });
        }
    }
    pairs
}

/// A [`FingerprintSource`] that consults the index before computing, and stores
/// what it computed.
///
/// The Python passes the index itself into `find_by_audio`; keeping the lookup
/// behind a closure means `duplicates` never has to know about SQLite, and the
/// tests can substitute a table without patching a module.
pub fn cached_fingerprints(
    index: &mut LibraryIndex,
) -> impl FnMut(&LibraryEntry) -> Option<Fingerprint> + '_ {
    move |entry: &LibraryEntry| {
        if let Ok(Some(cached)) = index.fingerprint(&entry.path, entry.mtime) {
            return Some(cached);
        }
        let value = fingerprint::fingerprint(&entry.path, None).ok()?;
        let _ = index.store_fingerprint(&entry.path, entry.mtime, &value);
        Some(value)
    }
}
