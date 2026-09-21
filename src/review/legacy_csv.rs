//! Importing the review CSV the Nicotine+ plugin used.
//!
//! A migration path, so a session interrupted mid-batch under the old plugin can
//! be picked up here rather than re-searched from scratch. The CSV held three
//! candidates per row in columns named `cand1_user`, `cand2_path` and so on,
//! plus an `approve` column holding `1`, `2` or `3`.

use std::collections::HashMap;
use std::path::Path;

use crate::errors::{Error, Result};
use crate::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use crate::review::store::ReviewStore;

const CANDIDATES_PER_ROW: usize = 3;

/// What an import changed: tracks added, and approvals carried over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportResult {
    pub added: usize,
    pub approvals: usize,
}

/// Load a legacy review CSV into the store.
///
/// Tracks already in the store are left untouched.
pub fn import_csv(path: &Path, store: &mut ReviewStore) -> Result<ImportResult> {
    let mut reader = csv::Reader::from_path(path)
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;
    let headers = reader
        .headers()
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?
        .clone();

    let rows: Vec<HashMap<String, String>> = reader
        .records()
        .filter_map(std::result::Result::ok)
        .map(|record| {
            headers
                .iter()
                .zip(record.iter())
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect()
        })
        .collect();

    let tracks: Vec<Track> = rows
        .iter()
        .filter(|row| !field(row, "artist").is_empty() || !field(row, "title").is_empty())
        .map(|row| Track::new(&field(row, "artist"), &field(row, "title")))
        .collect();
    let added = store.add(&tracks)?;

    let by_query: HashMap<String, i64> = store
        .tracks(&[], None)?
        .into_iter()
        .map(|entry| (entry.track.query(), entry.id))
        .collect();

    let mut approvals = 0;
    for row in &rows {
        let query = format!("{} - {}", field(row, "artist"), field(row, "title"));
        let Some(track_id) = by_query.get(&query).copied() else {
            continue;
        };

        let candidates = candidates(row);
        if candidates.is_empty() {
            store.mark(
                track_id,
                TrackStatus::NoCandidates,
                "no candidates in imported CSV",
            )?;
            continue;
        }

        store.set_candidates(track_id, &candidates)?;
        let approve = field(row, "approve");
        if let Ok(rank) = approve.parse::<i64>() {
            if rank >= 1 && usize::try_from(rank).unwrap_or(usize::MAX) <= candidates.len() {
                store.approve(track_id, rank)?;
                approvals += 1;
            }
        }
    }

    Ok(ImportResult { added, approvals })
}

fn field(row: &HashMap<String, String>, key: &str) -> String {
    row.get(key).map_or("", |value| value.trim()).to_owned()
}

fn candidates(row: &HashMap<String, String>) -> Vec<ScoredCandidate> {
    (1..=CANDIDATES_PER_ROW)
        .filter_map(|position| {
            let prefix = format!("cand{position}_");
            let username = field(row, &format!("{prefix}user"));
            let remote_path = field(row, &format!("{prefix}path"));
            if username.is_empty() || remote_path.is_empty() {
                return None;
            }

            Some(ScoredCandidate {
                candidate: Candidate {
                    username,
                    remote_path,
                    size: number(row, &format!("{prefix}size"))
                        .unwrap_or(0)
                        .unsigned_abs(),
                    bitrate: number(row, &format!("{prefix}bitrate")).and_then(as_u32),
                    sample_rate: number(row, &format!("{prefix}samplerate")).and_then(as_u32),
                    bit_depth: number(row, &format!("{prefix}bitdepth")).and_then(as_u32),
                    is_vbr: number(row, &format!("{prefix}vbr")).is_some_and(|value| value != 0),
                    ..Candidate::default()
                },
                score: number(row, &format!("{prefix}score")).unwrap_or(0),
                title_ratio: 0.0,
                flags: vec!["imported".to_owned()],
            })
        })
        .collect()
}

/// The plugin wrote some numbers as floats, so `int(float(text))` is what the
/// Python parses them with.
fn number(row: &HashMap<String, String>, key: &str) -> Option<i64> {
    let text = field(row, key);
    if text.is_empty() {
        return None;
    }
    // The plugin wrote whole numbers as floats, so the cast is Python's
    // `int(float(text))`, truncating toward zero.
    #[allow(clippy::cast_possible_truncation)]
    text.parse::<f64>().ok().map(|value| value as i64)
}

fn as_u32(value: i64) -> Option<u32> {
    u32::try_from(value).ok()
}
