//! Reading a streaming-service export and working out what is missing.
//!
//! The ownership test is deliberately asymmetric. A track is dropped from the
//! wantlist only when a library file is convincingly the same song; anything
//! uncertain stays in the list, because downloading a duplicate costs a few
//! megabytes and silently skipping a track loses it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::errors::Result;
use crate::matching::{blocking_keys, score_track, MatchThresholds, NameForms};
use crate::models::{LibraryEntry, Track};

mod delimited;

pub const ARTIST_COLUMNS: &[&str] = &[
    "artist name",
    "artist name(s)",
    "artist",
    "artists",
    "album artist",
];
pub const TITLE_COLUMNS: &[&str] = &["track name", "title", "name", "song"];
pub const ALBUM_COLUMNS: &[&str] = &["album name", "album"];
pub const DURATION_COLUMNS: &[&str] = &[
    "duration (ms)",
    "duration_ms",
    "track duration (ms)",
    "length",
];
/// Not read into a [`Track`] field: it stays in `source_row` and is looked up
/// when a download is tagged (#11).
pub const TRACK_NUMBER_COLUMNS: &[&str] = &["track number", "track_number", "track no", "track"];

/// A wanted track matched to a file already in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedTrack {
    pub track: Track,
    pub entry: LibraryEntry,
    pub score: f64,
}

/// What an export contains that the library does not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WantlistDiff {
    pub missing: Vec<Track>,
    pub owned: Vec<OwnedTrack>,
}

impl WantlistDiff {
    #[must_use]
    pub fn total(&self) -> usize {
        self.missing.len() + self.owned.len()
    }
}

/// The en and em dashes are deliberate: a track name pasted out of a browser or
/// a music player usually carries one rather than a plain hyphen.
pub const SEPARATORS: &[&str] = &[" - ", " \u{2013} ", " \u{2014} "];

/// Read a streaming-service CSV export into tracks.
pub fn read_export(path: &Path) -> Result<Vec<Track>> {
    delimited::read(path)
}

/// Split wanted tracks into those already owned and those still missing.
#[must_use]
pub fn diff(
    tracks: &[Track],
    entries: &[LibraryEntry],
    thresholds: &MatchThresholds,
) -> WantlistDiff {
    let index = by_gram(entries);
    let mut result = WantlistDiff::default();

    for track in tracks {
        let artist = NameForms::of(&track.artist);
        let title = NameForms::of(&track.title);

        let best = candidates(&index, &artist, &title)
            .into_iter()
            .filter_map(|position| {
                let entry = &entries[position];
                let score = score_track(&artist, &title, &entry.artist, &entry.title, thresholds)?;
                Some(OwnedTrack {
                    track: track.clone(),
                    entry: entry.clone(),
                    score: score.combined,
                })
            })
            .max_by(|left, right| {
                left.score
                    .partial_cmp(&right.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        match best {
            Some(owned) => result.owned.push(owned),
            None => result.missing.push(track.clone()),
        }
    }
    result
}

fn by_gram(entries: &[LibraryEntry]) -> HashMap<String, Vec<usize>> {
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    for (position, entry) in entries.iter().enumerate() {
        for gram in blocking_keys(&[&entry.artist, &entry.title]) {
            index.entry(gram).or_default().push(position);
        }
    }
    index
}

fn candidates(
    index: &HashMap<String, Vec<usize>>,
    artist: &NameForms,
    title: &NameForms,
) -> Vec<usize> {
    let mut positions: HashSet<usize> = HashSet::new();
    for gram in blocking_keys(&[artist, title]) {
        if let Some(found) = index.get(&gram) {
            positions.extend(found);
        }
    }
    let mut ordered: Vec<usize> = positions.into_iter().collect();
    // The Python iterates a set, whose order is arbitrary but whose best score
    // wins regardless; sorting only makes ties resolve reproducibly.
    ordered.sort_unstable();
    ordered
}

/// Split `artist - title` into its parts, tolerating either being absent.
///
/// A query with no separator is treated as a title: searching for a title alone
/// works, searching for a bare artist does not.
#[must_use]
pub fn split_query(query: &str) -> (String, String) {
    for separator in SEPARATORS {
        if let Some((artist, title)) = query.split_once(separator) {
            return (artist.trim().to_owned(), title.trim().to_owned());
        }
    }
    (String::new(), query.trim().to_owned())
}

/// Read a duration, accepting both milliseconds and `m:ss`.
pub(crate) fn parse_duration(raw: &str) -> Option<f64> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }

    if text.contains(':') {
        let mut seconds = 0.0;
        for part in text.split(':') {
            seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
        }
        return Some(seconds);
    }

    let value: f64 = text.parse().ok()?;
    // Exporters write milliseconds; a track measured in seconds is never this long.
    Some(if value > 3600.0 {
        value / 1000.0
    } else {
        value
    })
}
