//! Reading a streaming-service export and working out what is missing.
//!
//! The ownership test is deliberately asymmetric. A track is dropped from the
//! wantlist only when a library file is convincingly the same song; anything
//! uncertain stays in the list, because downloading a duplicate costs a few
//! megabytes and silently skipping a track loses it.
//!
//! Column names vary between exporters, so the reader accepts the common
//! spellings rather than requiring one exact header.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::errors::{Error, Result};
use crate::matching::{blocking_keys, score_track, MatchThresholds, NameForms};
use crate::models::{LibraryEntry, Track};

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

/// Read a streaming-service CSV export into tracks.
pub fn read_export(path: &Path) -> Result<Vec<Track>> {
    let mut reader = csv::Reader::from_path(path)
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;

    let headers = reader
        .headers()
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?
        .clone();
    if headers.is_empty() {
        return Err(Error::Config(format!("{}: file is empty", path.display())));
    }

    let lowered: Vec<String> = headers
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let position = |names: &[&str]| {
        names
            .iter()
            .find_map(|wanted| lowered.iter().position(|name| name == wanted))
    };

    let (Some(artist_at), Some(title_at)) = (position(ARTIST_COLUMNS), position(TITLE_COLUMNS))
    else {
        return Err(Error::Config(format!(
            "{}: need an artist and a title column, found {}",
            path.display(),
            lowered.join(", ")
        )));
    };
    let album_at = position(ALBUM_COLUMNS);
    let duration_at = position(DURATION_COLUMNS);

    let mut tracks = Vec::new();
    for record in reader.records() {
        let record =
            record.map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;
        let cell = |at: Option<usize>| {
            at.and_then(|at| record.get(at))
                .unwrap_or("")
                .trim()
                .to_owned()
        };

        let artist = cell(Some(artist_at));
        let title = cell(Some(title_at));
        if artist.is_empty() && title.is_empty() {
            continue;
        }

        tracks.push(Track {
            artist,
            title,
            album: cell(album_at),
            duration_seconds: duration_at
                .and_then(|at| record.get(at))
                .and_then(parse_duration),
            title_guessed: false,
            source_row: headers
                .iter()
                .zip(record.iter())
                .filter(|(key, _)| !key.is_empty())
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        });
    }
    Ok(tracks)
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

/// Read a duration, accepting both milliseconds and `m:ss`.
fn parse_duration(raw: &str) -> Option<f64> {
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
