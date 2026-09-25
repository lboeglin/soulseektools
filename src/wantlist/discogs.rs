//! Discogs collection exports: one row per release, each release's tracklist
//! fetched from the API and cached, since it never changes.
//!
//! The row's `Artist` and `Title` name the release, so they become `album
//! artist` and `album`. In a tracklist a `heading` is not a song, and an
//! `index`'s movements are prefixed with the work's title.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::parse_duration;
use crate::errors::{Error, Result};
use crate::models::Track;
use crate::services::{RateLimiter, ServiceHttp};

pub const RELEASE_URL: &str = "https://api.discogs.com/releases";

/// Discogs allows 25 requests a minute without a token.
pub const MIN_INTERVAL_SECONDS: f64 = 60.0 / 25.0;

/// And 60 with one.
pub const MIN_INTERVAL_WITH_TOKEN_SECONDS: f64 = 1.0;

/// The column a collection export is recognised by.
pub const RELEASE_COLUMN: &str = "release_id";

/// Discogs tells two artists of one name apart as `Care Company (2)`, and
/// marks a name printed differently on the sleeve with a trailing `*`.
static DISAMBIGUATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d+\)$").unwrap());

/// The Discogs client, with the cache of the releases it has fetched.
pub struct Discogs {
    limiter: RateLimiter,
    token: Option<String>,
    cache: PathBuf,
}

impl Discogs {
    #[must_use]
    pub fn new(token: Option<String>, min_interval_seconds: f64, cache: PathBuf) -> Self {
        Self {
            limiter: RateLimiter::new(min_interval_seconds),
            token,
            cache,
        }
    }

    /// The pacing Discogs publishes for a caller with or without a token.
    #[must_use]
    pub fn min_interval_seconds(token: Option<&str>) -> f64 {
        if token.is_some() {
            MIN_INTERVAL_WITH_TOKEN_SECONDS
        } else {
            MIN_INTERVAL_SECONDS
        }
    }

    /// One release, as far as a wantlist needs it: title, artists, tracklist.
    fn release(&self, http: &dyn ServiceHttp, id: &str) -> Result<Value> {
        let params: Vec<(&str, String)> = self
            .token
            .iter()
            .map(|token| ("token", token.clone()))
            .collect();

        self.limiter.wait();
        let release = http
            .get_json(&format!("{RELEASE_URL}/{id}"), &params)
            .map_err(|error| {
                let mut message = error.to_string();
                // The token rides in the query string, which the transport quotes back.
                if let Some(token) = &self.token {
                    message = message.replace(token.as_str(), "<token>");
                }
                Error::Config(format!("Discogs release {id}: {message}"))
            })?;

        Ok(json!({
            "title": release.get("title").cloned().unwrap_or(Value::Null),
            "artists": release.get("artists").cloned().unwrap_or(Value::Null),
            "tracklist": release.get("tracklist").cloned().unwrap_or(Value::Null),
        }))
    }
}

/// Whether a delimited file's header names the column a collection export has.
pub(super) fn is_collection(header: &str) -> bool {
    header
        .trim_start_matches('\u{feff}')
        .split(',')
        .any(|column| {
            column
                .trim()
                .trim_matches('"')
                .eq_ignore_ascii_case(RELEASE_COLUMN)
        })
}

pub(super) fn read(path: &Path, http: &dyn ServiceHttp, discogs: &Discogs) -> Result<Vec<Track>> {
    let mut reader = csv::Reader::from_path(path).map_err(|error| failed(path, &error))?;
    let headers: Vec<String> = reader
        .headers()
        .map_err(|error| failed(path, &error))?
        .iter()
        .map(|name| name.trim_start_matches('\u{feff}').trim().to_owned())
        .collect();
    let position = |wanted: &str| {
        headers
            .iter()
            .position(|name| name.eq_ignore_ascii_case(wanted))
    };
    let Some(release_at) = position(RELEASE_COLUMN) else {
        return Err(Error::Config(format!(
            "{}: not a Discogs collection export, no {RELEASE_COLUMN} column",
            path.display()
        )));
    };
    let release_columns = [position("artist"), position("title")];

    let mut cache = Cache::open(&discogs.cache);
    let mut tracks = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|error| failed(path, &error))?;
        let id = record.get(release_at).unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }

        let release = if let Some(release) = cache.get(id) {
            release.clone()
        } else {
            let release = discogs.release(http, id)?;
            cache.insert(id, release.clone())?;
            release
        };

        let row: Vec<(String, String)> = headers
            .iter()
            .zip(record.iter())
            .enumerate()
            .filter(|(at, (key, _))| !key.is_empty() && !release_columns.contains(&Some(*at)))
            .map(|(_, (key, value))| (key.clone(), value.to_owned()))
            .collect();
        tracks.extend(release_tracks(&release, &row));
    }
    Ok(tracks)
}

fn release_tracks(release: &Value, row: &[(String, String)]) -> Vec<Track> {
    let album = text(release.get("title"));
    let album_artist = credited(release.get("artists"));

    let mut tracks = Vec::new();
    for entry in list(release.get("tracklist")) {
        match entry.get("type_").and_then(Value::as_str) {
            Some("heading") => {}
            Some("index") => {
                let work = text(entry.get("title"));
                for movement in list(entry.get("sub_tracks")) {
                    let movement_title = text(movement.get("title"));
                    let title = if work.is_empty() {
                        movement_title
                    } else {
                        format!("{work}: {movement_title}")
                    };
                    tracks.extend(track(movement, title, &album, &album_artist, row));
                }
            }
            _ => tracks.extend(track(
                entry,
                text(entry.get("title")),
                &album,
                &album_artist,
                row,
            )),
        }
    }
    tracks
}

fn track(
    entry: &Value,
    title: String,
    album: &str,
    album_artist: &str,
    row: &[(String, String)],
) -> Option<Track> {
    if title.is_empty() {
        return None;
    }
    let own = credited(entry.get("artists"));
    let artist = if own.is_empty() {
        album_artist.to_owned()
    } else {
        own
    };
    let position = text(entry.get("position"));
    let duration = text(entry.get("duration"));

    let mut source_row = vec![
        ("artist".to_owned(), artist.clone()),
        ("title".to_owned(), title.clone()),
        ("album".to_owned(), album.to_owned()),
        ("album artist".to_owned(), album_artist.to_owned()),
        ("track number".to_owned(), position),
        ("duration".to_owned(), duration.clone()),
    ];
    source_row.extend(row.iter().cloned());

    Some(Track {
        artist,
        title,
        album: album.to_owned(),
        duration_seconds: parse_duration(&duration),
        title_guessed: false,
        source_row,
    })
}

/// An artist credit as Discogs prints it: `Stacey Pullen Presents Black Odyssey`.
fn credited(artists: Option<&Value>) -> String {
    let mut text = String::new();
    for artist in list(artists) {
        let name = artist.get("name").and_then(Value::as_str).unwrap_or("");
        text.push_str(&plain_name(name));
        match artist
            .get("join")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
        {
            "" => {}
            "," => text.push_str(", "),
            join => {
                text.push(' ');
                text.push_str(join);
                text.push(' ');
            }
        }
    }
    text.trim().to_owned()
}

fn plain_name(name: &str) -> String {
    DISAMBIGUATION
        .replace(name.trim().trim_end_matches('*'), "")
        .trim()
        .to_owned()
}

/// Every release fetched so far, by ID, as one JSON object on disk.
struct Cache {
    path: PathBuf,
    releases: Map<String, Value>,
}

impl Cache {
    /// A missing cache is an empty one, and so is an unreadable one: all it
    /// costs is fetching again.
    fn open(path: &Path) -> Self {
        let releases = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: path.to_path_buf(),
            releases,
        }
    }

    fn get(&self, id: &str) -> Option<&Value> {
        self.releases.get(id)
    }

    /// Written through a temporary file, so an interrupted write never leaves
    /// half a cache behind.
    fn insert(&mut self, id: &str, release: Value) -> Result<()> {
        self.releases.insert(id.to_owned(), release);

        let failed =
            |error: std::io::Error| Error::Config(format!("{}: {error}", self.path.display()));
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(failed)?;
            }
        }
        let body = serde_json::to_vec(&self.releases)
            .map_err(|error| Error::Config(format!("{}: {error}", self.path.display())))?;
        let partial = self.path.with_extension("json.partial");
        std::fs::write(&partial, body).map_err(failed)?;
        std::fs::rename(&partial, &self.path).map_err(failed)
    }
}

fn failed(path: &Path, error: &csv::Error) -> Error {
    Error::Config(format!("{}: {error}", path.display()))
}

fn list(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}
