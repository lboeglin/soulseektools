//! M3U and M3U8 playlists, the inverse of what [`crate::playlist`] writes.
//!
//! A playlist names files, not songs, so each entry is described by the best
//! evidence at hand: the file's own tags when it is on this machine, else the
//! `#EXTINF` display text a player would show, else the filename. Only the last
//! is a guess. Every track carries the same `source_row` keys, so `--missing`
//! writes one header whichever of the three described it.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::split_query;
use crate::errors::{Error, Result};
use crate::library::tags::{read_duration, read_tags};
use crate::models::Track;

/// A track number in front of a filename, as in `01 - Roki` or `01. Roki`.
static TRACK_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,3}(?:\.|\s+-)\s+").unwrap());

/// What an `#EXTINF` line says about the entry after it.
struct Extinf {
    seconds: Option<f64>,
    display: String,
}

pub(super) fn read(path: &Path) -> Result<Vec<Track>> {
    let bytes = std::fs::read(path)
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;
    // Latin-1 is common in a `.m3u` from an older player; a mangled character
    // costs one track's match, refusing the file costs all of them.
    let text = String::from_utf8_lossy(&bytes);
    let base = path.parent().unwrap_or_else(|| Path::new(""));

    let mut pending = None;
    let mut tracks = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            pending = Some(extinf(info));
        } else if !line.is_empty() && !line.starts_with('#') {
            tracks.extend(entry(base, line, pending.take()));
        }
    }
    Ok(tracks)
}

/// `#EXTINF:<seconds> [attributes],<display>`; `-1` means unknown.
fn extinf(info: &str) -> Extinf {
    let (length, display) = info.split_once(',').unwrap_or((info, ""));
    Extinf {
        seconds: length
            .split_whitespace()
            .next()
            .and_then(|seconds| seconds.parse::<f64>().ok())
            .filter(|seconds| *seconds > 0.0),
        display: display.trim().to_owned(),
    }
}

fn entry(base: &Path, location: &str, info: Option<Extinf>) -> Option<Track> {
    let file = base.join(location);
    let seconds = info.as_ref().and_then(|info| info.seconds);

    let (mut track, number) = if let Some(tags) = read_tags(&file)
        .ok()
        .filter(|tags| !tags.artist.is_empty() || !tags.title.is_empty())
    {
        let number = tags.track_number.map(|number| number.to_string());
        let track = Track {
            artist: tags.artist,
            title: tags.title,
            album: tags.album,
            duration_seconds: read_duration(&file).ok().flatten().or(seconds),
            ..Track::default()
        };
        (track, number)
    } else if let Some(info) = info.filter(|info| !info.display.is_empty()) {
        let (artist, title) = split_query(&info.display);
        let track = Track {
            duration_seconds: seconds,
            ..Track::new(&artist, &title)
        };
        (track, None)
    } else {
        let (artist, title) = split_query(&TRACK_NUMBER.replace(&stem(location), ""));
        let track = Track {
            title_guessed: true,
            ..Track::new(&artist, &title)
        };
        (track, None)
    };
    if track.artist.is_empty() && track.title.is_empty() {
        return None;
    }

    track.source_row = vec![
        ("path".to_owned(), location.to_owned()),
        ("artist".to_owned(), track.artist.clone()),
        ("title".to_owned(), track.title.clone()),
        ("album".to_owned(), track.album.clone()),
        ("track number".to_owned(), number.unwrap_or_default()),
    ];
    Some(track)
}

/// The filename without its extension, whichever slash the playlist was
/// written with: one from Windows read elsewhere still names its files.
fn stem(location: &str) -> String {
    let name = location.rsplit(['/', '\\']).next().unwrap_or(location);
    name.rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_owned()
}
