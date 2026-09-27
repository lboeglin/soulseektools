//! `stk fetch` -- search Soulseek, queue approved picks, and check what arrives.

use std::fmt::Write as _;

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::config::SlskdSettings;
use crate::errors::{Error, Result};
use crate::fetch;
use crate::models::TrackStatus;
use crate::retag::Retagged;
use crate::soulseek::base::SoulseekClient as _;
use crate::soulseek::slskd::{HttpTransport, SlskdClient};

#[derive(Subcommand, Debug)]
pub enum FetchCommand {
    /// Search for every track waiting in the review queue and rank what comes back.
    Search {
        /// Search at most this many tracks.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Queue approved picks, then record and verify whatever has finished.
    Download {
        /// Queue at most this many tracks.
        #[arg(long)]
        limit: Option<usize>,
        /// Only check finished transfers; queue nothing new.
        #[arg(long = "collect")]
        collect_only: bool,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: FetchCommand) -> Result<i32> {
    match command {
        FetchCommand::Search { limit } => search(context, limit),
        FetchCommand::Download {
            limit,
            collect_only,
        } => download(context, limit, collect_only),
    }
}

/// Checked before any work: every request without a key is refused, and a
/// search against an slskd that is down would fail the whole queue.
pub(crate) fn client(settings: &SlskdSettings) -> Result<SlskdClient<HttpTransport>> {
    if settings.api_key.is_none() {
        return Err(Error::Config(
            "slskd needs an API key\n\
             hint: export SLSKD_API_KEY, the key in slskd's web.authentication.api_keys"
                .to_owned(),
        ));
    }
    let client = SlskdClient::new(settings.clone(), HttpTransport::new(settings)?);
    if !client.is_connected() {
        return Err(Error::Config(format!(
            "slskd at {} is not running, or not logged in to Soulseek\n\
             hint: start slskd, and check its web UI says it is connected",
            settings.url
        )));
    }
    Ok(client)
}

fn search(context: &AppContext, limit: Option<usize>) -> Result<i32> {
    let mut store = context.open_review()?;

    let idle = store.tracks(&[TrackStatus::Pending], limit)?.is_empty();
    if idle && context.nothing_to_do("Nothing waiting to be searched.") {
        return Ok(0);
    }
    let outcomes = if idle {
        Vec::new()
    } else {
        let client = client(&context.settings().slskd)?;
        fetch::search_pending(&mut store, &client, context.settings(), limit)?
    };
    let failed = outcomes.iter().any(|outcome| !outcome.error.is_empty());

    if context.as_json {
        context.emit(&json!(outcomes.iter().map(search_row).collect::<Vec<_>>()));
        return Ok(i32::from(failed));
    }

    report_searched(context, &outcomes);
    let with_candidates = outcomes.iter().filter(|outcome| outcome.found > 0).count();
    context.say(&format!(
        "Searched {} track(s); {with_candidates} have candidates to review. \
         Run stk review pick to choose.",
        outcomes.len()
    ));
    Ok(i32::from(failed))
}

fn download(context: &AppContext, limit: Option<usize>, collect_only: bool) -> Result<i32> {
    let client = client(&context.settings().slskd)?;
    let mut store = context.open_review()?;
    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let now = unix_now();

    // Before queueing, so a promoted candidate goes out in this same run.
    // `--collect` queues nothing new, stalls included.
    let (stalled, queued) = if collect_only {
        (fetch::Stalled::default(), Vec::new())
    } else {
        (
            fetch::retry_stalled(&mut store, &client, context.settings(), now)?,
            fetch::enqueue_approved(&mut store, &client, limit, now)?,
        )
    };
    let collected = fetch::collect(
        &mut store,
        &client,
        context.settings(),
        &mut index,
        &roots,
        now,
        true,
    )?;
    let failed = i32::from(fetch::downloads_went_wrong(&queued, &collected));

    if context.as_json {
        context.emit(&json!({
            "stalled": stalled.tracks.iter().map(stall_row).collect::<Vec<_>>(),
            "stalled_peers": stalled.peers.iter().map(peer_stall_row).collect::<Vec<_>>(),
            "queued": queued.iter().map(enqueue_row).collect::<Vec<_>>(),
            "collected": collected.iter().map(collect_row).collect::<Vec<_>>(),
        }));
        return Ok(failed);
    }

    report_stalled(context, &stalled.tracks, &stalled.peers);
    report_queued(context, &queued);
    report_collected(context, &collected);

    context.say(&format!(
        "Queued {} track(s); recorded {} finished transfer(s); cancelled {} stalled one(s)",
        queued.iter().filter(|outcome| outcome.queued).count(),
        collected.len(),
        stalled.tracks.len()
    ));
    Ok(failed)
}

pub(crate) fn search_row(outcome: &fetch::SearchOutcome) -> serde_json::Value {
    json!({
        "track_id": outcome.track_id,
        "query": outcome.query,
        "found": outcome.found,
        "error": outcome.error,
        "preferred_uploader": outcome.preferred_uploader,
    })
}

pub(crate) fn report_searched(context: &AppContext, outcomes: &[fetch::SearchOutcome]) {
    let rows: Vec<Vec<String>> = outcomes
        .iter()
        .map(|outcome| {
            let found = if outcome.error.is_empty() {
                outcome.found.to_string()
            } else {
                "--".to_owned()
            };
            vec![found, outcome.query.clone(), outcome.error.clone()]
        })
        .collect();
    context.say(&table(&["Found", "Track", "Note"], &rows));
}

pub(crate) fn stall_row(outcome: &fetch::StallOutcome) -> serde_json::Value {
    json!({
        "track_id": outcome.track_id,
        "query": outcome.query,
        "waited_seconds": outcome.waited_seconds,
        "retried_with_rank": outcome.retried_with_rank,
        "error": outcome.error,
        "peer_group": outcome.peer_group,
    })
}

pub(crate) fn peer_stall_row(outcome: &fetch::PeerStall) -> serde_json::Value {
    json!({
        "username": outcome.username,
        "moved_to": outcome.moved_to,
        "moved": outcome.moved,
        "individually": outcome.individually,
        "exhausted": outcome.exhausted,
    })
}

pub(crate) fn enqueue_row(outcome: &fetch::EnqueueOutcome) -> serde_json::Value {
    json!({
        "track_id": outcome.track_id,
        "query": outcome.query,
        "destination": outcome.destination,
        "ok": outcome.queued,
        "error": outcome.error,
    })
}

pub(crate) fn collect_row(outcome: &fetch::CollectOutcome) -> serde_json::Value {
    json!({
        "track_id": outcome.track_id,
        "query": outcome.query,
        "state": outcome.state.to_string(),
        "local_path": outcome.local_path.as_deref().map(show),
        "verdict": outcome
            .verdict
            .as_ref()
            .map(|verdict| verdict.verdict.to_string()),
        "tags": outcome.tags.as_ref().map(tags_row),
        "error": outcome.error,
        "retried_with_rank": outcome.retried_with_rank,
    })
}

/// The same fields `tags apply` reports, so a script reads both alike.
fn tags_row(retagged: &Retagged) -> serde_json::Value {
    json!({
        "policy": retagged.policy.as_str(),
        "fields": retagged
            .fields
            .iter()
            .map(|field| json!({
                "field": field.as_str(),
                "from": field.value(&retagged.before),
                "to": field.value(&retagged.after),
            }))
            .collect::<Vec<_>>(),
        "dropped": retagged.dropped,
        "error": retagged.error,
    })
}

fn describe_tags(retagged: &Retagged) -> String {
    if !retagged.error.is_empty() {
        return format!("tags: {}", retagged.error);
    }
    if !retagged.changed() {
        return String::new();
    }
    let names: Vec<&str> = retagged.fields.iter().map(|field| field.as_str()).collect();
    let mut note = if names.is_empty() {
        "tags kept".to_owned()
    } else {
        format!("tagged {}", names.join(", "))
    };
    if retagged.dropped > 0 {
        let _ = write!(note, " (+{} dropped)", retagged.dropped);
    }
    note
}

pub(crate) fn report_stalled(
    context: &AppContext,
    stalled: &[fetch::StallOutcome],
    peers: &[fetch::PeerStall],
) {
    for peer in peers {
        context.say(&describe_peer_stall(peer));
    }
    if stalled.is_empty() {
        return;
    }
    let rows: Vec<Vec<String>> = stalled
        .iter()
        .map(|outcome| {
            let note = match outcome.retried_with_rank {
                Some(rank) if !outcome.peer_group.is_empty() => format!(
                    "moved with {}'s group to candidate {rank}",
                    outcome.peer_group
                ),
                Some(rank) => format!("retrying with candidate {rank}"),
                None => "no candidates left".to_owned(),
            };
            vec![
                outcome.query.clone(),
                format!("{:.0}s", outcome.waited_seconds),
                note,
                outcome.error.clone(),
            ]
        })
        .collect();
    context.say(&table(&["Stalled", "Waited", "Note", "Error"], &rows));
}

fn describe_peer_stall(peer: &fetch::PeerStall) -> String {
    let mut line = format!("{} stalled", peer.username);
    if let Some(to) = peer.moved_to.as_deref().filter(|_| peer.moved > 0) {
        let _ = write!(line, "; {} track(s) moved to {to}", peer.moved);
    }
    if peer.individually > 0 {
        let _ = write!(line, "; {} fell back individually", peer.individually);
    }
    if peer.exhausted > 0 {
        let _ = write!(line, "; {} had nothing left to try", peer.exhausted);
    }
    line
}

pub(crate) fn report_queued(context: &AppContext, queued: &[fetch::EnqueueOutcome]) {
    if queued.is_empty() {
        return;
    }
    let rows: Vec<Vec<String>> = queued
        .iter()
        .map(|outcome| {
            let marker = if outcome.queued { "OK" } else { "--" };
            vec![
                marker.to_owned(),
                outcome.query.clone(),
                outcome.error.clone(),
            ]
        })
        .collect();
    context.say(&table(&["", "Track", "Note"], &rows));
}

pub(crate) fn report_collected(context: &AppContext, collected: &[fetch::CollectOutcome]) {
    if collected.is_empty() {
        return;
    }
    let rows: Vec<Vec<String>> = collected
        .iter()
        .map(|outcome| {
            let verdict = outcome
                .verdict
                .as_ref()
                .map_or_else(String::new, |verdict| verdict.verdict.to_string());
            let note = if let Some(rank) = outcome.retried_with_rank {
                format!("retrying with candidate {rank}")
            } else if !outcome.error.is_empty() {
                outcome.error.clone()
            } else {
                outcome.tags.as_ref().map(describe_tags).unwrap_or_default()
            };
            vec![
                outcome.state.to_string(),
                outcome.query.clone(),
                verdict,
                note,
            ]
        })
        .collect();
    context.say(&table(&["State", "Track", "Verdict", "Note"], &rows));
}
