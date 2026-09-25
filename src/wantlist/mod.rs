//! Reading a wantlist and working out what is missing.
//!
//! Every format's reader produces the same [`Track`]s, keeping the source's own
//! columns in `source_row`. A track counts as owned only on a convincing match:
//! a duplicate download is cheaper than a skipped track.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::errors::{Error, Result};
use crate::matching::{blocking_keys, score_track, MatchThresholds, NameForms};
use crate::models::{LibraryEntry, Track};
use crate::services::ServiceHttp;

pub use discogs::Discogs;
pub use lastfm::LastFm;

mod apple;
mod delimited;
pub mod discogs;
pub mod lastfm;
mod m3u;

/// The prefix naming a Last.fm user where an export file would go.
pub const LASTFM_PREFIX: &str = "lastfm:";

pub const ARTIST_COLUMNS: &[&str] = &[
    "artist name",
    "artist name(s)",
    "artist",
    "artists",
    "album artist",
    "artist name 1",
];
/// `track` comes last: Last.fm's own export names the title so, but elsewhere
/// it is as likely to be a track number.
pub const TITLE_COLUMNS: &[&str] = &["track name", "title", "name", "song", "song title", "track"];
pub const ALBUM_COLUMNS: &[&str] = &["album name", "album", "album title"];
pub const DURATION_COLUMNS: &[&str] = &[
    "duration (ms)",
    "duration_ms",
    "track duration (ms)",
    "length",
];
/// Not read into a [`Track`] field: it stays in `source_row` and is looked up
/// when a download is tagged.
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

/// The shapes a wantlist is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A delimited export, recognised by its header, or a headerless Last.fm log.
    Csv,
    /// An Apple Music or iTunes `Library.xml`.
    Apple,
    /// An M3U or M3U8 playlist.
    M3u,
    /// A Discogs collection export, one row per release.
    Discogs,
}

impl Format {
    pub const ALL: [Self; 4] = [Self::Csv, Self::Apple, Self::M3u, Self::Discogs];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Apple => "apple",
            Self::M3u => "m3u",
            Self::Discogs => "discogs",
        }
    }

    /// What `path` holds, judged by its content first and its extension second.
    pub fn detect(path: &Path) -> Result<Self> {
        let failed = |error: std::io::Error| Error::Config(format!("{}: {error}", path.display()));
        let mut opening = Vec::with_capacity(512);
        std::fs::File::open(path)
            .and_then(|file| file.take(512).read_to_end(&mut opening))
            .map_err(failed)?;
        let opening = String::from_utf8_lossy(&opening);
        let playlist_extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("m3u") || extension.eq_ignore_ascii_case("m3u8")
            });

        Ok(if opening.contains("<plist") {
            Self::Apple
        } else if opening
            .trim_start_matches('\u{feff}')
            .starts_with("#EXTM3U")
            || playlist_extension
        {
            Self::M3u
        } else if discogs::is_collection(opening.lines().next().unwrap_or("")) {
            Self::Discogs
        } else {
            Self::Csv
        })
    }
}

impl fmt::Display for Format {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|format| format.as_str() == text)
            .ok_or_else(|| {
                format!(
                    "unknown format: {text}; known: {}",
                    Self::ALL.map(Self::as_str).join(", ")
                )
            })
    }
}

/// Where a wantlist is read from: a file, or a Last.fm user's profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    LastFm(String),
}

impl FromStr for Source {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        match text.strip_prefix(LASTFM_PREFIX) {
            Some(user) if user.trim().is_empty() => Err(format!(
                "{LASTFM_PREFIX} needs a username, as in {LASTFM_PREFIX}rj"
            )),
            Some(user) => Ok(Self::LastFm(user.trim().to_owned())),
            None => Ok(Self::File(PathBuf::from(text))),
        }
    }
}

/// The web services a wantlist may be read through, beyond the file itself.
pub struct Remote<'a> {
    pub http: &'a dyn ServiceHttp,
    pub discogs: &'a Discogs,
    pub lastfm: &'a LastFm,
}

/// Read a wantlist from wherever it is, keeping under `min_plays` only what
/// was played that often.
///
/// A file lists a play per row, so `min_plays` counts its repeats. A Last.fm
/// profile counts its own plays, so there `min_plays` adds the tracks played
/// that often to the loved ones.
pub fn read_source(
    source: &Source,
    format: Option<Format>,
    min_plays: Option<usize>,
    remote: &Remote<'_>,
) -> Result<Vec<Track>> {
    match source {
        Source::File(path) => {
            let tracks = read_export(path, format, Some(remote))?;
            Ok(match min_plays {
                Some(min_plays) => keep_played(tracks, min_plays),
                None => tracks,
            })
        }
        Source::LastFm(user) => {
            if let Some(format) = format {
                return Err(Error::Config(format!(
                    "--format {format} reads a file, and {LASTFM_PREFIX}{user} is a Last.fm profile"
                )));
            }
            remote.lastfm.read(remote.http, user, min_plays)
        }
    }
}

/// Read a wantlist into tracks, as `format`, or as whatever it looks like.
///
/// Only a Discogs collection needs `remote`; read without one, it is refused.
pub fn read_export(
    path: &Path,
    format: Option<Format>,
    remote: Option<&Remote<'_>>,
) -> Result<Vec<Track>> {
    let format = match format {
        Some(format) => format,
        None => Format::detect(path)?,
    };
    match format {
        Format::Csv => delimited::read(path),
        Format::Apple => apple::read(path),
        Format::M3u => m3u::read(path),
        Format::Discogs => match remote {
            Some(remote) => discogs::read(path, remote.http, remote.discogs),
            None => Err(Error::Config(format!(
                "{}: a Discogs collection is read through the Discogs API",
                path.display()
            ))),
        },
    }
}

/// The tracks listed at least `min_plays` times, once each, in first-seen order.
#[must_use]
pub fn keep_played(tracks: Vec<Track>, min_plays: usize) -> Vec<Track> {
    let key = |track: &Track| track.query().to_lowercase();
    let mut plays: HashMap<String, usize> = HashMap::new();
    for track in &tracks {
        *plays.entry(key(track)).or_default() += 1;
    }

    let mut kept = HashSet::new();
    tracks
        .into_iter()
        .filter(|track| {
            let key = key(track);
            plays[&key] >= min_plays && kept.insert(key)
        })
        .collect()
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
    // Sorted so ties resolve reproducibly.
    ordered.sort_unstable();
    ordered
}

/// Includes en and em dashes, which pasted track names often carry.
pub const SEPARATORS: &[&str] = &[" - ", " \u{2013} ", " \u{2014} "];

/// Split `artist - title` into its parts, tolerating either being absent.
///
/// A query with no separator is treated as a title.
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
