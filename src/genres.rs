//! Assigning genres from a TOML map of artists, with per-album and per-track
//! overrides. Names match on their normalised form, and the most specific entry
//! wins.

use std::collections::HashMap;
use std::path::Path;

use crate::errors::{Error, Result};
use crate::library::tags;
use crate::models::LibraryEntry;
use crate::text::normalize::normalize;

/// Overrides below the artist level, keyed by normalised artist and name.
pub type Overrides = HashMap<(String, String), Vec<String>>;

/// What a file is filed under, as a genre lookup needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackKey<'a> {
    pub artist: &'a str,
    pub album: &'a str,
    pub title: &'a str,
}

impl<'a> TrackKey<'a> {
    /// How an indexed file is addressed by a map.
    ///
    /// The artist is the folder, falling back to the resolved name, so an
    /// untagged file is still addressable.
    #[must_use]
    pub fn of(entry: &'a LibraryEntry) -> Self {
        Self {
            artist: if entry.folder_artist.is_empty() {
                &entry.artist.raw
            } else {
                &entry.folder_artist
            },
            album: &entry.tags.album,
            title: &entry.title.raw,
        }
    }
}

/// Genre assignments, looked up by normalised artist, album and title.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenreMap {
    artists: HashMap<String, Vec<String>>,
    albums: Overrides,
    tracks: Overrides,
}

impl GenreMap {
    /// A map of artists alone, which is what most libraries need.
    #[must_use]
    pub fn new(artists: HashMap<String, Vec<String>>) -> Self {
        Self {
            artists,
            ..Self::default()
        }
    }

    /// Add per-album overrides, keyed by normalised artist and album.
    #[must_use]
    pub fn with_albums(mut self, albums: Overrides) -> Self {
        self.albums = albums;
        self
    }

    /// Add per-track overrides, keyed by normalised artist and title.
    #[must_use]
    pub fn with_tracks(mut self, tracks: Overrides) -> Self {
        self.tracks = tracks;
        self
    }

    /// How many artists the map names, whatever else it overrides.
    #[must_use]
    pub fn len(&self) -> usize {
        self.artists.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.artists.is_empty() && self.albums.is_empty() && self.tracks.is_empty()
    }

    /// The genres assigned to a file: track override, else album, else artist.
    #[must_use]
    pub fn genres_for(&self, key: TrackKey<'_>) -> Vec<String> {
        let artist = normalize(key.artist);
        self.tracks
            .get(&(artist.clone(), normalize(key.title)))
            .or_else(|| self.albums.get(&(artist.clone(), normalize(key.album))))
            .or_else(|| self.artists.get(&artist))
            .cloned()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn covers(&self, key: TrackKey<'_>) -> bool {
        !self.genres_for(key).is_empty()
    }

    /// Whether the map names this artist at all, overrides aside.
    #[must_use]
    pub fn covers_artist(&self, artist: &str) -> bool {
        self.artists.contains_key(&normalize(artist))
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
/// `[albums."Artist"]` and `[tracks."Artist"]` name overrides below it, keyed by
/// album title and by track title.
///
/// Nested tables, not an `"Artist/Album"` key, because names can contain `/`.
pub fn load_genre_map(path: &Path) -> Result<GenreMap> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))?;
    let data: toml::Value = toml::from_str(&text)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))?;

    let albums = read_overrides(path, "albums", data.get("albums"))?;
    let tracks = read_overrides(path, "tracks", data.get("tracks"))?;

    let artists = artist_table(path, &data, albums.is_empty() && tracks.is_empty())?;
    let mut mapping = HashMap::new();
    for (artist, value) in artists {
        mapping.insert(normalize(&artist), read_genres(path, &artist, &value)?);
    }

    Ok(GenreMap::new(mapping)
        .with_albums(albums)
        .with_tracks(tracks))
}

/// The artist table, wherever the file put it.
///
/// Without an `[artists]` section the whole file is the artist table, unless
/// an override section exists (`alone` is false). Then top-level artists are
/// refused rather than silently ignored.
fn artist_table(path: &Path, data: &toml::Value, alone: bool) -> Result<toml::Table> {
    let expected = || {
        Error::Config(format!(
            "{}: expected a table of artist names",
            path.display()
        ))
    };

    if let Some(artists) = data.get("artists") {
        return artists.as_table().cloned().ok_or_else(expected);
    }
    let whole = data.as_table().cloned().ok_or_else(expected)?;
    if alone {
        return Ok(whole);
    }

    let stray: Vec<&str> = whole
        .keys()
        .map(String::as_str)
        .filter(|key| !matches!(*key, "albums" | "tracks"))
        .collect();
    if stray.is_empty() {
        return Ok(toml::Table::new());
    }
    Err(Error::Config(format!(
        "{}: put {} under [artists]; a map using [albums] or [tracks] cannot also          list artists at the top level",
        path.display(),
        stray.join(", ")
    )))
}

/// One entry's value: a genre, or a list of them.
fn read_genres(path: &Path, name: &str, value: &toml::Value) -> Result<Vec<String>> {
    match value {
        toml::Value::String(genre) => Ok(vec![genre.clone()]),
        toml::Value::Array(items) if items.iter().all(toml::Value::is_str) => Ok(items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()),
        _ => Err(Error::Config(format!(
            "{}: '{name}' must be a genre or a list of genres",
            path.display()
        ))),
    }
}

/// An `[albums]` or `[tracks]` section: artist, then what is being overridden.
fn read_overrides(path: &Path, section: &str, value: Option<&toml::Value>) -> Result<Overrides> {
    let Some(value) = value else {
        return Ok(Overrides::new());
    };
    let artists = value.as_table().ok_or_else(|| {
        Error::Config(format!(
            "{}: [{section}] must be a table of artist names",
            path.display()
        ))
    })?;

    let mut mapping = Overrides::new();
    for (artist, entries) in artists {
        let entries = entries.as_table().ok_or_else(|| {
            Error::Config(format!(
                "{}: [{section}.\"{artist}\"] must be a table",
                path.display()
            ))
        })?;
        for (name, value) in entries {
            let genres = read_genres(path, &format!("{artist} / {name}"), value)?;
            mapping.insert((normalize(artist), normalize(name)), genres);
        }
    }
    Ok(mapping)
}

/// What applying `genre_map` would do, without touching anything.
#[must_use]
pub fn plan_changes(entries: &[LibraryEntry], genre_map: &GenreMap) -> Vec<GenreChange> {
    entries
        .iter()
        .filter(|entry| genre_map.covers(TrackKey::of(entry)))
        .map(|entry| GenreChange {
            current: entry.tags.genres.clone(),
            proposed: genre_map.genres_for(TrackKey::of(entry)),
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
        let artist = TrackKey::of(entry).artist;
        if !artist.is_empty() && !genre_map.covers_artist(artist) {
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
