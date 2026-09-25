//! `stk review`: inspect and decide on the candidates found for each track.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, AppContext};
use crate::errors::Result;
use crate::models::TrackStatus;
use crate::review::albums::{self, AlbumOffer};
use crate::review::legacy_csv::import_csv;
use crate::review::picker;
use crate::review::store::{ReviewStore, ReviewTrack};

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
    /// Walk the tracks awaiting review, choosing a candidate for each.
    Pick,
    /// Show the folders one peer could supply several waiting tracks from.
    Albums {
        /// Show at most this many.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// List the tracks each folder covers.
        #[arg(long)]
        tracks: bool,
    },
    /// Take every track one peer's folder covers, as a single decision.
    ApproveAlbum {
        /// Which album, as `stk review albums` numbers them.
        index: usize,
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
        ReviewCommand::Pick => pick(context),
        ReviewCommand::Albums { limit, tracks } => list_albums(context, limit, tracks),
        ReviewCommand::ApproveAlbum { index } => approve_album(context, index),
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
                    "title_guessed": entry.track.title_guessed,
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
            let guessed = if entry.track.title_guessed {
                " (guessed)"
            } else {
                ""
            };
            vec![
                entry.id.to_string(),
                entry.status.to_string(),
                format!("{} - {}{guessed}", entry.track.artist, entry.track.title),
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

/// Refused under `--json`, where the prompt would be invisible.
fn pick(context: &AppContext) -> Result<i32> {
    if context.as_json {
        eprintln!("error: stk review pick asks a question per track and cannot run with --json");
        eprintln!("hint: stk --json review list --candidates, then stk review approve <id> <rank>");
        return Ok(1);
    }

    let mut store = context.open_review()?;
    let tally = picker::walk(
        &mut store,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )?;

    if tally.waiting == 0 {
        context.say("Nothing in the review queue.");
        return Ok(0);
    }

    let left = tally.waiting - tally.approved - tally.rejected;
    let passed = if tally.skipped > 0 {
        format!(" ({} passed over)", tally.skipped)
    } else {
        String::new()
    };
    context.say(&format!(
        "\n{} approved, {} rejected, {left} left to review{passed}.",
        tally.approved, tally.rejected
    ));
    if tally.approved > 0 {
        context.say("Run stk fetch download to queue them.");
    }
    Ok(0)
}

/// Shared by `albums` and `approve-album`, so an index from one works in the other.
fn album_offers(context: &AppContext, store: &ReviewStore) -> Result<Vec<AlbumOffer>> {
    let tracks = store.tracks(&[TrackStatus::AwaitingReview], None)?;
    Ok(albums::offers(
        &tracks,
        context.settings().review.album_minimum_tracks,
    ))
}

fn list_albums(context: &AppContext, limit: usize, with_tracks: bool) -> Result<i32> {
    let store = context.open_review()?;
    let offers = album_offers(context, &store)?;
    let shown = || offers.iter().take(limit).enumerate();

    if context.as_json {
        context.emit(&json!(shown()
            .map(|(index, offer)| json!({
                "index": index + 1,
                "username": offer.username,
                "folder": offer.folder,
                "album": offer.name(),
                "covered": offer.covered(),
                "sacrificed": offer.sacrificed,
                "tracks": offer
                    .tracks
                    .iter()
                    .map(|track| json!({
                        "track_id": track.track_id,
                        "query": track.query,
                        "rank": track.rank,
                        "score": track.scored.score,
                        "remote_path": track.scored.candidate.remote_path,
                    }))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()));
        return Ok(0);
    }

    if offers.is_empty() {
        context.say("No folder in the queue covers enough tracks to be an album.");
        return Ok(0);
    }

    let rows: Vec<Vec<String>> = shown()
        .map(|(index, offer)| {
            vec![
                (index + 1).to_string(),
                offer.username.clone(),
                offer.name().to_owned(),
                offer.covered().to_string(),
                format!("{} of {}", offer.top_picks(), offer.covered()),
                offer.quality(),
            ]
        })
        .collect();
    context.say(&table(
        &["#", "Peer", "Album", "Tracks", "Top pick", "Quality"],
        &rows,
    ));

    if with_tracks {
        for (index, offer) in shown() {
            context.say(&format!("\n{}. {}", index + 1, offer.folder));
            for track in &offer.tracks {
                context.say(&format!(
                    "  {} -- candidate {} of track {}",
                    track.query, track.rank, track.track_id
                ));
                context.say(&format!("    {}", track.scored.describe()));
            }
        }
    }

    let mut footer = vec!["Take one with stk review approve-album <#>.".to_owned()];
    if offers.len() > limit {
        footer.insert(
            0,
            format!(
                "Showing {limit} of {} offer(s); pass --limit to see the rest.",
                offers.len()
            ),
        );
    }
    context.say(&format!("\n{}", footer.join("\n")));
    Ok(0)
}

fn approve_album(context: &AppContext, index: usize) -> Result<i32> {
    let mut store = context.open_review()?;
    let offers = album_offers(context, &store)?;
    let Some(offer) = index
        .checked_sub(1)
        .and_then(|position| offers.get(position))
    else {
        eprintln!("error: no album {index} in the review queue");
        return Ok(1);
    };

    store.approve_all(&offer.picks())?;
    if context.as_json {
        context.emit(&json!({
            "username": offer.username,
            "album": offer.name(),
            "approved": offer
                .picks()
                .iter()
                .map(|(track_id, rank)| json!({"id": track_id, "approved_rank": rank}))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }
    context.say(&format!(
        "Approved {} track(s) from {}'s \"{}\"",
        offer.covered(),
        offer.username,
        offer.name()
    ));
    // Decided tracks leave the listing, so indexes read before this run are stale.
    if offers.len() > 1 {
        context.say("The remaining offers are renumbered; list them again before taking another.");
    }
    Ok(0)
}

fn approve(context: &AppContext, track_id: i64, rank: i64) -> Result<i32> {
    let mut store = context.open_review()?;
    if let Err(error) = store.approve(track_id, rank) {
        eprintln!("error: {error}");
        return Ok(1);
    }

    if let Some(entry) = store.track(track_id)? {
        if context.as_json {
            context.emit(&decided_row(&entry));
            return Ok(0);
        }
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
    report_decided(
        context,
        &store,
        track_id,
        &format!("Rejected track {track_id}"),
    )
}

fn reset(context: &AppContext, track_id: i64) -> Result<i32> {
    let mut store = context.open_review()?;
    if store.track(track_id)?.is_none() {
        eprintln!("error: no track {track_id} in the review queue");
        return Ok(1);
    }
    store.reset(track_id)?;
    report_decided(
        context,
        &store,
        track_id,
        &format!("Track {track_id} will be searched again"),
    )
}

fn report_decided(
    context: &AppContext,
    store: &ReviewStore,
    track_id: i64,
    line: &str,
) -> Result<i32> {
    if context.as_json {
        if let Some(entry) = store.track(track_id)? {
            context.emit(&decided_row(&entry));
        }
    } else {
        context.say(line);
    }
    Ok(0)
}

fn decided_row(entry: &ReviewTrack) -> serde_json::Value {
    json!({
        "id": entry.id,
        "artist": entry.track.artist,
        "title": entry.track.title,
        "status": entry.status.to_string(),
        "detail": entry.detail,
        "approved_rank": entry.approved_rank,
    })
}

fn import(context: &AppContext, path: &std::path::Path) -> Result<i32> {
    if !path.is_file() {
        eprintln!("error: no such file: {}", show(path));
        return Ok(1);
    }

    let mut store = context.open_review()?;
    let result = import_csv(path, &mut store)?;
    if context.as_json {
        context.emit(&json!({"added": result.added, "approvals": result.approvals}));
        return Ok(0);
    }
    context.say(&format!(
        "Imported {} new track(s), carrying over {} approval(s)",
        result.added, result.approvals
    ));
    Ok(0)
}
