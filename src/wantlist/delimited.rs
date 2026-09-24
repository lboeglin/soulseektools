//! Delimited exports: Exportify, TuneMyMusic, a hand-written file.
//!
//! Column names vary between exporters, so the reader accepts the common
//! spellings rather than requiring one exact header.

use std::path::Path;

use super::{parse_duration, ALBUM_COLUMNS, ARTIST_COLUMNS, DURATION_COLUMNS, TITLE_COLUMNS};
use crate::errors::{Error, Result};
use crate::models::Track;

pub(super) fn read(path: &Path) -> Result<Vec<Track>> {
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
