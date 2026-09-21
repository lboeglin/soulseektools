//! SQLite cache of the library, so commands stop rescanning everything.
//!
//! Reading tags, artwork size and duration for a few thousand files takes long
//! enough that the original scripts each paid it again from scratch. Rows here
//! are keyed by path and invalidated by `(size, mtime)`, so a rescan only
//! touches files that actually changed, and the expensive derived values --
//! normalised and romanised name forms -- are stored rather than recomputed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::audio::fingerprint::{self, Fingerprint};
use crate::errors::{Error, Result};
use crate::library::changes::{Change, ChangeFilter, NewChange};
use crate::library::scan::{
    folder_artist, iter_audio_files, owning_root, Excludes, AUDIO_EXTENSIONS,
};
use crate::library::stats::LibrarySnapshot;
use crate::library::tags;
use crate::matching::NameForms;
use crate::models::{LibraryEntry, ReplayGainTags, TrackTags, Verdict};
use crate::text::normalize::strip_artist_prefix;

/// Bumped from 5 to 6 by the loudness tags behind `stk loudness` (#7).
pub const SCHEMA_VERSION: i64 = 6;

/// The oldest version this opens rather than refusing.
///
/// Every version since is reached by adding columns in place, so a database
/// written before any of that still opens where it stands.
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

CREATE TABLE IF NOT EXISTS snapshots (
    id             INTEGER PRIMARY KEY,
    taken_at       REAL    NOT NULL,
    total          INTEGER NOT NULL,
    tagged         INTEGER NOT NULL,
    with_artwork   INTEGER NOT NULL,
    with_genre     INTEGER NOT NULL,
    lossless       INTEGER NOT NULL,
    verified_clean INTEGER NOT NULL,
    flagged        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS changes (
    id          INTEGER PRIMARY KEY,
    run_id      INTEGER NOT NULL DEFAULT 0,
    applied_at  REAL NOT NULL,
    command     TEXT NOT NULL,
    path        TEXT NOT NULL,
    field       TEXT NOT NULL,
    old_value   BLOB,
    new_value   BLOB
);
";

/// Indexes over columns a migration may still be about to add, so they are
/// created after it rather than alongside the tables.
const MIGRATED_INDEXES: &str = "
CREATE INDEX IF NOT EXISTS changes_run ON changes (run_id);
";

/// The six columns schema v3 adds to `files`, from #2.
///
/// Every one is nullable, so a build that predates them can still write a row:
/// its `INSERT OR REPLACE` names its own columns and these fall to NULL. Added
/// with `ALTER TABLE` rather than in [`SCHEMA`] so an existing database is
/// upgraded where it stands.
const V3_COLUMNS: &[(&str, &str)] = &[
    ("codec", "TEXT"),
    ("bitrate", "INTEGER"),
    ("sample_rate", "INTEGER"),
    ("bit_depth", "INTEGER"),
    ("verdict", "TEXT"),
    ("verdict_at", "REAL"),
];

/// The column schema v4 adds to `changes`, so an undo can revert a whole
/// `--apply` run and not only its last row.
///
/// In [`SCHEMA`] as well, for a database created from scratch; here for one the
/// port already wrote a v3 `changes` table into.
const V4_CHANGE_COLUMNS: &[(&str, &str)] = &[("run_id", "INTEGER NOT NULL DEFAULT 0")];

/// The six columns schema v6 adds to `files`, from #7.
///
/// The tags as the file spells them, so counting what nothing has level-matched
/// is a query rather than a walk. `describe` reads them off the header parse it
/// already performs, the same argument that put `codec` and `bitrate` here.
const V6_COLUMNS: &[(&str, &str)] = &[
    ("replay_gain_track_gain", "TEXT"),
    ("replay_gain_track_peak", "TEXT"),
    ("replay_gain_album_gain", "TEXT"),
    ("replay_gain_album_peak", "TEXT"),
    ("r128_track_gain", "TEXT"),
    ("r128_album_gain", "TEXT"),
];

/// Set while the v6 columns still hold nothing for rows an older build wrote.
const BACKFILL_PENDING: &str = "replay_gain_backfill_pending";

/// The column schema v6 adds to `snapshots`.
///
/// Nullable rather than `NOT NULL DEFAULT 0`: a measurement taken before v6 did
/// not count this, and saying so is not the same as saying it counted none.
/// Zero would make the first scan after an upgrade read as a jump from nothing
/// to the whole library.
const V6_SNAPSHOT_COLUMNS: &[(&str, &str)] = &[("with_replay_gain", "INTEGER")];

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
    "replay_gain_track_gain",
    "replay_gain_track_peak",
    "replay_gain_album_gain",
    "replay_gain_album_peak",
    "r128_track_gain",
    "r128_album_gain",
];

const CHANGE_COLUMNS: &str = "id, run_id, applied_at, command, path, field, old_value, new_value";

fn row_to_change(row: &rusqlite::Row<'_>) -> rusqlite::Result<Change> {
    Ok(Change {
        id: row.get(0)?,
        run_id: row.get(1)?,
        applied_at: row.get(2)?,
        command: row.get(3)?,
        path: PathBuf::from(row.get::<_, String>(4)?),
        field: row.get(5)?,
        old_value: row.get(6)?,
        new_value: row.get(7)?,
    })
}

const SNAPSHOT_COLUMNS: &str = concat!(
    "taken_at, total, tagged, with_artwork, with_genre, ",
    "lossless, verified_clean, flagged, with_replay_gain"
);

/// SQLite counts in `i64` and the domain counts in `usize`; the conversion
/// belongs at the boundary rather than in the type.
fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibrarySnapshot> {
    let count = |index: usize| -> rusqlite::Result<usize> {
        Ok(usize::try_from(row.get::<_, i64>(index)?).unwrap_or(0))
    };
    Ok(LibrarySnapshot {
        taken_at: row.get(0)?,
        total: count(1)?,
        tagged: count(2)?,
        with_artwork: count(3)?,
        with_genre: count(4)?,
        lossless: count(5)?,
        verified_clean: count(6)?,
        flagged: count(7)?,
        with_replay_gain: row
            .get::<_, Option<i64>>(8)?
            .map(|value| usize::try_from(value).unwrap_or(0)),
    })
}

fn stored(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

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

    let described = tags::describe_file(path).unwrap_or_default();
    let read = described.tags;
    let stream = described.stream;

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
        artwork_bytes: described.artwork_bytes,
        replay_gain: described.replay_gain,
        tags: read,
        codec: stream.codec,
        bitrate: stream.bitrate,
        sample_rate: stream.sample_rate,
        bit_depth: stream.bit_depth,
    })
}

/// Bring one table up to date in place, so a database an older build wrote
/// opens without being rewritten. Reports whether anything was added.
fn add_missing_columns(
    connection: &Connection,
    table: &str,
    columns: &[(&str, &str)],
) -> Result<bool> {
    let existing: HashSet<String> = connection
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect()
        })
        .map_err(|error| sqlite_error(&error))?;

    let mut added = false;
    for (name, kind) in columns {
        if !existing.contains(*name) {
            connection
                .execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {name} {kind}"))
                .map_err(|error| sqlite_error(&error))?;
            added = true;
        }
    }
    Ok(added)
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

        add_missing_columns(&connection, "files", V3_COLUMNS)?;
        add_missing_columns(&connection, "changes", V4_CHANGE_COLUMNS)?;
        let loudness_columns_added = add_missing_columns(&connection, "files", V6_COLUMNS)?;
        add_missing_columns(&connection, "snapshots", V6_SNAPSHOT_COLUMNS)?;
        if loudness_columns_added {
            connection
                .execute(
                    "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, '1')",
                    params![BACKFILL_PENDING],
                )
                .map_err(|error| sqlite_error(&error))?;
        }
        connection
            .execute_batch(MIGRATED_INDEXES)
            .map_err(|error| sqlite_error(&error))?;

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

    /// Bring the index in line with the files under every configured root.
    ///
    /// Only files whose size or modification time changed are re-read, unless
    /// `full` forces every file to be reprocessed.
    ///
    /// Every root has to be walked in one call: what the walk did not see is
    /// deleted at the end, so refreshing one root at a time would make each root
    /// evict the others.
    pub fn refresh(
        &mut self,
        roots: &[PathBuf],
        extensions: &[&str],
        exclude: &Excludes,
        full: bool,
        mut on_file: Option<&mut dyn FnMut(&Path)>,
    ) -> Result<RefreshResult> {
        self.backfill_replay_gain()?;

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

        for root in roots {
            for path in iter_audio_files(root, extensions, exclude) {
                let key = path.display().to_string();
                if !seen.insert(key.clone()) {
                    continue;
                }
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

                // Nested roots offer the same file twice, and the deeper one is
                // the one whose first component names the artist.
                match describe(&path, owning_root(&path, roots).unwrap_or(root)) {
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

    /// Fill the v6 loudness columns for rows an older build wrote.
    ///
    /// A row is invalidated by its `(size, mtime)`, and adding a column moves
    /// neither, so a library scanned before v6 would report every one of its
    /// files un-level-matched until something happened to touch it -- and would
    /// record a snapshot saying so. Re-describing the rows would fix it and
    /// throw away every verdict `stk verify` had recorded, because `upsert`
    /// replaces a row whole; these six columns are updated on their own instead.
    ///
    /// Runs once, at the first refresh after the upgrade. A file that has since
    /// gone is left to the walk, which removes it.
    fn backfill_replay_gain(&mut self) -> Result<()> {
        let pending: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![BACKFILL_PENDING],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sqlite_error(&error))?;
        if pending.is_none() {
            return Ok(());
        }

        let paths: Vec<String> = self
            .connection
            .prepare("SELECT path FROM files")
            .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
            .map_err(|error| sqlite_error(&error))?;

        let transaction = self
            .connection
            .transaction()
            .map_err(|error| sqlite_error(&error))?;
        for path in paths {
            let Ok(gain) = tags::read_replay_gain(Path::new(&path)) else {
                continue;
            };
            transaction
                .execute(
                    "UPDATE files SET replay_gain_track_gain = ?2, replay_gain_track_peak = ?3,                      replay_gain_album_gain = ?4, replay_gain_album_peak = ?5,                      r128_track_gain = ?6, r128_album_gain = ?7 WHERE path = ?1",
                    params![
                        path,
                        gain.track_gain,
                        gain.track_peak,
                        gain.album_gain,
                        gain.album_peak,
                        gain.r128_track_gain,
                        gain.r128_album_gain,
                    ],
                )
                .map_err(|error| sqlite_error(&error))?;
        }
        transaction
            .execute("DELETE FROM meta WHERE key = ?1", params![BACKFILL_PENDING])
            .map_err(|error| sqlite_error(&error))?;
        transaction.commit().map_err(|error| sqlite_error(&error))?;
        Ok(())
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
    pub fn reindex(&mut self, path: &Path, roots: &[PathBuf]) -> Result<Option<LibraryEntry>> {
        if !path.is_file() {
            return Ok(None);
        }
        let entry = describe(path, owning_root(path, roots).unwrap_or(Path::new("")))?;
        self.upsert(&entry)?;
        Ok(Some(entry))
    }

    /// Record what `stk verify` concluded, so the next run can skip the file.
    ///
    /// New in schema v3 (#2): `verify` is the most expensive command in the tool
    /// and redid every file each run, including ones whose `(size, mtime)` had
    /// not moved.
    ///
    /// A verdict outlives only the file it was made about: `upsert` replaces the
    /// whole row, so re-describing a changed file drops it.
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

    /// The stored verdict, but only while it still describes the file on disk.
    ///
    /// The comparison is the index's own invalidation rule -- the `(size, mtime)`
    /// the row was written from -- so a re-encoded or replaced file falls back to
    /// being analysed rather than answering with a verdict about its predecessor.
    pub fn current_verdict(&self, path: &Path) -> Result<Option<Verdict>> {
        let stored: Option<(Option<String>, i64, f64)> = self
            .connection
            .query_row(
                "SELECT verdict, size, mtime FROM files WHERE path = ?1",
                params![path.display().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| sqlite_error(&error))?;

        let Some((Some(verdict), size, mtime)) = stored else {
            return Ok(None);
        };
        let Ok(metadata) = path.metadata() else {
            return Ok(None);
        };
        let current = (
            i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            modified_seconds(&metadata),
        );
        if current != (size, mtime) {
            return Ok(None);
        }
        Ok(verdict.parse().ok())
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
        // A row written by something else -- the `fpcalc` backend #9 replaced,
        // or a future one -- is a miss rather than a value, so it is recomputed
        // instead of compared against a fingerprint it does not agree with.
        if kind != fingerprint::KIND {
            return Ok(None);
        }
        Ok(Some(Fingerprint {
            values: serde_json::from_str(&values).unwrap_or_default(),
            duration_seconds,
        }))
    }

    /// A fingerprint for each entry, computed only where the cache has none.
    ///
    /// The misses are decoded in parallel, the way `verify` fans out over its
    /// files: a fingerprint reads two minutes of audio, and a library-wide run
    /// is otherwise bounded by one core. What was computed is stored, so the
    /// next command needing the same print pays nothing.
    pub fn fingerprints(&mut self, entries: &[LibraryEntry]) -> Vec<Result<Fingerprint>> {
        let cached: Vec<Option<Fingerprint>> = entries
            .iter()
            .map(|entry| self.fingerprint(&entry.path, entry.mtime).ok().flatten())
            .collect();

        let missing: Vec<&LibraryEntry> = entries
            .iter()
            .zip(&cached)
            .filter(|(_, hit)| hit.is_none())
            .map(|(entry, _)| entry)
            .collect();
        let mut computed = missing
            .par_iter()
            .map(|entry| fingerprint::fingerprint(&entry.path))
            .collect::<Vec<_>>()
            .into_iter();

        entries
            .iter()
            .zip(cached)
            .map(|(entry, hit)| {
                if let Some(value) = hit {
                    return Ok(value);
                }
                let value = computed
                    .next()
                    .unwrap_or_else(|| unreachable!("one computed print per miss"));
                if let Ok(value) = &value {
                    let _ = self.store_fingerprint(&entry.path, entry.mtime, value);
                }
                value
            })
            .collect()
    }

    pub fn store_fingerprint(
        &mut self,
        path: &Path,
        mtime: f64,
        value: &Fingerprint,
    ) -> Result<()> {
        let values = serde_json::to_string(&value.values).unwrap_or_else(|_| "[]".to_owned());

        self.connection
            .execute(
                "INSERT OR REPLACE INTO fingerprints                  (path, mtime, kind, values_json, duration_seconds) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    path.display().to_string(),
                    mtime,
                    fingerprint::KIND,
                    values,
                    value.duration_seconds,
                ],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(())
    }

    /// Follow a file this tool moved.
    ///
    /// The fingerprint is carried across rather than dropped: it is keyed by
    /// path, and the audio did not change because the file did. The rest of the
    /// row is re-derived, since `folder_artist` and every name form come from
    /// where the file sits.
    ///
    /// The verdict does not survive, for the same reason `upsert` does not name
    /// it: a row is replaced wholesale, and a stale verdict is worse than none.
    pub fn relocate(
        &mut self,
        from: &Path,
        to: &Path,
        roots: &[PathBuf],
    ) -> Result<Option<LibraryEntry>> {
        let (from, to) = (from.display().to_string(), to.display().to_string());
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| sqlite_error(&error))?;
        transaction
            .execute("DELETE FROM files WHERE path = ?1", params![from])
            .map_err(|error| sqlite_error(&error))?;
        transaction
            .execute(
                "UPDATE OR REPLACE fingerprints SET path = ?2 WHERE path = ?1",
                params![from, to],
            )
            .map_err(|error| sqlite_error(&error))?;
        transaction.commit().map_err(|error| sqlite_error(&error))?;

        self.reindex(Path::new(&to), roots)
    }

    /// Forget a file that has been moved away or removed.
    pub fn forget(&mut self, path: &Path) -> Result<()> {
        let path = path.display().to_string();
        self.connection
            .execute("DELETE FROM files WHERE path = ?1", params![path])
            .map_err(|error| sqlite_error(&error))?;
        self.connection
            .execute("DELETE FROM fingerprints WHERE path = ?1", params![path])
            .map_err(|error| sqlite_error(&error))?;
        Ok(())
    }

    /// How many files carry each verdict `stk verify` has recorded.
    pub fn verdict_counts(&self) -> Result<std::collections::BTreeMap<Verdict, usize>> {
        let rows: Vec<(String, i64)> = self
            .connection
            .prepare(
                "SELECT verdict, COUNT(*) FROM files WHERE verdict IS NOT NULL GROUP BY verdict",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))?;

        Ok(rows
            .into_iter()
            .filter_map(|(verdict, total)| {
                verdict
                    .parse()
                    .ok()
                    .map(|verdict| (verdict, usize::try_from(total).unwrap_or(0)))
            })
            .collect())
    }

    /// Record a measurement, unless it says exactly what the last one said.
    ///
    /// Returns the row written, or `None` when nothing changed. Scanning four
    /// times on a quiet afternoon should leave one point on the trend, not four:
    /// a series of identical rows is not evidence of anything.
    pub fn record_snapshot(&mut self, snapshot: &LibrarySnapshot) -> Result<Option<i64>> {
        if self
            .snapshots(Some(1))?
            .first()
            .is_some_and(|last| last.counts() == snapshot.counts())
        {
            return Ok(None);
        }

        let placeholders = (1..=9).map(|index| format!("?{index}")).collect::<Vec<_>>();
        self.connection
            .execute(
                &format!(
                    "INSERT INTO snapshots ({SNAPSHOT_COLUMNS}) VALUES ({})",
                    placeholders.join(", ")
                ),
                params![
                    snapshot.taken_at,
                    stored(snapshot.total),
                    stored(snapshot.tagged),
                    stored(snapshot.with_artwork),
                    stored(snapshot.with_genre),
                    stored(snapshot.lossless),
                    stored(snapshot.verified_clean),
                    stored(snapshot.flagged),
                    snapshot.with_replay_gain.map(stored),
                ],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(Some(self.connection.last_insert_rowid()))
    }

    /// Recorded measurements, newest first.
    pub fn snapshots(&self, limit: Option<usize>) -> Result<Vec<LibrarySnapshot>> {
        let mut query = format!("SELECT {SNAPSHOT_COLUMNS} FROM snapshots ORDER BY id DESC");
        if let Some(limit) = limit {
            use std::fmt::Write as _;
            let _ = write!(query, " LIMIT {limit}");
        }
        self.connection
            .prepare(&query)
            .and_then(|mut statement| statement.query_map([], row_to_snapshot)?.collect())
            .map_err(|error| sqlite_error(&error))
    }

    /// The run a fresh `--apply` invocation should stamp its rows with.
    pub fn next_run_id(&self) -> Result<i64> {
        self.connection
            .query_row(
                "SELECT COALESCE(MAX(run_id), 0) + 1 FROM changes",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sqlite_error(&error))
    }

    /// The most recent run, which is what `undo --last` means.
    pub fn last_run_id(&self) -> Result<Option<i64>> {
        self.connection
            .query_row("SELECT MAX(run_id) FROM changes", [], |row| row.get(0))
            .optional()
            .map(Option::flatten)
            .map_err(|error| sqlite_error(&error))
    }

    pub fn record_change(&mut self, change: &NewChange) -> Result<i64> {
        self.connection
            .execute(
                "INSERT INTO changes (run_id, applied_at, command, path, field, old_value, new_value)                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    change.run_id,
                    change.applied_at,
                    change.command,
                    change.path.display().to_string(),
                    change.field.as_str(),
                    change.old_value,
                    change.new_value,
                ],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Logged writes, newest first.
    pub fn changes(&self, filter: &ChangeFilter) -> Result<Vec<Change>> {
        let mut query = format!("SELECT {CHANGE_COLUMNS} FROM changes");
        let mut conditions = Vec::new();
        let mut arguments: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(command) = &filter.command {
            conditions.push(format!("command = ?{}", arguments.len() + 1));
            arguments.push(Box::new(command.clone()));
        }
        if let Some(run) = filter.run {
            conditions.push(format!("run_id = ?{}", arguments.len() + 1));
            arguments.push(Box::new(run));
        }
        if !conditions.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&conditions.join(" AND "));
        }
        query.push_str(" ORDER BY id DESC");
        if let Some(limit) = filter.limit {
            use std::fmt::Write as _;
            let _ = write!(query, " LIMIT {limit}");
        }

        self.connection
            .prepare(&query)
            .and_then(|mut statement| {
                statement
                    .query_map(params_from_iter(arguments.iter()), row_to_change)?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))
    }

    pub fn change(&self, id: i64) -> Result<Option<Change>> {
        self.connection
            .query_row(
                &format!("SELECT {CHANGE_COLUMNS} FROM changes WHERE id = ?1"),
                params![id],
                row_to_change,
            )
            .optional()
            .map_err(|error| sqlite_error(&error))
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
                    entry.replay_gain.track_gain,
                    entry.replay_gain.track_peak,
                    entry.replay_gain.album_gain,
                    entry.replay_gain.album_peak,
                    entry.replay_gain.r128_track_gain,
                    entry.replay_gain.r128_album_gain,
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
        replay_gain: ReplayGainTags {
            track_gain: row.get("replay_gain_track_gain")?,
            track_peak: row.get("replay_gain_track_peak")?,
            album_gain: row.get("replay_gain_album_gain")?,
            album_peak: row.get("replay_gain_album_peak")?,
            r128_track_gain: row.get("r128_track_gain")?,
            r128_album_gain: row.get("r128_album_gain")?,
        },
    })
}

/// The default extension set, as `refresh` wants it.
#[must_use]
pub fn default_extensions() -> &'static [&'static str] {
    AUDIO_EXTENSIONS
}
