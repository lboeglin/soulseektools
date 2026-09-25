//! `stk library` -- build and inspect the cached view of the library.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::Result;
use crate::library::index::RefreshResult;
use crate::library::stats::{self, LibrarySnapshot};

#[derive(Subcommand, Debug)]
pub enum LibraryCommand {
    /// Bring the index in line with what is on disk.
    Scan {
        /// Re-read every file, ignoring the cached size and mtime.
        #[arg(long)]
        full: bool,
    },
    /// Summarise what the index currently holds.
    Stats {
        /// How many artists to list.
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// Show how the library has changed over the scans recorded so far.
    Trend {
        /// How many measurements to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: LibraryCommand) -> Result<i32> {
    match command {
        LibraryCommand::Scan { full } => scan(context, full),
        LibraryCommand::Stats { top } => stats(context, top),
        LibraryCommand::Trend { limit } => trend(context, limit),
    }
}

pub(crate) struct Scanned {
    pub roots: Vec<PathBuf>,
    pub result: RefreshResult,
    pub total: usize,
}

impl Scanned {
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "roots": self.roots.iter().map(|root| show(root)).collect::<Vec<_>>(),
            "added": self.result.added,
            "updated": self.result.updated,
            "unchanged": self.result.unchanged,
            "removed": self.result.removed,
            "failed": self
                .result
                .failed
                .iter()
                .map(|(path, reason)| json!([show(path), reason]))
                .collect::<Vec<_>>(),
            "total": self.total,
        })
    }

    pub fn report(&self, context: &AppContext) {
        context.say(&format!(
            "Indexed {} file(s) under {}: {} added, {} updated, {} unchanged, {} removed",
            self.total,
            self.roots
                .iter()
                .map(|root| show(root))
                .collect::<Vec<_>>()
                .join(", "),
            self.result.added,
            self.result.updated,
            self.result.unchanged,
            self.result.removed
        ));
        for (path, reason) in &self.result.failed {
            context.say(&format!("skipped {}: {reason}", show(path)));
        }
    }
}

pub(crate) fn refresh(context: &AppContext, full: bool) -> Result<Scanned> {
    let roots = context.require_library_roots()?;
    let exclude = context.excludes()?;
    let extensions: Vec<&str> = context
        .settings()
        .library
        .extensions
        .iter()
        .map(String::as_str)
        .collect();

    let mut index = context.open_index()?;
    let result = index.refresh(&roots, &extensions, &exclude, full, None)?;
    let total = index.count()?;
    index.record_measurement(unix_now())?;
    Ok(Scanned {
        roots,
        result,
        total,
    })
}

fn scan(context: &AppContext, full: bool) -> Result<i32> {
    let scanned = refresh(context, full)?;
    if context.as_json {
        context.emit(&scanned.to_json());
    } else {
        scanned.report(context);
    }
    Ok(0)
}

fn stats(context: &AppContext, top: usize) -> Result<i32> {
    let index = context.open_index()?;
    let total = index.count()?;
    let by_artist = index.artist_counts()?;
    let entries = index.entries()?;

    let tagged = entries
        .iter()
        .filter(|entry| !entry.tags.title.is_empty())
        .count();
    let with_art = entries.iter().filter(|entry| entry.has_artwork()).count();
    let genres = entries
        .iter()
        .filter(|entry| !entry.tags.genres.is_empty())
        .count();
    let level_matched = entries
        .iter()
        .filter(|entry| entry.is_level_matched())
        .count();

    if context.as_json {
        let mut artists = serde_json::Map::new();
        for (artist, count) in &by_artist {
            artists.insert(artist.clone(), json!(count));
        }
        context.emit(&json!({
            "total": total,
            "tagged": tagged,
            "with_artwork": with_art,
            "with_genre": genres,
            "with_replay_gain": level_matched,
            "artists": artists,
        }));
        return Ok(0);
    }

    if total == 0 {
        context.say("Index is empty. Run stk library scan first.");
        return Ok(0);
    }

    context.say(&format!(
        "{total} file(s) across {} artist folder(s): {tagged} titled, {with_art} with artwork, \
         {genres} with a genre, {level_matched} level-matched",
        by_artist.len()
    ));

    let rows: Vec<Vec<String>> = by_artist
        .iter()
        .take(top)
        .map(|(artist, count)| {
            let name = if artist.is_empty() {
                "<library root>"
            } else {
                artist
            };
            vec![name.to_owned(), count.to_string()]
        })
        .collect();
    context.say(&format!(
        "Largest {} artist folders",
        top.min(by_artist.len())
    ));
    context.say(&table(&["Artist", "Files"], &rows));
    Ok(0)
}

fn trend(context: &AppContext, limit: usize) -> Result<i32> {
    let index = context.open_index()?;
    // Oldest first, which is the direction a trend is read in.
    let mut series = index.snapshots(Some(limit))?;
    series.reverse();

    if context.as_json {
        context.emit(&json!(series.iter().map(as_row).collect::<Vec<_>>()));
        return Ok(0);
    }

    if series.is_empty() {
        context.say("Nothing measured yet. Run stk library scan first.");
        return Ok(0);
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut previous: Option<&LibrarySnapshot> = None;
    for snapshot in &series {
        rows.push(vec![
            stats::date_of(snapshot.taken_at),
            against(snapshot.total, previous.map(|last| last.total)),
            against(snapshot.tagged, previous.map(|last| last.tagged)),
            against(
                snapshot.with_artwork,
                previous.map(|last| last.with_artwork),
            ),
            against(snapshot.with_genre, previous.map(|last| last.with_genre)),
            against(snapshot.lossless, previous.map(|last| last.lossless)),
            against(
                snapshot.verified_clean,
                previous.map(|last| last.verified_clean),
            ),
            against(snapshot.flagged, previous.map(|last| last.flagged)),
            optional(
                snapshot.with_replay_gain,
                previous.and_then(|last| last.with_replay_gain),
            ),
        ]);
        previous = Some(snapshot);
    }

    context.say(&table(
        &[
            "Measured", "Files", "Tagged", "Artwork", "Genre", "Lossless", "Verified", "Flagged",
            "Loudness",
        ],
        &rows,
    ));
    if let Some(latest) = series.last() {
        if latest.verified_clean + latest.flagged == 0 {
            context.say(
                "
No verdicts recorded yet -- run stk verify to fill the last two columns.",
            );
        }
    }
    Ok(0)
}

fn as_row(snapshot: &LibrarySnapshot) -> serde_json::Value {
    json!({
        "taken_at": snapshot.taken_at,
        "total": snapshot.total,
        "tagged": snapshot.tagged,
        "with_artwork": snapshot.with_artwork,
        "with_genre": snapshot.with_genre,
        "lossless": snapshot.lossless,
        "verified_clean": snapshot.verified_clean,
        "flagged": snapshot.flagged,
        "with_replay_gain": snapshot.with_replay_gain,
    })
}

/// Printed as `-` rather than zero when the measurement predates the column.
fn optional(now: Option<usize>, before: Option<usize>) -> String {
    now.map_or_else(|| "-".to_owned(), |now| against(now, before))
}

fn against(now: usize, before: Option<usize>) -> String {
    match before {
        Some(before) if before != now => {
            let sign = if now > before { "+" } else { "-" };
            format!("{now} ({sign}{})", now.abs_diff(before))
        }
        _ => now.to_string(),
    }
}
