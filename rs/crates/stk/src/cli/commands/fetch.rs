//! `stk fetch` -- search Soulseek, queue approved picks, and check what arrives.

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, AppContext};
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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    let queued = if collect_only {
        Vec::new()
    } else {
        fetch::enqueue_approved(&mut store, &client, limit, now)?
    };
    let collected = fetch::collect(&mut store, &client, context.settings(), true)?;

    if context.as_json {
        context.emit(&json!({
            "queued": queued
                .iter()
                .map(|outcome| json!({
                    "track_id": outcome.track_id,
                    "query": outcome.query,
                    "destination": outcome.destination,
                    "ok": outcome.queued,
                    "error": outcome.error,
                }))
                .collect::<Vec<_>>(),
            "collected": collected
                .iter()
                .map(|outcome| json!({
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
                }))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }

    if !queued.is_empty() {
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

    if !collected.is_empty() {
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

    context.say(&format!(
        "Queued {} track(s); recorded {} finished transfer(s)",
        queued.iter().filter(|outcome| outcome.queued).count(),
        collected.len()
    ));
    Ok(0)
}
