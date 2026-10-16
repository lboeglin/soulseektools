//! `stk art` -- audit, repair and choose embedded cover art.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use crate::artwork::base::ArtProvider;
use crate::artwork::service::{self, ArtworkResult};
use crate::cli::context::{show, table, unix_now, AppContext};
use crate::config::ArtworkSettings;
use crate::errors::Result;
use crate::library::changes::{digest, ChangeLog};
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
    /// Embed a cover of your choosing, replacing the one there.
    Set {
        /// A file, or a folder: every file directly inside it.
        target: PathBuf,
        /// A JPEG or PNG: a local path or an http(s) URL.
        image: String,
        /// Write the artwork. Without this, nothing is modified.
        #[arg(long)]
        apply: bool,
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
        ArtCommand::Set {
            target,
            image,
            apply,
        } => set(context, &target, &image, apply),
    }
}

enum SetOutcome {
    Embedded,
    AlreadyThere,
    Failed(String),
}

fn set(context: &AppContext, target: &Path, image: &str, apply: bool) -> Result<i32> {
    let settings = &context.settings().artwork;
    let target = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let files = match service::cover_targets(&target) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(1);
        }
    };
    if files.is_empty() {
        eprintln!("error: {}: no file that can carry a cover", show(&target));
        return Ok(1);
    }

    let http = HttpTransport::new(settings.request_timeout_seconds, &settings.contact)?;
    let (artwork, warning) = match service::load_image(image, &http, settings) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(1);
        }
    };

    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let indexed: Vec<PathBuf> = index
        .entries()?
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    let log = ChangeLog::open(&index, "art set", unix_now())?;
    let wanted = digest(&artwork.data);
    let mut outcomes = Vec::new();
    for file in files {
        // The index's own spelling (`E:/Music\…` against `E:\Music\…`), so the
        // row is refreshed rather than doubled.
        let known = indexed.iter().find(|path| **path == file).cloned();
        let path = known.clone().unwrap_or(file);
        let outcome = match tags::read_artwork(&path) {
            Err(error) => SetOutcome::Failed(error.to_string()),
            Ok(previous)
                if previous
                    .as_ref()
                    .is_some_and(|cover| digest(&cover.data) == wanted) =>
            {
                SetOutcome::AlreadyThere
            }
            Ok(_) if !apply => SetOutcome::Embedded,
            Ok(previous) => {
                if let Err(error) = tags::write_artwork(&path, &artwork) {
                    SetOutcome::Failed(error.to_string())
                } else {
                    log.artwork(&mut index, &path, previous.as_ref(), Some(&artwork))?;
                    if known.is_some() {
                        index.reindex(&path, &roots)?;
                    }
                    SetOutcome::Embedded
                }
            }
        };
        outcomes.push((path, outcome));
    }

    report_set(
        context,
        image,
        artwork.data.len(),
        warning.as_deref(),
        &outcomes,
        apply,
    );
    Ok(0)
}

fn report_set(
    context: &AppContext,
    image: &str,
    bytes: usize,
    warning: Option<&str>,
    outcomes: &[(PathBuf, SetOutcome)],
    apply: bool,
) {
    if context.as_json {
        context.emit(&json!({
            "image": image,
            "bytes": bytes,
            "warning": warning,
            "applied": apply,
            "files": outcomes
                .iter()
                .map(|(path, outcome)| match outcome {
                    SetOutcome::Embedded => json!({"path": show(path), "ok": true}),
                    SetOutcome::AlreadyThere => {
                        json!({"path": show(path), "ok": true, "skipped": "already this cover"})
                    }
                    SetOutcome::Failed(error) => {
                        json!({"path": show(path), "ok": false, "error": error})
                    }
                })
                .collect::<Vec<_>>(),
        }));
        return;
    }

    if let Some(warning) = warning {
        context.say(&format!("warning: {warning}"));
    }
    let mut embedded = 0;
    for (path, outcome) in outcomes {
        match outcome {
            SetOutcome::Embedded => embedded += 1,
            SetOutcome::AlreadyThere => {
                context.say(&format!("skipped {}: already this cover", show(path)));
            }
            SetOutcome::Failed(error) => context.say_failure(&show(path), error),
        }
    }
    if apply {
        context.say(&format!(
            "Embedded the cover in {embedded} of {} file(s)",
            outcomes.len()
        ));
    } else {
        context.say(&format!(
            "{embedded} of {} file(s) would get this cover. \
             Nothing was written -- pass --apply to embed it.",
            outcomes.len()
        ));
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
                if let Some(year) = result.year.clone() {
                    match service::fill_year(index, &log, &entry.path, &year) {
                        Ok(true) => {}
                        Ok(false) => result.year = None,
                        Err(error) => {
                            result.year = None;
                            result.error =
                                format!("cover embedded, but the year was not written: {error}");
                        }
                    }
                }
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
        "year": result.year.as_ref().filter(|_| result.ok()),
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
            let outcome = match (&result.error, &result.year) {
                (error, _) if !error.is_empty() => error.clone(),
                (_, Some(year)) if result.ok() => {
                    format!("{}; year {year}", result.selection.reason)
                }
                _ => result.selection.reason.clone(),
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
