//! Delimited exports (Exportify, TuneMyMusic, Last.fm, YouTube Takeout, hand
//! written), accepting the common spellings of each column.
//!
//! A headerless `lastfm-to-csv` log is recognised by the date in its fourth
//! column. A list with titles but no artist column is split like a `stk get`
//! query, as a guess.

use std::fs::File;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{
    parse_duration, split_query, ALBUM_COLUMNS, ARTIST_COLUMNS, DURATION_COLUMNS, TITLE_COLUMNS,
};
use crate::errors::{Error, Result};
use crate::models::Track;

/// The columns `lastfm-to-csv` writes, in its order, without a header.
const SCROBBLE_COLUMNS: [&str; 4] = ["artist", "album", "title", "date"];

/// `lastfm-to-csv`'s timestamp, as in `31 Jan 2021 12:34`.
static SCROBBLE_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,2} [A-Za-z]{3} \d{4},? \d{1,2}:\d{2}$").unwrap());

pub(super) fn read(path: &Path) -> Result<Vec<Track>> {
    let first = open(path, true)?
        .headers()
        .map_err(|error| failed(path, &error))?
        .clone();
    if first.is_empty() {
        return Err(Error::Config(format!("{}: file is empty", path.display())));
    }

    let (headers, has_headers): (Vec<String>, bool) = if is_scrobble_row(&first) {
        (SCROBBLE_COLUMNS.map(str::to_owned).to_vec(), false)
    } else {
        (first.iter().map(str::to_owned).collect(), true)
    };

    let lowered: Vec<String> = headers
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let position = |names: &[&str]| {
        names
            .iter()
            .find_map(|wanted| lowered.iter().position(|name| name == wanted))
    };

    let Some(title_at) = position(TITLE_COLUMNS) else {
        return Err(Error::Config(format!(
            "{}: need a title column, found {}",
            path.display(),
            lowered.join(", ")
        )));
    };
    let artist_at = position(ARTIST_COLUMNS);
    let album_at = position(ALBUM_COLUMNS);
    let duration_at = position(DURATION_COLUMNS);

    let mut tracks = Vec::new();
    for record in open(path, has_headers)?.records() {
        let record = record.map_err(|error| failed(path, &error))?;
        let cell = |at: Option<usize>| {
            at.and_then(|at| record.get(at))
                .unwrap_or("")
                .trim()
                .to_owned()
        };

        let (artist, title, title_guessed) = if artist_at.is_some() {
            (cell(artist_at), cell(Some(title_at)), false)
        } else {
            let (artist, title) = split_query(&cell(Some(title_at)));
            (artist, title, true)
        };
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
            title_guessed,
            source_row: headers
                .iter()
                .zip(record.iter())
                .filter(|(key, _)| !key.is_empty())
                .map(|(key, value)| (key.clone(), value.to_owned()))
                .collect(),
        });
    }
    Ok(tracks)
}

fn open(path: &Path, has_headers: bool) -> Result<csv::Reader<File>> {
    csv::ReaderBuilder::new()
        .has_headers(has_headers)
        .from_path(path)
        .map_err(|error| failed(path, &error))
}

fn failed(path: &Path, error: &csv::Error) -> Error {
    Error::Config(format!("{}: {error}", path.display()))
}

fn is_scrobble_row(row: &csv::StringRecord) -> bool {
    row.len() == SCROBBLE_COLUMNS.len()
        && row
            .get(SCROBBLE_COLUMNS.len() - 1)
            .is_some_and(|date| SCROBBLE_DATE.is_match(date.trim()))
}
