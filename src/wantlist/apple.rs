//! Apple Music and iTunes `Library.xml`. Podcasts, films, TV, music videos and
//! audiobooks are skipped.

use std::path::Path;

use plist::{Dictionary, Value};

use super::parse_duration;
use crate::errors::{Error, Result};
use crate::models::Track;

/// Flags Apple sets on an item that is not a song.
const NOT_MUSIC: &[&str] = &["Podcast", "Movie", "TV Show", "Music Video"];

pub(super) fn read(path: &Path) -> Result<Vec<Track>> {
    let library = Value::from_file(path)
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;
    let Some(items) = library
        .as_dictionary()
        .and_then(|library| library.get("Tracks"))
        .and_then(Value::as_dictionary)
    else {
        return Err(Error::Config(format!(
            "{}: not an Apple Music library, no Tracks in it",
            path.display()
        )));
    };

    Ok(items
        .values()
        .filter_map(Value::as_dictionary)
        .filter(|item| is_music(item))
        .filter_map(track)
        .collect())
}

fn is_music(item: &Dictionary) -> bool {
    let flagged = NOT_MUSIC
        .iter()
        .any(|flag| item.get(flag).and_then(Value::as_boolean) == Some(true));
    let audiobook = text(item, "Kind").to_lowercase().contains("audiobook");
    !flagged && !audiobook
}

fn track(item: &Dictionary) -> Option<Track> {
    let artist = [text(item, "Artist"), text(item, "Album Artist")]
        .into_iter()
        .find(|artist| !artist.is_empty())
        .unwrap_or_default();
    let title = text(item, "Name");
    if artist.is_empty() && title.is_empty() {
        return None;
    }

    Some(Track {
        artist,
        title,
        album: text(item, "Album"),
        // Milliseconds, which `parse_duration` already tells from seconds.
        duration_seconds: parse_duration(&text(item, "Total Time")),
        title_guessed: false,
        source_row: item
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), scalar(value)?)))
            .collect(),
    })
}

fn text(item: &Dictionary, key: &str) -> String {
    item.get(key)
        .and_then(scalar)
        .map(|value| value.trim().to_owned())
        .unwrap_or_default()
}

/// A field as a CSV cell would hold it; data and nested values have none.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Integer(number) => Some(number.to_string()),
        Value::Real(number) => Some(number.to_string()),
        Value::Boolean(flag) => Some(flag.to_string()),
        Value::Date(date) => Some(date.to_xml_format()),
        _ => None,
    }
}
