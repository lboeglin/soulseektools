//! `stk fetch` -- search Soulseek, queue approved picks, and check what arrives.

use std::fmt::Write as _;

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::config::SlskdSettings;
use crate::errors::Result;
use crate::fetch;
use crate::models::TrackStatus;
use crate::retag::Retagged;

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
                "preferred_uploader": outcome.preferred_uploader,
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
         Run stk review pick to choose.",
        outcomes.len()
    ));
    Ok(0)
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
        (Vec::new(), Vec::new())
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
        "tags": outcome.tags.as_ref().map(tags_row),
        "error": outcome.error,
        "retried_with_rank": outcome.retried_with_rank,
    })
}

/// The same field triple `tags apply` reports, so a script reads both alike.
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
