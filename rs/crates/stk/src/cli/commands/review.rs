//! `stk review` -- inspect and decide on the candidates found for each track.
//!
//! The Python opens a Textual app when run bare. The TUI is deferred out of the
//! Rust v1 (#1), so the bare form points at the subcommands that cover the same
//! ground rather than pretending to have one.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, AppContext};
use crate::errors::Result;
use crate::models::TrackStatus;
use crate::review::legacy_csv::import_csv;

#[derive(Subcommand, Debug)]
pub enum ReviewCommand {
    /// Show what is in the review queue.
    List {
        /// Show only tracks in this state.
        #[arg(long)]
        status: Option<String>,
        /// Show at most this many.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Show each track's candidates too.
        #[arg(long)]
        candidates: bool,
    },
    /// Choose one candidate for a track, so the downloader can queue it.
    Approve {
        /// Track to approve.
        track_id: i64,
        /// Which candidate to take.
        #[arg(default_value_t = 1)]
        rank: i64,
    },
    /// Mark a track as not worth downloading from any candidate found.
    Reject {
        /// Track to reject.
        track_id: i64,
        /// Why, for the record.
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Discard a track's candidates and return it to the search queue.
    Reset {
        /// Track to search again.
        track_id: i64,
    },
    /// Load a review CSV written by the Nicotine+ plugin, approvals included.
    Import {
        /// A soulseek_review.csv from the old plugin.
        path: PathBuf,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: ReviewCommand) -> Result<i32> {
    match command {
        ReviewCommand::List {
            status,
            limit,
            candidates,
        } => list(context, status.as_deref(), limit, candidates),
        ReviewCommand::Approve { track_id, rank } => approve(context, track_id, rank),
        ReviewCommand::Reject { track_id, reason } => reject(context, track_id, &reason),
        ReviewCommand::Reset { track_id } => reset(context, track_id),
        ReviewCommand::Import { path } => import(context, &path),
    }
}

fn list(
    context: &AppContext,
    status: Option<&str>,
    limit: usize,
    with_candidates: bool,
) -> Result<i32> {
    let statuses: Vec<TrackStatus> = match status {
        Some(status) => match status.parse() {
            Ok(status) => vec![status],
            Err(message) => {
                eprintln!("error: {message}");
                return Ok(1);
            }
        },
        None => Vec::new(),
    };

    let store = context.open_review()?;
    let tracks = store.tracks(&statuses, Some(limit))?;
    let counts = store.counts()?;

    if context.as_json {
        let mut count_table = serde_json::Map::new();
        for (status, count) in &counts {
            count_table.insert(status.to_string(), json!(count));
        }
        context.emit(&json!({
            "counts": count_table,
            "tracks": tracks
                .iter()
                .map(|entry| json!({
                    "id": entry.id,
                    "artist": entry.track.artist,
                    "title": entry.track.title,
                    "status": entry.status.to_string(),
                    "detail": entry.detail,
                    "approved_rank": entry.approved_rank,
                    "candidates": entry
                        .candidates
                        .iter()
                        .enumerate()
                        .map(|(index, scored)| json!({
                            "rank": index + 1,
                            "score": scored.score,
                            "flags": scored.flags,
                            "username": scored.candidate.username,
                            "remote_path": scored.candidate.remote_path,
                        }))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }

    if tracks.is_empty() {
        context.say("Nothing in the review queue.");
        return Ok(0);
    }

    let rows: Vec<Vec<String>> = tracks
        .iter()
        .map(|entry| {
            vec![
                entry.id.to_string(),
                entry.status.to_string(),
                format!("{} - {}", entry.track.artist, entry.track.title),
                entry.detail.clone(),
            ]
        })
        .collect();
    context.say(&table(&["ID", "Status", "Track", "Detail"], &rows));

    if with_candidates {
        for entry in &tracks {
            if entry.candidates.is_empty() {
                continue;
            }
            context.say(&format!(
                "\n{} {} - {}",
                entry.id, entry.track.artist, entry.track.title
            ));
            for (index, scored) in entry.candidates.iter().enumerate() {
                let rank = i64::try_from(index + 1).unwrap_or(0);
                let marker = if Some(rank) == entry.approved_rank {
                    "*"
                } else {
                    " "
                };
                context.say(&format!(" {marker}{rank}. {}", scored.describe()));
                context.say(&format!("    {}", scored.candidate.remote_path));
            }
        }
    }

    let summary: Vec<String> = counts
        .iter()
        .map(|(status, count)| format!("{count} {status}"))
        .collect();
    context.say(&format!("\n{}", summary.join(", ")));
    Ok(0)
}

fn approve(context: &AppContext, track_id: i64, rank: i64) -> Result<i32> {
    let mut store = context.open_review()?;
    if let Err(error) = store.approve(track_id, rank) {
        eprintln!("error: {error}");
        return Ok(1);
    }

    if let Some(entry) = store.track(track_id)? {
        if let Some(approved) = entry.approved() {
            context.say(&format!(
                "Approved candidate {rank}: {}",
                approved.describe()
            ));
        }
    }
    Ok(0)
}

fn reject(context: &AppContext, track_id: i64, reason: &str) -> Result<i32> {
    let mut store = context.open_review()?;
    if store.track(track_id)?.is_none() {
        eprintln!("error: no track {track_id} in the review queue");
        return Ok(1);
    }
    store.reject(track_id, reason)?;
    context.say(&format!("Rejected track {track_id}"));
    Ok(0)
}

fn reset(context: &AppContext, track_id: i64) -> Result<i32> {
    let mut store = context.open_review()?;
    if store.track(track_id)?.is_none() {
        eprintln!("error: no track {track_id} in the review queue");
        return Ok(1);
    }
    store.reset(track_id)?;
    context.say(&format!("Track {track_id} will be searched again"));
    Ok(0)
}

fn import(context: &AppContext, path: &std::path::Path) -> Result<i32> {
    if !path.is_file() {
        eprintln!("error: no such file: {}", show(path));
        return Ok(1);
    }

    let mut store = context.open_review()?;
    let result = import_csv(path, &mut store)?;
    context.say(&format!(
        "Imported {} new track(s), carrying over {} approval(s)",
        result.added, result.approvals
    ));
    Ok(0)
}
