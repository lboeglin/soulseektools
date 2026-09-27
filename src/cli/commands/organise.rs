//! `stk organise` -- lay files out from what their tags say.

use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::library::scan::owning_root;
use crate::models::LibraryEntry;
use crate::organise::{self, Applied, Pattern, PlannedMove};

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

    if let Some(destination) = &destination {
        inside_a_root(destination, &roots)?;
    }

    let planned =
        organise::plan_moves(&entries, &pattern, settings, &roots, destination.as_deref());
    let ready: Vec<&PlannedMove> = planned.iter().filter(|move_| move_.ready()).collect();

    let applied = if args.apply {
        organise::apply_moves(
            &mut index,
            &ready,
            &roots,
            args.from.as_deref(),
            settings,
            unix_now(),
        )?
    } else {
        Applied::default()
    };
    let failures: Vec<(String, String)> = applied
        .failures
        .iter()
        .map(|(path, error)| (show(path), error.clone()))
        .collect();

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

    report(
        context,
        &planned,
        &ready,
        &failures,
        args.apply,
        applied.pruned,
    );
    Ok(0)
}

pub(crate) fn inside_a_root(destination: &Path, roots: &[PathBuf]) -> Result<()> {
    // Otherwise the files leave the library and the next scan evicts them.
    if owning_root(destination, roots).is_none() {
        return Err(Error::Config(format!(
            "organise.destination is outside every library root: {}\n\
             hint: point it inside one, or add it to library.roots",
            show(destination)
        )));
    }
    Ok(())
}

fn loose(context: &AppContext, source: &Path) -> Result<Vec<LibraryEntry>> {
    let extensions: Vec<&str> = context
        .settings()
        .library
        .extensions
        .iter()
        .map(String::as_str)
        .collect();
    organise::loose_entries(source, &extensions, &context.excludes()?)
        .map_err(|error| Error::Config(format!("--from: {error}")))
}

/// With several roots and no configured destination this is an error, not a guess.
pub(crate) fn where_to(context: &AppContext, roots: &[PathBuf]) -> Result<PathBuf> {
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

pub(crate) fn blocked_row(planned: &PlannedMove) -> Option<serde_json::Value> {
    planned.blocked.as_ref().map(|obstacle| {
        json!({
            "path": show(&planned.entry.path),
            "reason": obstacle.to_string(),
        })
    })
}

pub(crate) fn report(
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

    // "Already in place" is counted rather than listed.
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
            context.say_failure(path, reason);
        }
    } else if !ready.is_empty() {
        context.say("Nothing was written -- pass --apply to move them.");
    }
}
