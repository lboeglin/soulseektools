//! Assigning a consistent genre taxonomy across a library.
//!
//! The mapping lives in a TOML file rather than in this module. It is inherently
//! per-library data -- one person's artist roster says nothing about anyone
//! else's -- and editing it should not mean editing source.
//!
//! Artists are matched on their normalised name, so a `DECO*27` entry also
//! covers a `DECO_27` folder, and an entry may name either one genre or several.

use std::collections::HashMap;
use std::path::Path;

use crate::errors::{Error, Result};
use crate::library::tags;
use crate::models::LibraryEntry;
use crate::text::normalize::normalize;

/// Artist-to-genre assignments, looked up by normalised artist name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenreMap {
    by_artist: HashMap<String, Vec<String>>,
}

impl GenreMap {
    #[must_use]
    pub fn new(by_artist: HashMap<String, Vec<String>>) -> Self {
        Self { by_artist }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_artist.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_artist.is_empty()
    }

    /// The genres assigned to `artist`, or nothing when unmapped.
    #[must_use]
    pub fn genres_for(&self, artist: &str) -> Vec<String> {
        self.by_artist
            .get(&normalize(artist))
            .cloned()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn covers(&self, artist: &str) -> bool {
        self.by_artist.contains_key(&normalize(artist))
    }
}

/// One file's genre tag before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct GenreChange {
    pub entry: LibraryEntry,
    pub current: Vec<String>,
    pub proposed: Vec<String>,
}

impl GenreChange {
    #[must_use]
    pub fn is_change(&self) -> bool {
        self.current != self.proposed
    }
}

/// Read a genre map from TOML.
///
/// The file is a flat table of `"Artist" = "Genre"` or
/// `"Artist" = ["Genre", "Other"]`, optionally nested under `[artists]`.
pub fn load_genre_map(path: &Path) -> Result<GenreMap> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))?;
    let data: toml::Value = toml::from_str(&text)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))?;

    let artists = data
        .get("artists")
        .unwrap_or(&data)
        .as_table()
        .ok_or_else(|| {
            Error::Config(format!(
                "{}: expected a table of artist names",
                path.display()
            ))
        })?
        .clone();

    let mut mapping = HashMap::new();
    for (artist, value) in artists {
        let genres = match &value {
            toml::Value::String(genre) => vec![genre.clone()],
            toml::Value::Array(items) if items.iter().all(toml::Value::is_str) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            _ => {
                return Err(Error::Config(format!(
                    "{}: '{artist}' must be a genre or a list of genres",
                    path.display()
                )));
            }
        };
        mapping.insert(normalize(&artist), genres);
    }
    Ok(GenreMap::new(mapping))
}

/// The artist a file is filed under: its folder, else the resolved name.
fn artist_of(entry: &LibraryEntry) -> &str {
    if entry.folder_artist.is_empty() {
        &entry.artist.raw
    } else {
        &entry.folder_artist
    }
}

/// What applying `genre_map` would do, without touching anything.
#[must_use]
pub fn plan_changes(entries: &[LibraryEntry], genre_map: &GenreMap) -> Vec<GenreChange> {
    entries
        .iter()
        .filter(|entry| genre_map.covers(artist_of(entry)))
        .map(|entry| GenreChange {
            current: entry.tags.genres.clone(),
            proposed: genre_map.genres_for(artist_of(entry)),
            entry: entry.clone(),
        })
        .collect()
}

/// Artists present in the library but absent from the map, with file counts.
///
/// Ordered by count descending then name, so the report is stable.
#[must_use]
pub fn unmapped_artists(entries: &[LibraryEntry], genre_map: &GenreMap) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for entry in entries {
        let artist = artist_of(entry);
        if !artist.is_empty() && !genre_map.covers(artist) {
            *counts.entry(artist).or_default() += 1;
        }
    }

    let mut ordered: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(artist, count)| (artist.to_owned(), count))
        .collect();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ordered
}

/// Write one planned genre change to disk.
pub fn apply_change(change: &GenreChange) -> Result<()> {
    tags::write_genres(&change.entry.path, &change.proposed)
        .map_err(|error| Error::Tag(format!("{}: {error}", change.entry.path.display())))
}
