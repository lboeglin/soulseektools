//! SQLite cache of the library, so commands stop rescanning everything.
//!
//! Reading tags, artwork size and duration for a few thousand files takes long
//! enough that the original scripts each paid it again from scratch. Rows here
//! are keyed by path and invalidated by `(size, mtime)`, so a rescan only
//! touches files that actually changed, and the expensive derived values --
//! normalised and romanised name forms -- are stored rather than recomputed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::audio::fingerprint::{Fingerprint, FingerprintKind, Values};
use crate::errors::{Error, Result};
use crate::library::scan::{folder_artist, iter_audio_files, AUDIO_EXTENSIONS};
use crate::library::tags;
use crate::matching::NameForms;
use crate::models::{LibraryEntry, TrackTags, Verdict};
use crate::text::normalize::strip_artist_prefix;

/// Bumped from 2 to 3 by the schema additions in #2.
pub const SCHEMA_VERSION: i64 = 3;

/// The lowest version this can open without migrating.
///
/// The Python build writes 2 and rewrites it to 2 every time it opens the file,
/// so both numbers have to be acceptable for the two implementations to share a
/// database while `py/` still exists.
pub const MINIMUM_SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    path               TEXT PRIMARY KEY,
    size               INTEGER NOT NULL,
    mtime              REAL    NOT NULL,
    duration_seconds   REAL,
    artwork_bytes      INTEGER NOT NULL DEFAULT 0,
    folder_artist      TEXT    NOT NULL DEFAULT '',
    tag_artist         TEXT    NOT NULL DEFAULT '',
    tag_album_artist   TEXT    NOT NULL DEFAULT '',
    tag_album          TEXT    NOT NULL DEFAULT '',
    tag_title          TEXT    NOT NULL DEFAULT '',
    tag_genres         TEXT    NOT NULL DEFAULT '[]',
    tag_track_number   INTEGER,
    artist_raw         TEXT    NOT NULL DEFAULT '',
    artist_normalized  TEXT    NOT NULL DEFAULT '',
    artist_romaji      TEXT    NOT NULL DEFAULT '',
    title_raw          TEXT    NOT NULL DEFAULT '',
    title_normalized   TEXT    NOT NULL DEFAULT '',
    title_romaji       TEXT    NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS files_artist_normalized ON files (artist_normalized);
CREATE INDEX IF NOT EXISTS files_title_normalized  ON files (title_normalized);

CREATE TABLE IF NOT EXISTS fingerprints (
    path   TEXT PRIMARY KEY,
    mtime  REAL NOT NULL,
    kind   TEXT NOT NULL,
    values_json TEXT NOT NULL,
    duration_seconds REAL
);

CREATE TABLE IF NOT EXISTS changes (
    id          INTEGER PRIMARY KEY,
    applied_at  REAL NOT NULL,
    command     TEXT NOT NULL,
    path        TEXT NOT NULL,
    field       TEXT NOT NULL,
    old_value   BLOB,
    new_value   BLOB
);
";

/// The six columns schema v3 adds to `files`, from #2.
///
/// Every one is nullable, which is what lets a v2 Python build open a v3
/// database: its `INSERT OR REPLACE` names its own columns and these fall to
/// NULL. Added with `ALTER TABLE` rather than in [`SCHEMA`] so that opening a
/// database the Python created upgrades it in place.
const V3_COLUMNS: &[(&str, &str)] = &[
    ("codec", "TEXT"),
    ("bitrate", "INTEGER"),
    ("sample_rate", "INTEGER"),
    ("bit_depth", "INTEGER"),
    ("verdict", "TEXT"),
    ("verdict_at", "REAL"),
];

const COLUMNS: &[&str] = &[
    "path",
    "size",
    "mtime",
    "duration_seconds",
    "artwork_bytes",
    "folder_artist",
    "tag_artist",
    "tag_album_artist",
    "tag_album",
    "tag_title",
    "tag_genres",
    "tag_track_number",
    "artist_raw",
    "artist_normalized",
    "artist_romaji",
    "title_raw",
    "title_normalized",
    "title_romaji",
    "codec",
    "bitrate",
    "sample_rate",
    "bit_depth",
];

/// What a refresh changed, for reporting and for asserting incrementality.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshResult {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub failed: Vec<(PathBuf, String)>,
}

impl RefreshResult {
    #[must_use]
    pub fn scanned(&self) -> usize {
        self.added + self.updated + self.unchanged
    }
}

/// Read everything the index stores about one file.
///
/// Tags win over the directory layout, which wins over the filename: a large
/// part of any Soulseek-sourced library arrives with no tags at all, and the
/// folder it landed in is then the only artist information there is.
pub fn describe(path: &Path, root: &Path) -> Result<LibraryEntry> {
    let metadata = path
        .metadata()
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))?;
    let artist_folder = folder_artist(path, root);

    let read = tags::read_tags(path).unwrap_or_default();
    let stream = tags::read_stream_info(path).unwrap_or_default();

    let artist = if read.artist.is_empty() {
        if read.album_artist.is_empty() {
            artist_folder.clone()
        } else {
            read.album_artist.clone()
        }
    } else {
        read.artist.clone()
    };

    let title = if read.title.is_empty() {
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let against = if artist_folder.is_empty() {
            &artist
        } else {
            &artist_folder
        };
        strip_artist_prefix(&stem, against)
    } else {
        read.title.clone()
    };

    Ok(LibraryEntry {
        path: path.to_path_buf(),
        size: metadata.len(),
        mtime: modified_seconds(&metadata),
        folder_artist: artist_folder,
        artist: NameForms::of(&artist),
        title: NameForms::of(&title),
        duration_seconds: stream.duration_seconds,
        artwork_bytes: tags::artwork_size(path),
        tags: read,
        codec: stream.codec,
        bitrate: stream.bitrate,
        sample_rate: stream.sample_rate,
        bit_depth: stream.bit_depth,
    })
}

fn modified_seconds(metadata: &std::fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Read/write access to the cached view of a library.
pub struct LibraryIndex {
    connection: Connection,
    pub database: PathBuf,
}

impl LibraryIndex {
    pub fn open(database: &Path) -> Result<Self> {
        if let Some(parent) = database.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| Error::Config(format!("{}: {error}", parent.display())))?;
            }
        }

        let connection = Connection::open(database)
            .map_err(|error| Error::Config(format!("{}: {error}", database.display())))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|error| sqlite_error(&error))?;

        let existing: HashSet<String> = connection
            .prepare("SELECT name FROM pragma_table_info('files')")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))?;
        for (name, kind) in V3_COLUMNS {
            if !existing.contains(*name) {
                connection
                    .execute_batch(&format!("ALTER TABLE files ADD COLUMN {name} {kind}"))
                    .map_err(|error| sqlite_error(&error))?;
            }
        }

        connection
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )
            .map_err(|error| sqlite_error(&error))?;

        Ok(Self {
            connection,
            database: database.to_path_buf(),
        })
    }

    /// The schema version recorded in the file, whoever wrote it last.
    pub fn schema_version(&self) -> Result<i64> {
        let recorded: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sqlite_error(&error))?;
        Ok(recorded
            .and_then(|value| value.parse().ok())
            .unwrap_or(SCHEMA_VERSION))
    }

    /// Bring the index in line with the files under `root`.
    ///
    /// Only files whose size or modification time changed are re-read, unless
    /// `full` forces every file to be reprocessed.
    pub fn refresh(
        &mut self,
        root: &Path,
        extensions: &[&str],
        full: bool,
        mut on_file: Option<&mut dyn FnMut(&Path)>,
    ) -> Result<RefreshResult> {
        let known: HashMap<String, (i64, f64)> = self
            .connection
            .prepare("SELECT path, size, mtime FROM files")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?)))
                    })?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))?;

        let mut result = RefreshResult::default();
        let mut seen: HashSet<String> = HashSet::new();

        for path in iter_audio_files(root, extensions) {
            let key = path.display().to_string();
            seen.insert(key.clone());
            if let Some(report) = on_file.as_mut() {
                report(&path);
            }

            let Ok(metadata) = path.metadata() else {
                continue;
            };
            let cached = known.get(&key);
            let current = (
                i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                modified_seconds(&metadata),
            );
            if !full && cached == Some(&current) {
                result.unchanged += 1;
                continue;
            }

            match describe(&path, root) {
                Ok(entry) => {
                    self.upsert(&entry)?;
                    if cached.is_none() {
                        result.added += 1;
                    } else {
                        result.updated += 1;
                    }
                }
                Err(error) => result.failed.push((path, error.to_string())),
            }
        }

        let stale: Vec<&String> = known.keys().filter(|path| !seen.contains(*path)).collect();
        result.removed = stale.len();
        for path in stale {
            self.connection
                .execute("DELETE FROM files WHERE path = ?1", params![path])
                .map_err(|error| sqlite_error(&error))?;
        }

        Ok(result)
    }

    /// Every indexed file, ordered by path.
    pub fn entries(&self) -> Result<Vec<LibraryEntry>> {
        let query = format!("SELECT {} FROM files ORDER BY path", COLUMNS.join(", "));
        self.connection
            .prepare(&query)
            .and_then(|mut statement| statement.query_map([], row_to_entry)?.collect())
            .map_err(|error| sqlite_error(&error))
    }

    pub fn entry(&self, path: &Path) -> Result<Option<LibraryEntry>> {
        let query = format!("SELECT {} FROM files WHERE path = ?1", COLUMNS.join(", "));
        self.connection
            .query_row(&query, params![path.display().to_string()], row_to_entry)
            .optional()
            .map_err(|error| sqlite_error(&error))
    }

    pub fn count(&self) -> Result<usize> {
        self.connection
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
            .map(|count| usize::try_from(count).unwrap_or(0))
            .map_err(|error| sqlite_error(&error))
    }

    /// Number of indexed files per artist folder, largest first.
    pub fn artist_counts(&self) -> Result<Vec<(String, usize)>> {
        self.connection
            .prepare(
                "SELECT folder_artist, COUNT(*) AS total FROM files \
                 GROUP BY folder_artist ORDER BY total DESC, folder_artist",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        ))
                    })?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))
    }

    /// Re-read one file, after this tool has written to it.
    ///
    /// A command that edits tags and leaves the cache alone makes the next
    /// command report what was true before the edit.
    pub fn reindex(&mut self, path: &Path, root: &Path) -> Result<Option<LibraryEntry>> {
        if !path.is_file() {
            return Ok(None);
        }
        let entry = describe(path, root)?;
        self.upsert(&entry)?;
        Ok(Some(entry))
    }

    /// Record what `stk verify` concluded, so the next run can skip the file.
    ///
    /// New in schema v3 (#2): `verify` is the most expensive command in the tool
    /// and currently redoes every file each run, including ones whose
    /// `(size, mtime)` have not moved.
    pub fn set_verdict(&mut self, path: &Path, verdict: Verdict, at: f64) -> Result<()> {
        self.connection
            .execute(
                "UPDATE files SET verdict = ?2, verdict_at = ?3 WHERE path = ?1",
                params![path.display().to_string(), verdict.as_str(), at],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(())
    }

    pub fn verdict(&self, path: &Path) -> Result<Option<Verdict>> {
        let stored: Option<Option<String>> = self
            .connection
            .query_row(
                "SELECT verdict FROM files WHERE path = ?1",
                params![path.display().to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sqlite_error(&error))?;
        Ok(stored.flatten().and_then(|value| value.parse().ok()))
    }

    /// A cached fingerprint, or `None` if absent or computed before an edit.
    pub fn fingerprint(&self, path: &Path, mtime: f64) -> Result<Option<Fingerprint>> {
        let row: Option<(String, String, Option<f64>)> = self
            .connection
            .query_row(
                "SELECT kind, values_json, duration_seconds FROM fingerprints                  WHERE path = ?1 AND mtime = ?2",
                params![path.display().to_string(), mtime],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| sqlite_error(&error))?;

        let Some((kind, values, duration_seconds)) = row else {
            return Ok(None);
        };
        let values = match kind.parse::<FingerprintKind>() {
            Ok(FingerprintKind::Chromaprint) => {
                Values::Chromaprint(serde_json::from_str(&values).unwrap_or_default())
            }
            Ok(FingerprintKind::Envelope) => {
                Values::Envelope(serde_json::from_str(&values).unwrap_or_default())
            }
            Err(_) => return Ok(None),
        };
        Ok(Some(Fingerprint {
            values,
            duration_seconds,
        }))
    }

    pub fn store_fingerprint(
        &mut self,
        path: &Path,
        mtime: f64,
        value: &Fingerprint,
    ) -> Result<()> {
        let values = match &value.values {
            Values::Chromaprint(values) => serde_json::to_string(values),
            Values::Envelope(values) => serde_json::to_string(values),
        }
        .unwrap_or_else(|_| "[]".to_owned());

        self.connection
            .execute(
                "INSERT OR REPLACE INTO fingerprints                  (path, mtime, kind, values_json, duration_seconds) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    path.display().to_string(),
                    mtime,
                    value.kind().as_str(),
                    values,
                    value.duration_seconds,
                ],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(())
    }

    fn upsert(&mut self, entry: &LibraryEntry) -> Result<()> {
        let placeholders = (1..=COLUMNS.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let statement = format!(
            "INSERT OR REPLACE INTO files ({}) VALUES ({placeholders})",
            COLUMNS.join(", ")
        );

        self.connection
            .execute(
                &statement,
                params![
                    entry.path.display().to_string(),
                    i64::try_from(entry.size).unwrap_or(i64::MAX),
                    entry.mtime,
                    entry.duration_seconds,
                    i64::try_from(entry.artwork_bytes).unwrap_or(i64::MAX),
                    entry.folder_artist,
                    entry.tags.artist,
                    entry.tags.album_artist,
                    entry.tags.album,
                    entry.tags.title,
                    serde_json::to_string(&entry.tags.genres).unwrap_or_else(|_| "[]".to_owned()),
                    entry.tags.track_number,
                    entry.artist.raw,
                    entry.artist.normalized,
                    entry.artist.romaji,
                    entry.title.raw,
                    entry.title.normalized,
                    entry.title.romaji,
                    entry.codec,
                    entry.bitrate,
                    entry.sample_rate,
                    entry.bit_depth,
                ],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(())
    }
}

fn sqlite_error(error: &rusqlite::Error) -> Error {
    Error::Config(error.to_string())
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryEntry> {
    let genres: String = row.get("tag_genres")?;
    Ok(LibraryEntry {
        path: PathBuf::from(row.get::<_, String>("path")?),
        size: u64::try_from(row.get::<_, i64>("size")?).unwrap_or(0),
        mtime: row.get("mtime")?,
        folder_artist: row.get("folder_artist")?,
        tags: TrackTags {
            artist: row.get("tag_artist")?,
            album_artist: row.get("tag_album_artist")?,
            album: row.get("tag_album")?,
            title: row.get("tag_title")?,
            genres: serde_json::from_str(&genres).unwrap_or_default(),
            track_number: row.get("tag_track_number")?,
        },
        artist: NameForms {
            raw: row.get("artist_raw")?,
            normalized: row.get("artist_normalized")?,
            romaji: row.get("artist_romaji")?,
        },
        title: NameForms {
            raw: row.get("title_raw")?,
            normalized: row.get("title_normalized")?,
            romaji: row.get("title_romaji")?,
        },
        duration_seconds: row.get("duration_seconds")?,
        artwork_bytes: u64::try_from(row.get::<_, i64>("artwork_bytes")?).unwrap_or(0),
        codec: row.get::<_, Option<String>>("codec")?.unwrap_or_default(),
        bitrate: row.get("bitrate")?,
        sample_rate: row.get("sample_rate")?,
        bit_depth: row.get("bit_depth")?,
    })
}

/// The default extension set, as `refresh` wants it.
#[must_use]
pub fn default_extensions() -> &'static [&'static str] {
    AUDIO_EXTENSIONS
}
