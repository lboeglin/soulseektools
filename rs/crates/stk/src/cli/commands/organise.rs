//! `stk organise` -- lay files out from what their tags say.

use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::library::changes::ChangeLog;
use crate::library::index::describe;
use crate::library::scan::{iter_audio_files, owning_root};
use crate::models::LibraryEntry;
use crate::organise::{self, Pattern, PlannedMove};

#[derive(Args, Debug)]
pub struct OrganiseArgs {
    /// Move the files. Without this, nothing is modified.
    #[arg(long)]
    apply: bool,

    /// Organise this directory instead of the indexed library.
    ///
    /// For the flat `<download dir>/<Artist>/<peer's filename>` layout a fetch
    /// leaves behind.
    #[arg(long)]
    from: Option<PathBuf>,

    /// Use this layout instead of the configured one, for one run.
    #[arg(long)]
    pattern: Option<String>,
}

pub fn run(context: &AppContext, args: &OrganiseArgs) -> Result<i32> {
    let settings = &context.settings().organise;
    let pattern = match &args.pattern {
        Some(text) => Pattern::parse(text)?,
        None => Pattern::parse(&settings.pattern)?,
    };

    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let configured = context.resolved.resolved_organise_destination();

    let (entries, destination) = match &args.from {
        Some(source) => (loose(context, source)?, Some(where_to(context, &roots)?)),
        None => (index.entries()?, configured),
    };

    let planned =
        organise::plan_moves(&entries, &pattern, settings, &roots, destination.as_deref());
    let ready: Vec<&PlannedMove> = planned.iter().filter(|move_| move_.ready()).collect();

    let mut failures: Vec<(String, String)> = Vec::new();
    let mut pruned = 0;
    if args.apply {
        let log = ChangeLog::open(&index, "organise", unix_now())?;
        for planned in &ready {
            let from = &planned.entry.path;
            // Logged before the rename: a row for a move that did not happen is
            // refused by `changes undo` as a file that is gone, where a move
            // with no row is a hole in the record.
            log.moved(&mut index, from, &planned.destination)?;
            match organise::move_file(from, &planned.destination) {
                Ok(()) => {
                    let landed = organise::spelled_on_disk(&planned.destination);
                    index.relocate(from, &landed, &roots)?;
                    if settings.remove_empty_directories {
                        if let Some(root) = owning_root(from, &roots) {
                            pruned += organise::prune_empty(from, root).len();
                        }
                    }
                }
                Err(error) => failures.push((show(from), error.to_string())),
            }
        }
    }

    if context.as_json {
        context.emit(&json!({
            "planned": ready.iter().map(|planned| json!({
                "from": show(&planned.entry.path),
                "to": show(&planned.destination),
            })).collect::<Vec<_>>(),
            "blocked": planned
                .iter()
                .filter_map(blocked_row)
                .collect::<Vec<_>>(),
            "applied": args.apply,
            "failed": failures
                .iter()
                .map(|(path, error)| json!({"path": path, "error": error}))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }

    report(context, &planned, &ready, &failures, args.apply, pruned);
    Ok(0)
}

/// Files under a directory that is not part of the library.
///
/// Described rather than looked up: nothing outside a root is in the index, and
/// the whole point of `--from` is to reach what the index has not seen.
fn loose(context: &AppContext, source: &Path) -> Result<Vec<LibraryEntry>> {
    if !source.is_dir() {
        return Err(Error::Config(format!(
            "--from: not a directory: {}",
            source.display()
        )));
    }
    let extensions: Vec<&str> = context
        .settings()
        .library
        .extensions
        .iter()
        .map(String::as_str)
        .collect();

    Ok(iter_audio_files(source, &extensions, &context.excludes()?)
        .iter()
        .filter_map(|path| describe(path, source).ok())
        .collect())
}

/// Where `--from` sends what it finds.
///
/// With several roots and no configured destination there is no answer, and
/// picking one silently is how a tool moves files somebody did not mean to move.
fn where_to(context: &AppContext, roots: &[PathBuf]) -> Result<PathBuf> {
    if let Some(destination) = context.resolved.resolved_organise_destination() {
        return Ok(destination);
    }
    match roots {
        [only] => Ok(only.clone()),
        _ => Err(Error::Config(
            "--from has several library roots to choose between\nhint: set organise.destination"
                .to_owned(),
        )),
    }
}

fn blocked_row(planned: &PlannedMove) -> Option<serde_json::Value> {
    planned.blocked.as_ref().map(|obstacle| {
        json!({
            "path": show(&planned.entry.path),
            "reason": obstacle.to_string(),
        })
    })
}

fn report(
    context: &AppContext,
    planned: &[PlannedMove],
    ready: &[&PlannedMove],
    failures: &[(String, String)],
    apply: bool,
    pruned: usize,
) {
    if !ready.is_empty() {
        let rows: Vec<Vec<String>> = ready
            .iter()
            .take(50)
            .map(|planned| vec![show(&planned.entry.path), show(&planned.destination)])
            .collect();
        context.say(&table(&["From", "To"], &rows));
    }

    // "Already in place" is the answer for most of a tidy library and says
    // nothing, so it is counted rather than listed.
    let mut settled = 0;
    let mut obstacles: Vec<Vec<String>> = Vec::new();
    for planned in planned {
        match &planned.blocked {
            None => {}
            Some(organise::Obstacle::AlreadyInPlace) => settled += 1,
            Some(obstacle) => obstacles.push(vec![show(&planned.entry.path), obstacle.to_string()]),
        }
    }
    if !obstacles.is_empty() {
        context.say(&table(&["File", "Blocked"], &obstacles));
    }

    context.say(&format!(
        "{} file(s) would move; {settled} already in place; {} blocked",
        ready.len(),
        obstacles.len()
    ));

    if apply {
        context.say(&format!("Moved {} file(s)", ready.len() - failures.len()));
        if pruned > 0 {
            context.say(&format!("Removed {pruned} empty directory(ies)"));
        }
        for (path, reason) in failures {
            context.say(&format!("failed {path}: {reason}"));
        }
    } else if !ready.is_empty() {
        context.say("Nothing was written -- pass --apply to move them.");
    }
}
