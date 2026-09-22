//! The review queue: what is wanted, what was found, and what was approved.
//!
//! This replaces the review CSV the Nicotine+ plugin polled every thirty
//! seconds. A CSV made the approval step a hand-edit and gave no way to record a
//! track's progress beyond a free-text column; a table makes each track's state
//! explicit and lets the TUI, the CLI and the downloader all read the same thing.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Transaction};

use crate::errors::{Error, Result};
use crate::models::{Candidate, ScoredCandidate, Track, TrackStatus};

/// Bumped from 1 to 2 by the retry tracking in #2, to 3 by the album fallback
/// in #6, and to 4 by the guessed titles in #13.
pub const SCHEMA_VERSION: i64 = 4;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tracks (
    id               INTEGER PRIMARY KEY,
    query            TEXT NOT NULL UNIQUE,
    artist           TEXT NOT NULL,
    title            TEXT NOT NULL,
    album            TEXT NOT NULL DEFAULT '',
    duration_seconds REAL,
    status           TEXT NOT NULL DEFAULT 'pending',
    detail           TEXT NOT NULL DEFAULT '',
    source_row       TEXT NOT NULL DEFAULT '{}',
    added_at         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS candidates (
    id               INTEGER PRIMARY KEY,
    track_id         INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    rank             INTEGER NOT NULL,
    score            INTEGER NOT NULL,
    title_ratio      REAL NOT NULL DEFAULT 0,
    flags            TEXT NOT NULL DEFAULT '[]',
    username         TEXT NOT NULL,
    remote_path      TEXT NOT NULL,
    size             INTEGER NOT NULL DEFAULT 0,
    bitrate          INTEGER,
    sample_rate      INTEGER,
    bit_depth        INTEGER,
    is_vbr           INTEGER NOT NULL DEFAULT 0,
    free_slots       INTEGER NOT NULL DEFAULT 0,
    queue_length     INTEGER NOT NULL DEFAULT 0,
    duration_seconds REAL,
    approved         INTEGER NOT NULL DEFAULT 0,
    attempted        INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS candidates_track ON candidates (track_id, rank);
CREATE INDEX IF NOT EXISTS tracks_status ON tracks (status);
";

/// The two columns schema v2 adds to `tracks`, from #2.
///
/// Nullable or defaulted, so a build that predates them opens the database
/// unchanged. Added with `ALTER TABLE` at open time so an existing database is
/// upgraded in place rather than rewritten.
const V2_COLUMNS: &[(&str, &str)] = &[
    ("queued_at", "REAL"),
    ("attempts", "INTEGER NOT NULL DEFAULT 0"),
];

/// The column schema v3 adds to `candidates`, from #6.
///
/// The retry used to promote `approved_rank + 1`, which assumes the choice being
/// retried was the first one. An album approval routinely takes a rank other
/// than 1 -- the whole point is preferring one peer's folder over each track's
/// own best -- so everything ranked *above* the album's pick was unreachable,
/// and a track failed with its own best copy never tried. Recording which
/// candidates have been tried is what lets the fallback reach them.
const V3_COLUMNS: &[(&str, &str)] = &[("attempted", "INTEGER NOT NULL DEFAULT 0")];

/// The column schema v4 adds to `tracks`, from #13.
///
/// A row `stk get --album` writes carries a title read off a peer's filename:
/// the folder was what was asked for, and no tracklist says what is inside it.
/// Every other row in the queue was typed or exported, so nothing had to record
/// the difference before -- and the difference decides whether `fetch download`
/// may write that title over the one the peer put on the file.
const V4_COLUMNS: &[(&str, &str)] = &[("title_guessed", "INTEGER NOT NULL DEFAULT 0")];

/// A wanted track and everything found for it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewTrack {
    pub id: i64,
    pub track: Track,
    pub status: TrackStatus,
    pub detail: String,
    pub candidates: Vec<ScoredCandidate>,
    pub approved_rank: Option<i64>,
    /// When the track entered `QUEUED`, so a stalled transfer is visible (#2).
    pub queued_at: Option<f64>,
    /// How many candidates have been tried for it (#2).
    pub attempts: i64,
}

impl ReviewTrack {
    #[must_use]
    pub fn approved(&self) -> Option<&ScoredCandidate> {
        let rank = self.approved_rank?;
        usize::try_from(rank - 1)
            .ok()
            .and_then(|position| self.candidates.get(position))
    }
}

/// Read/write access to the review queue.
pub struct ReviewStore {
    connection: Connection,
    pub database: PathBuf,
}

impl ReviewStore {
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
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|error| sqlite_error(&error))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|error| sqlite_error(&error))?;

        add_missing_columns(&connection, "tracks", V2_COLUMNS)?;
        add_missing_columns(&connection, "candidates", V3_COLUMNS)?;
        add_missing_columns(&connection, "tracks", V4_COLUMNS)?;

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

    /// Queue tracks for searching, skipping any already present.
    pub fn add(&mut self, tracks: &[Track]) -> Result<usize> {
        let now = now_iso8601();
        let mut added = 0;
        for track in tracks {
            added += self
                .connection
                .execute(
                    "INSERT OR IGNORE INTO tracks \
                     (query, artist, title, album, duration_seconds, status, source_row, \
                      title_guessed, added_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        track.query(),
                        track.artist,
                        track.title,
                        track.album,
                        track.duration_seconds,
                        TrackStatus::Pending.as_str(),
                        source_row_json(&track.source_row),
                        i64::from(track.title_guessed),
                        now,
                    ],
                )
                .map_err(|error| sqlite_error(&error))?;
        }
        Ok(added)
    }

    /// Tracks in the given states, oldest first; all of them when none is given.
    pub fn tracks(
        &self,
        statuses: &[TrackStatus],
        limit: Option<usize>,
    ) -> Result<Vec<ReviewTrack>> {
        let mut query = "SELECT * FROM tracks".to_owned();
        let mut parameters: Vec<String> = Vec::new();
        if !statuses.is_empty() {
            let placeholders = (1..=statuses.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = write!(query, " WHERE status IN ({placeholders})");
            parameters.extend(statuses.iter().map(|status| status.as_str().to_owned()));
        }
        query.push_str(" ORDER BY id");
        if let Some(limit) = limit {
            let _ = write!(query, " LIMIT {limit}");
        }

        let rows: Vec<PartialTrack> = self
            .connection
            .prepare(&query)
            .and_then(|mut statement| {
                statement
                    .query_map(params_from_iter(parameters.iter()), partial_track)?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))?;

        rows.into_iter().map(|row| self.hydrate(row)).collect()
    }

    pub fn track(&self, track_id: i64) -> Result<Option<ReviewTrack>> {
        let row = self
            .connection
            .query_row(
                "SELECT * FROM tracks WHERE id = ?1",
                params![track_id],
                partial_track,
            )
            .optional()
            .map_err(|error| sqlite_error(&error))?;
        row.map(|row| self.hydrate(row)).transpose()
    }

    /// Every track query the queue has ever held, whatever became of it.
    ///
    /// What makes `wantlist diff --new` possible: a track the queue rejected or
    /// failed on is still missing from the library, so a plain diff reports it
    /// again on every run. This says which of those have been seen before.
    pub fn known_queries(&self) -> Result<HashSet<String>> {
        self.connection
            .prepare("SELECT query FROM tracks")
            .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
            .map_err(|error| sqlite_error(&error))
    }

    pub fn counts(&self) -> Result<BTreeMap<TrackStatus, usize>> {
        let rows: Vec<(String, i64)> = self
            .connection
            .prepare("SELECT status, COUNT(*) AS total FROM tracks GROUP BY status")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))?;

        Ok(rows
            .into_iter()
            .filter_map(|(status, total)| {
                status
                    .parse()
                    .ok()
                    .map(|status| (status, usize::try_from(total).unwrap_or(0)))
            })
            .collect())
    }

    /// Record what a search found, replacing anything found before.
    pub fn set_candidates(&mut self, track_id: i64, candidates: &[ScoredCandidate]) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| sqlite_error(&error))?;
        transaction
            .execute(
                "DELETE FROM candidates WHERE track_id = ?1",
                params![track_id],
            )
            .map_err(|error| sqlite_error(&error))?;

        for (index, scored) in candidates.iter().enumerate() {
            let candidate = &scored.candidate;
            transaction
                .execute(
                    "INSERT INTO candidates (track_id, rank, score, title_ratio, flags, username, \
                     remote_path, size, bitrate, sample_rate, bit_depth, is_vbr, free_slots, \
                     queue_length, duration_seconds) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        track_id,
                        i64::try_from(index + 1).unwrap_or(i64::MAX),
                        scored.score,
                        scored.title_ratio,
                        serde_json::to_string(&scored.flags).unwrap_or_else(|_| "[]".to_owned()),
                        candidate.username,
                        candidate.remote_path,
                        i64::try_from(candidate.size).unwrap_or(i64::MAX),
                        candidate.bitrate,
                        candidate.sample_rate,
                        candidate.bit_depth,
                        i64::from(candidate.is_vbr),
                        i64::from(candidate.free_upload_slots),
                        candidate.queue_length,
                        candidate.duration_seconds,
                    ],
                )
                .map_err(|error| sqlite_error(&error))?;
        }

        let (status, detail) = if candidates.is_empty() {
            (TrackStatus::NoCandidates, "no usable search results")
        } else {
            (TrackStatus::AwaitingReview, "")
        };
        transaction
            .execute(
                "UPDATE tracks SET status = ?2, detail = ?3 WHERE id = ?1",
                params![track_id, status.as_str(), detail],
            )
            .map_err(|error| sqlite_error(&error))?;

        transaction.commit().map_err(|error| sqlite_error(&error))
    }

    /// Choose one candidate, clearing any previous choice.
    pub fn approve(&mut self, track_id: i64, rank: i64) -> Result<()> {
        self.approve_all(&[(track_id, rank)])
    }

    /// Choose a candidate for several tracks as one decision, or for none (#6).
    ///
    /// What an album offer is approved through: a folder covers many tracks and
    /// is taken or left as a unit, so a rank that turns out not to exist has to
    /// leave the whole offer undecided rather than half of it. Returning early
    /// drops the transaction, which rolls it back -- the same reason a bad rank
    /// has always left the previous choice standing.
    pub fn approve_all(&mut self, picks: &[(i64, i64)]) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| sqlite_error(&error))?;
        for (track_id, rank) in picks {
            approve_within(&transaction, *track_id, *rank)?;
        }
        transaction.commit().map_err(|error| sqlite_error(&error))
    }

    pub fn reject(&mut self, track_id: i64, reason: &str) -> Result<()> {
        self.connection
            .execute(
                "UPDATE candidates SET approved = 0 WHERE track_id = ?1",
                params![track_id],
            )
            .map_err(|error| sqlite_error(&error))?;
        self.set_status(track_id, TrackStatus::Rejected, reason)
    }

    /// Take back a decision nothing has acted on yet (#8).
    ///
    /// What the picker's undo is: a choice made a keystroke ago and regretted.
    /// The `attempted` mark goes with the approval, because [`Self::approve`]
    /// sets it so the retry in [`Self::next_candidate`] never offers a tried
    /// candidate twice -- and a candidate nothing has queued was never tried.
    /// Only sound for a track no download has started for, which is the only
    /// kind the picker walks.
    pub fn undecide(&mut self, track_id: i64) -> Result<()> {
        self.connection
            .execute(
                "UPDATE candidates SET approved = 0, attempted = 0 WHERE track_id = ?1",
                params![track_id],
            )
            .map_err(|error| sqlite_error(&error))?;
        self.set_status(track_id, TrackStatus::AwaitingReview, "")
    }

    pub fn mark(&mut self, track_id: i64, status: TrackStatus, detail: &str) -> Result<()> {
        self.set_status(track_id, status, detail)
    }

    /// Return a track to the queue so it is searched again.
    ///
    /// Destructive: it throws away a completed search. [`Self::next_candidate`]
    /// is the non-destructive counterpart.
    pub fn reset(&mut self, track_id: i64) -> Result<()> {
        self.connection
            .execute(
                "DELETE FROM candidates WHERE track_id = ?1",
                params![track_id],
            )
            .map_err(|error| sqlite_error(&error))?;
        self.set_status(track_id, TrackStatus::Pending, "")
    }

    /// Promote the best candidate nobody has tried yet (#2).
    ///
    /// Returns the rank now approved, or `None` when every candidate has been
    /// tried -- which is the only point at which a track becomes `FAILED`.
    /// Unlike [`Self::reset`] this keeps the search results, so a stalled
    /// transfer costs a re-queue rather than a re-search.
    pub fn next_candidate(&mut self, track_id: i64) -> Result<Option<i64>> {
        if self.track(track_id)?.is_none() {
            return Ok(None);
        }
        // The best candidate nobody has tried yet, rather than the one after the
        // current choice: an album approval takes a rank of its own choosing, so
        // walking upward from it would strand everything above (#6).
        let next: Option<i64> = self
            .connection
            .query_row(
                "SELECT rank FROM candidates WHERE track_id = ?1 AND attempted = 0                  ORDER BY rank LIMIT 1",
                params![track_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sqlite_error(&error))?;

        let Some(next) = next else {
            self.set_status(track_id, TrackStatus::Failed, "no candidates left to try")?;
            return Ok(None);
        };

        self.approve(track_id, next)?;
        self.connection
            .execute(
                "UPDATE tracks SET attempts = attempts + 1 WHERE id = ?1",
                params![track_id],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(Some(next))
    }

    /// Mark a track queued, recording when, so a stall becomes visible (#2).
    ///
    /// Nothing recorded *when* a track entered `QUEUED` before, so a transfer
    /// sitting for six hours was indistinguishable from one queued a minute ago.
    pub fn mark_queued(&mut self, track_id: i64, at: f64, detail: &str) -> Result<()> {
        self.connection
            .execute(
                "UPDATE tracks SET status = ?2, detail = ?3, queued_at = ?4 WHERE id = ?1",
                params![track_id, TrackStatus::Queued.as_str(), detail, at],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(())
    }

    /// Tracks queued longer than `stall_after_seconds` (#2).
    pub fn stalled(&self, now: f64, stall_after_seconds: f64) -> Result<Vec<ReviewTrack>> {
        Ok(self
            .tracks(&[TrackStatus::Queued], None)?
            .into_iter()
            .filter(|entry| {
                entry
                    .queued_at
                    .is_some_and(|queued| now - queued > stall_after_seconds)
            })
            .collect())
    }

    fn set_status(&mut self, track_id: i64, status: TrackStatus, detail: &str) -> Result<()> {
        self.connection
            .execute(
                "UPDATE tracks SET status = ?2, detail = ?3 WHERE id = ?1",
                params![track_id, status.as_str(), detail],
            )
            .map_err(|error| sqlite_error(&error))?;
        Ok(())
    }

    fn hydrate(&self, row: PartialTrack) -> Result<ReviewTrack> {
        let rows: Vec<(i64, i64, ScoredCandidate)> = self
            .connection
            .prepare("SELECT * FROM candidates WHERE track_id = ?1 ORDER BY rank")
            .and_then(|mut statement| {
                statement
                    .query_map(params![row.id], |row| {
                        Ok((
                            row.get::<_, i64>("rank")?,
                            row.get::<_, i64>("approved")?,
                            to_scored(row)?,
                        ))
                    })?
                    .collect()
            })
            .map_err(|error| sqlite_error(&error))?;

        let approved_rank = rows
            .iter()
            .find(|(_, approved, _)| *approved != 0)
            .map(|(rank, _, _)| *rank);

        Ok(ReviewTrack {
            id: row.id,
            track: row.track,
            status: row.status,
            detail: row.detail,
            candidates: rows.into_iter().map(|(_, _, scored)| scored).collect(),
            approved_rank,
            queued_at: row.queued_at,
            attempts: row.attempts,
        })
    }
}

/// Add any of `columns` the table does not already carry.
///
/// Nullable or defaulted throughout, and applied with `ALTER TABLE` at open
/// time, so a database an earlier build wrote is upgraded where it stands
/// rather than rewritten.
fn add_missing_columns(
    connection: &Connection,
    table: &str,
    columns: &[(&str, &str)],
) -> Result<()> {
    let existing: HashSet<String> = connection
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect()
        })
        .map_err(|error| sqlite_error(&error))?;

    for (name, kind) in columns {
        if !existing.contains(*name) {
            connection
                .execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {name} {kind}"))
                .map_err(|error| sqlite_error(&error))?;
        }
    }
    Ok(())
}

/// Select one candidate for one track, inside a transaction the caller owns.
///
/// Choosing a candidate also records it as tried, so the retry in
/// [`ReviewStore::next_candidate`] never offers it back a second time.
fn approve_within(transaction: &Transaction<'_>, track_id: i64, rank: i64) -> Result<()> {
    transaction
        .execute(
            "UPDATE candidates SET approved = 0 WHERE track_id = ?1",
            params![track_id],
        )
        .map_err(|error| sqlite_error(&error))?;
    let updated = transaction
        .execute(
            "UPDATE candidates SET approved = 1, attempted = 1 WHERE track_id = ?1 AND rank = ?2",
            params![track_id, rank],
        )
        .map_err(|error| sqlite_error(&error))?;

    if updated == 0 {
        return Err(Error::Config(format!(
            "track {track_id} has no candidate {rank}"
        )));
    }

    transaction
        .execute(
            "UPDATE tracks SET status = ?2, detail = ?3 WHERE id = ?1",
            params![
                track_id,
                TrackStatus::Approved.as_str(),
                format!("candidate {rank}")
            ],
        )
        .map_err(|error| sqlite_error(&error))?;
    Ok(())
}

struct PartialTrack {
    id: i64,
    track: Track,
    status: TrackStatus,
    detail: String,
    queued_at: Option<f64>,
    attempts: i64,
}

fn partial_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<PartialTrack> {
    let source_row: String = row.get("source_row")?;
    let status: String = row.get("status")?;
    Ok(PartialTrack {
        id: row.get("id")?,
        track: Track {
            artist: row.get("artist")?,
            title: row.get("title")?,
            album: row.get("album")?,
            duration_seconds: row.get("duration_seconds")?,
            title_guessed: row.get::<_, Option<i64>>("title_guessed")?.unwrap_or(0) != 0,
            source_row: parse_source_row(&source_row),
        },
        status: status.parse().unwrap_or(TrackStatus::Pending),
        detail: row.get("detail")?,
        queued_at: row.get("queued_at")?,
        attempts: row.get::<_, Option<i64>>("attempts")?.unwrap_or(0),
    })
}

fn to_scored(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScoredCandidate> {
    let flags: String = row.get("flags")?;
    Ok(ScoredCandidate {
        candidate: Candidate {
            username: row.get("username")?,
            remote_path: row.get("remote_path")?,
            size: u64::try_from(row.get::<_, i64>("size")?).unwrap_or(0),
            bitrate: row.get("bitrate")?,
            sample_rate: row.get("sample_rate")?,
            bit_depth: row.get("bit_depth")?,
            is_vbr: row.get::<_, i64>("is_vbr")? != 0,
            free_upload_slots: row.get::<_, i64>("free_slots")? != 0,
            queue_length: row.get("queue_length")?,
            duration_seconds: row.get("duration_seconds")?,
        },
        score: row.get("score")?,
        title_ratio: row.get("title_ratio")?,
        flags: serde_json::from_str(&flags).unwrap_or_default(),
    })
}

/// `source_row` is a JSON object in the database; the Python stores a dict and
/// this keeps the insertion order the export had.
fn source_row_json(source_row: &[(String, String)]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = source_row
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect();
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_owned())
}

fn parse_source_row(raw: &str) -> Vec<(String, String)> {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw)
        .map(|map| {
            map.into_iter()
                .map(|(key, value)| {
                    let value = match value {
                        serde_json::Value::String(text) => text,
                        other => other.to_string(),
                    };
                    (key, value)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // The column is only ever read back as an opaque timestamp, so seconds since
    // the epoch rendered in RFC 3339 keeps it sortable without a date crate.
    let seconds = now.as_secs();
    let (days, rest) = (seconds / 86400, seconds % 86400);
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant's `civil_from_days`, so a timestamp needs no date crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_prime + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    })
    .unwrap_or(1);
    (year + i64::from(month <= 2), month, day)
}

fn sqlite_error(error: &rusqlite::Error) -> Error {
    Error::Config(error.to_string())
}
