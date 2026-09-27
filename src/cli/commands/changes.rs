//! `stk changes` -- what `--apply` wrote, and how to take it back.

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{fail, show, table, unix_now, AppContext};
use crate::errors::Result;
use crate::library::changes::{
    apply_undo, plan_undo, Change, ChangeFilter, ChangeLog, UndoPlan, UNDO_COMMAND,
};
use crate::library::index::LibraryIndex;

#[derive(Subcommand, Debug)]
pub enum ChangesCommand {
    /// Show what the writing commands have changed, newest first.
    List {
        /// Show at most this many rows.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only rows this command wrote, such as 'genre apply'.
        #[arg(long)]
        command: Option<String>,
        /// Only rows from this run.
        #[arg(long)]
        run: Option<i64>,
    },
    /// Put a change back.
    Undo {
        /// The change to undo.
        id: Option<i64>,
        /// Undo the whole of the most recent run instead.
        #[arg(long)]
        last: bool,
        /// Write the files. Without this, nothing is modified.
        #[arg(long)]
        apply: bool,
        /// Undo even a file that has changed since it was logged.
        #[arg(long)]
        force: bool,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: ChangesCommand) -> Result<i32> {
    match command {
        ChangesCommand::List {
            limit,
            command,
            run,
        } => list(context, limit, command, run),
        ChangesCommand::Undo {
            id,
            last,
            apply,
            force,
        } => undo(context, id, last, apply, force),
    }
}

fn list(
    context: &AppContext,
    limit: usize,
    command: Option<String>,
    run: Option<i64>,
) -> Result<i32> {
    let index = context.open_index()?;
    let filtered = command.is_some() || run.is_some();
    let changes = index.changes(&ChangeFilter {
        limit: Some(limit),
        command,
        run,
    })?;

    if context.as_json {
        context.emit(&json!(changes.iter().map(as_row).collect::<Vec<_>>()));
        return Ok(0);
    }

    if changes.is_empty() {
        context.say(if filtered {
            "No change matches that command or run."
        } else {
            "Nothing has been written yet."
        });
        return Ok(0);
    }

    let rows: Vec<Vec<String>> = changes
        .iter()
        .map(|change| {
            vec![
                change.id.to_string(),
                change.run_id.to_string(),
                change.command.clone(),
                change.field.clone(),
                show(&change.path),
            ]
        })
        .collect();
    context.say(&table(&["ID", "Run", "Command", "Field", "File"], &rows));
    Ok(0)
}

fn as_row(change: &Change) -> serde_json::Value {
    json!({
        "id": change.id,
        "run": change.run_id,
        "applied_at": change.applied_at,
        "command": change.command,
        "field": change.field,
        "path": show(&change.path),
        "old_value": change.old_value,
        "new_value": change.new_value,
    })
}

fn undo(
    context: &AppContext,
    id: Option<i64>,
    last: bool,
    apply: bool,
    force: bool,
) -> Result<i32> {
    let mut index = context.open_index()?;
    let Some(changes) = wanted(&index, id, last)? else {
        return Ok(1);
    };
    if changes.is_empty() && context.nothing_to_do("Nothing has been written yet.") {
        return Ok(0);
    }

    let plans: Vec<UndoPlan> = changes
        .into_iter()
        .map(|change| plan_undo(&index, change))
        .collect::<Result<_>>()?;

    if context.as_json {
        context.emit(&json!(plans
            .iter()
            .map(|plan| json!({
                "id": plan.change.id,
                "path": show(&plan.change.path),
                "action": plan.description,
                "blocked": plan.blocked.as_ref().map(ToString::to_string),
                "applied": apply && plan.allowed(force),
            }))
            .collect::<Vec<_>>()));
    } else {
        let rows: Vec<Vec<String>> = plans
            .iter()
            .map(|plan| {
                vec![
                    plan.change.id.to_string(),
                    show(&plan.change.path),
                    plan.description.clone(),
                    plan.blocked
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                ]
            })
            .collect();
        context.say(&table(&["ID", "File", "Would", "Blocked"], &rows));
    }

    if !apply {
        context.say(&format!(
            "{} change(s) would be put back. Nothing was written -- pass --apply.",
            plans.len()
        ));
        return Ok(0);
    }

    let log = ChangeLog::open(&index, UNDO_COMMAND, unix_now())?;
    let roots = context.library_roots();
    let mut undone = 0;
    let mut skipped = 0;
    for plan in &plans {
        if !plan.allowed(force) {
            skipped += 1;
            continue;
        }
        apply_undo(&mut index, plan, &log, &roots)?;
        undone += 1;
    }

    context.say(&format!("Put {undone} change(s) back"));
    if skipped > 0 {
        context.say(&format!("Left {skipped} alone; see the Blocked column"));
    }
    Ok(0)
}

fn wanted(index: &LibraryIndex, id: Option<i64>, last: bool) -> Result<Option<Vec<Change>>> {
    match (id, last) {
        (Some(_), true) => {
            let _ = fail("give either a change id or --last, not both");
            Ok(None)
        }
        (Some(id), false) => {
            if let Some(change) = index.change(id)? {
                return Ok(Some(vec![change]));
            }
            let _ = fail(&format!("no change with id {id}"));
            Ok(None)
        }
        (None, true) => {
            let Some(run) = index.last_run_id()? else {
                return Ok(Some(Vec::new()));
            };
            Ok(Some(index.changes(&ChangeFilter {
                run: Some(run),
                ..ChangeFilter::default()
            })?))
        }
        (None, false) => {
            let _ = fail("give a change id, or --last for the most recent run");
            Ok(None)
        }
    }
}
