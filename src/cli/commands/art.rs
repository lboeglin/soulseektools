//! `stk art` -- audit and repair embedded cover art.

use clap::Subcommand;
use serde_json::json;

use crate::artwork::base::ArtProvider;
use crate::artwork::service::{self, ArtworkResult};
use crate::cli::context::{show, table, unix_now, AppContext};
use crate::config::ArtworkSettings;
use crate::errors::Result;
use crate::library::changes::ChangeLog;
use crate::library::index::LibraryIndex;
use crate::library::tags;
use crate::models::LibraryEntry;
use crate::services::HttpTransport;

#[derive(Subcommand, Debug)]
pub enum ArtCommand {
    /// List files with no cover, or with one too small to be a real image.
    Check,
    /// Look up artwork for files that need it, and embed the verified matches.
    Fetch {
        /// Write the artwork. Without this, nothing is modified.
        #[arg(long)]
        apply: bool,
        /// Look up at most this many files.
        #[arg(long)]
        limit: Option<usize>,
        /// Accept a result even when its artist does not match. Not recommended.
        #[arg(long)]
        allow_unverified: bool,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: ArtCommand) -> Result<i32> {
    match command {
        ArtCommand::Check => check(context),
        ArtCommand::Fetch {
            apply,
            limit,
            allow_unverified,
        } => fetch(context, apply, limit, allow_unverified),
    }
}

fn check(context: &AppContext) -> Result<i32> {
    let entries = context.open_index()?.entries()?;
    if entries.is_empty() && context.nothing_to_do("Index is empty. Run stk library scan first.") {
        return Ok(0);
    }

    let statuses = service::audit(&entries, &context.settings().artwork);
    let missing: Vec<_> = statuses
        .iter()
        .filter(|status| status.needs_artwork)
        .collect();

    if context.as_json {
        context.emit(&json!(missing
            .iter()
            .map(|status| json!({
                "path": show(&status.entry.path),
                "reason": status.reason,
            }))
            .collect::<Vec<_>>()));
        return Ok(0);
    }

    if !missing.is_empty() {
        let rows: Vec<Vec<String>> = missing
            .iter()
            .map(|status| vec![show(&status.entry.path), status.reason.clone()])
            .collect();
        context.say(&table(&["File", "Problem"], &rows));
    }
    context.say(&format!(
        "{} of {} file(s) need artwork",
        missing.len(),
        entries.len()
    ));
    Ok(0)
}

fn fetch(
    context: &AppContext,
    apply: bool,
    limit: Option<usize>,
    allow_unverified: bool,
) -> Result<i32> {
    let mut settings = context.settings().artwork.clone();
    if allow_unverified {
        settings.require_artist_match = false;
    }

    let providers = match service::build_providers(&settings) {
        Ok(providers) => providers,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(1);
        }
    };

    let mut index = context.open_index()?;
    let entries = index.entries()?;

    let mut needing = needing_artwork(&entries, &settings);
    if let Some(limit) = limit {
        needing.truncate(limit);
    }
    if needing.is_empty() && context.nothing_to_do("Every indexed file already has usable artwork.")
    {
        return Ok(0);
    }

    let results = fetch_covers(context, &mut index, &needing, &providers, &settings, apply)?;
    report(context, &results, apply);
    Ok(0)
}

pub(crate) fn needing_artwork(
    entries: &[LibraryEntry],
    settings: &ArtworkSettings,
) -> Vec<LibraryEntry> {
    service::audit(entries, settings)
        .into_iter()
        .filter(|status| status.needs_artwork)
        .map(|status| status.entry)
        .collect()
}

pub(crate) fn fetch_covers(
    context: &AppContext,
    index: &mut LibraryIndex,
    needing: &[LibraryEntry],
    providers: &[Box<dyn ArtProvider>],
    settings: &ArtworkSettings,
    apply: bool,
) -> Result<Vec<ArtworkResult>> {
    let roots = context.library_roots();
    let http = HttpTransport::new(settings.request_timeout_seconds, &settings.contact)?;
    let log = ChangeLog::open(index, "art fetch", unix_now())?;
    let mut results = Vec::new();
    for entry in needing {
        let mut result = service::find_artwork(entry, providers, settings, &http);
        if result.ok() && apply {
            // Read before embedding: afterwards the cover this replaced is gone.
            let previous = tags::read_artwork(&entry.path).ok().flatten();
            if let Err(error) = service::embed(&result) {
                result.error = error.to_string();
            } else {
                log.artwork(
                    index,
                    &entry.path,
                    previous.as_ref(),
                    result.artwork.as_ref(),
                )?;
                index.reindex(&entry.path, &roots)?;
            }
        }
        results.push(result);
    }
    Ok(results)
}

pub(crate) fn artwork_row(result: &ArtworkResult, applied: bool) -> serde_json::Value {
    json!({
        "path": show(&result.entry.path),
        "ok": result.ok(),
        "reason": if result.error.is_empty() {
            result.selection.reason.clone()
        } else {
            result.error.clone()
        },
        "url": result
            .selection
            .candidate
            .as_ref()
            .map_or_else(String::new, |candidate| candidate.url.clone()),
        "bytes": result.artwork.as_ref().map_or(0, |artwork| artwork.data.len()),
        "applied": applied && result.ok(),
    })
}

pub(crate) fn report(context: &AppContext, results: &[ArtworkResult], applied: bool) {
    if context.as_json {
        context.emit(&json!(results
            .iter()
            .map(|result| artwork_row(result, applied))
            .collect::<Vec<_>>()));
        return;
    }

    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|result| {
            let marker = if result.ok() { "OK" } else { "--" };
            let outcome = if result.error.is_empty() {
                result.selection.reason.clone()
            } else {
                result.error.clone()
            };
            vec![
                marker.to_owned(),
                result
                    .entry
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                outcome,
            ]
        })
        .collect();
    context.say(&table(&["", "File", "Outcome"], &rows));

    let found = results.iter().filter(|result| result.ok()).count();
    if applied {
        context.say(&format!(
            "Embedded artwork in {found} of {} file(s)",
            results.len()
        ));
    } else {
        context.say(&format!(
            "Found artwork for {found} of {} file(s). \
             Nothing was written -- pass --apply to embed it.",
            results.len()
        ));
    }
}
