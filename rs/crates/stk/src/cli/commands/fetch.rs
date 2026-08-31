//! `stk fetch` -- search Soulseek, queue approved picks, and check what arrives.

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::config::SlskdSettings;
use crate::errors::Result;
use crate::fetch;
use crate::models::TrackStatus;

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

fn client(settings: &SlskdSettings) -> Result<SlskdClient<HttpTransport>> {
    Ok(SlskdClient::new(
        settings.clone(),
        HttpTransport::new(settings)?,
    ))
}

fn search(context: &AppContext, limit: Option<usize>) -> Result<i32> {
    let client = client(&context.settings().slskd)?;
    let mut store = context.open_review()?;

    if store.tracks(&[TrackStatus::Pending], limit)?.is_empty() {
        context.say("Nothing waiting to be searched.");
        return Ok(0);
    }

    let outcomes = fetch::search_pending(&mut store, &client, context.settings(), limit)?;

    if context.as_json {
        context.emit(&json!(outcomes
            .iter()
            .map(|outcome| json!({
                "track_id": outcome.track_id,
                "query": outcome.query,
                "found": outcome.found,
                "error": outcome.error,
            }))
            .collect::<Vec<_>>()));
        return Ok(0);
    }

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

    let with_candidates = outcomes.iter().filter(|outcome| outcome.found > 0).count();
    context.say(&format!(
        "Searched {} track(s); {with_candidates} have candidates to review. \
         Run stk review list to choose.",
        outcomes.len()
    ));
    Ok(0)
}

fn download(context: &AppContext, limit: Option<usize>, collect_only: bool) -> Result<i32> {
    let client = client(&context.settings().slskd)?;
    let mut store = context.open_review()?;
    let now = unix_now();

    // Before queueing, so a promoted candidate goes out in this same run.
    // `--collect` queues nothing new, stalls included.
    let (stalled, queued) = if collect_only {
        (Vec::new(), Vec::new())
    } else {
        (
            fetch::retry_stalled(&mut store, &client, context.settings(), now)?,
            fetch::enqueue_approved(&mut store, &client, limit, now)?,
        )
    };
    let collected = fetch::collect(&mut store, &client, context.settings(), true)?;

    if context.as_json {
        context.emit(&json!({
            "stalled": stalled.iter().map(stall_row).collect::<Vec<_>>(),
            "queued": queued.iter().map(enqueue_row).collect::<Vec<_>>(),
            "collected": collected.iter().map(collect_row).collect::<Vec<_>>(),
        }));
        return Ok(0);
    }

    report_stalled(context, &stalled);
    report_queued(context, &queued);
    report_collected(context, &collected);

    context.say(&format!(
        "Queued {} track(s); recorded {} finished transfer(s);          gave up on {} stalled one(s)",
        queued.iter().filter(|outcome| outcome.queued).count(),
        collected.len(),
        stalled.len()
    ));
    Ok(0)
}

fn stall_row(outcome: &fetch::StallOutcome) -> serde_json::Value {
    json!({
        "track_id": outcome.track_id,
        "query": outcome.query,
        "waited_seconds": outcome.waited_seconds,
        "retried_with_rank": outcome.retried_with_rank,
        "error": outcome.error,
    })
}

fn enqueue_row(outcome: &fetch::EnqueueOutcome) -> serde_json::Value {
    json!({
        "track_id": outcome.track_id,
        "query": outcome.query,
        "destination": outcome.destination,
        "ok": outcome.queued,
        "error": outcome.error,
    })
}

fn collect_row(outcome: &fetch::CollectOutcome) -> serde_json::Value {
    json!({
        "track_id": outcome.track_id,
        "query": outcome.query,
        "state": outcome.state.to_string(),
        "local_path": outcome.local_path.as_deref().map(show),
        "verdict": outcome
            .verdict
            .as_ref()
            .map(|verdict| verdict.verdict.to_string()),
        "error": outcome.error,
        "retried_with_rank": outcome.retried_with_rank,
    })
}

fn report_stalled(context: &AppContext, stalled: &[fetch::StallOutcome]) {
    if stalled.is_empty() {
        return;
    }
    let rows: Vec<Vec<String>> = stalled
        .iter()
        .map(|outcome| {
            let note = match outcome.retried_with_rank {
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

fn report_queued(context: &AppContext, queued: &[fetch::EnqueueOutcome]) {
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

fn report_collected(context: &AppContext, collected: &[fetch::CollectOutcome]) {
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
            } else {
                outcome.error.clone()
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
