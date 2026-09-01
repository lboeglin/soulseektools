//! `stk tags` -- dump the library's metadata for eyeballing or grepping.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::library::changes::ChangeLog;
use crate::library::tags::TagField;
use crate::models::LibraryEntry;
use crate::tagsheet::{self, Blanks, PlannedEdit, TagChange};

/// The dump's columns, which `oracle/tags.csv` froze.
///
/// Not extended in place: the oracle is the specification for what the port
/// preserved, and a round trip needs two more columns that nothing else does.
/// `--round-trip` asks for those instead.
pub const DUMP_FIELDS: &[&str] = &[
    "path",
    "folder_artist",
    "artist",
    "album",
    "title",
    "genres",
    "duration_seconds",
];

/// The columns `stk tags apply` needs to put a row back exactly.
///
/// `path` stays first: it is how the dump is keyed, and how the oracle suites
/// find the library a capture was taken from.
pub const ROUND_TRIP_FIELDS: &[&str] = &[
    "path",
    "folder_artist",
    "artist",
    "album_artist",
    "album",
    "title",
    "track_number",
    "genres",
    "duration_seconds",
];

#[derive(Subcommand, Debug)]
pub enum TagsCommand {
    /// Print artist, album, title and genre for every indexed file.
    Dump {
        /// Write CSV here instead of to stdout.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        /// Add the columns `tags apply` needs to write a row back.
        #[arg(long)]
        round_trip: bool,
    },
    /// Write the tags an edited dump asks for.
    Apply {
        /// The edited CSV.
        sheet: PathBuf,
        /// Write the tags. Without this, nothing is modified.
        #[arg(long)]
        apply: bool,
        /// Treat an empty cell as "remove this tag" rather than "leave it".
        #[arg(long)]
        clear_blanks: bool,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: TagsCommand) -> Result<i32> {
    match command {
        TagsCommand::Dump { output, round_trip } => {
            let fields = if round_trip {
                ROUND_TRIP_FIELDS
            } else {
                DUMP_FIELDS
            };
            dump(context, output.as_deref(), fields)
        }
        TagsCommand::Apply {
            sheet,
            apply,
            clear_blanks,
        } => apply_sheet(context, &sheet, apply, clear_blanks),
    }
}

/// One column's value for one file, stated once so both column sets agree.
fn cell(entry: &LibraryEntry, column: &str) -> String {
    match column {
        "path" => show(&entry.path),
        "folder_artist" => entry.folder_artist.clone(),
        "duration_seconds" => entry
            .duration_seconds
            .filter(|seconds| *seconds != 0.0)
            .map(|seconds| format!("{seconds:.1}"))
            .unwrap_or_default(),
        tag => tag
            .parse::<TagField>()
            .map(|field| field.value(&entry.tags))
            .unwrap_or_default(),
    }
}

fn cells(entry: &LibraryEntry, fields: &[&str]) -> Vec<String> {
    fields.iter().map(|field| cell(entry, field)).collect()
}

fn dump(context: &AppContext, output: Option<&Path>, fields: &[&str]) -> Result<i32> {
    let entries = context.open_index()?.entries()?;

    if context.as_json {
        let rows: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                let cells = cells(entry, fields);
                let mut row = serde_json::Map::new();
                for (field, value) in fields.iter().zip(cells) {
                    row.insert((*field).to_owned(), json!(value));
                }
                serde_json::Value::Object(row)
            })
            .collect();
        context.emit(&json!(rows));
        return Ok(0);
    }

    if let Some(destination) = output {
        write_csv(destination, &entries, fields)?;
        context.say(&format!(
            "Wrote {} row(s) to {}",
            entries.len(),
            show(destination)
        ));
        return Ok(0);
    }

    // To stdout the Python uses a plain "\n" terminator rather than the CSV
    // default, so the output pipes cleanly.
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(std::io::stdout().lock());
    writer
        .write_record(fields)
        .map_err(|error| Error::Config(error.to_string()))?;
    for entry in &entries {
        writer
            .write_record(cells(entry, fields))
            .map_err(|error| Error::Config(error.to_string()))?;
    }
    writer
        .flush()
        .map_err(|error| Error::Config(error.to_string()))?;
    Ok(0)
}

/// Written as `utf-8-sig`, matching the Python: a BOM is what stops Excel
/// mangling every non-ASCII path.
fn write_csv(destination: &Path, entries: &[LibraryEntry], fields: &[&str]) -> Result<()> {
    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::Config(format!("{}: {error}", parent.display())))?;
        }
    }

    let mut buffer = b"\xef\xbb\xbf".to_vec();
    {
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(&mut buffer);
        writer
            .write_record(fields)
            .map_err(|error| Error::Config(error.to_string()))?;
        for entry in entries {
            writer
                .write_record(cells(entry, fields))
                .map_err(|error| Error::Config(error.to_string()))?;
        }
        writer
            .flush()
            .map_err(|error| Error::Config(error.to_string()))?;
    }

    std::fs::write(destination, buffer)
        .map_err(|error| Error::Config(format!("{}: {error}", destination.display())))
}

fn apply_sheet(context: &AppContext, sheet: &Path, apply: bool, clear_blanks: bool) -> Result<i32> {
    let blanks = if clear_blanks {
        Blanks::Clear
    } else {
        Blanks::LeaveAlone
    };
    let edits = match tagsheet::read_sheet(sheet, blanks) {
        Ok(edits) => edits,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(1);
        }
    };

    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let planned = tagsheet::plan_changes(&edits, &index.entries()?);

    let changes: Vec<&TagChange> = planned
        .iter()
        .filter_map(|edit| match edit {
            PlannedEdit::Change(change) => Some(change.as_ref()),
            PlannedEdit::Skip { .. } => None,
        })
        .collect();

    let mut failures: Vec<(String, String)> = Vec::new();
    if apply {
        let log = ChangeLog::open(&index, "tags apply", unix_now())?;
        for change in &changes {
            let path = &change.edit.path;
            match tagsheet::apply_change(change) {
                Ok(before) => {
                    for field in &change.fields {
                        log.tag(&mut index, path, *field, &before, &change.proposed)?;
                    }
                    index.reindex(path, &roots)?;
                }
                Err(error) => failures.push((show(path), error.to_string())),
            }
        }
    }

    if context.as_json {
        context.emit(&json!({
            "planned": changes.iter().copied().map(planned_row).collect::<Vec<_>>(),
            "skipped": planned.iter().filter_map(skipped_row).collect::<Vec<_>>(),
            "applied": apply,
            "failed": failures
                .iter()
                .map(|(path, error)| json!({"path": path, "error": error}))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }

    report(context, &planned, &changes, &failures, apply);
    Ok(0)
}

fn planned_row(change: &TagChange) -> serde_json::Value {
    json!({
        "path": show(&change.edit.path),
        "fields": change
            .fields
            .iter()
            .map(|field| json!({
                "field": field.as_str(),
                "from": field.value(&change.current),
                "to": field.value(&change.proposed),
            }))
            .collect::<Vec<_>>(),
    })
}

fn skipped_row(edit: &PlannedEdit) -> Option<serde_json::Value> {
    match edit {
        PlannedEdit::Change(_) => None,
        PlannedEdit::Skip { path, reason } => Some(json!({
            "path": show(path),
            "reason": reason.to_string(),
        })),
    }
}

fn report(
    context: &AppContext,
    planned: &[PlannedEdit],
    changes: &[&TagChange],
    failures: &[(String, String)],
    apply: bool,
) {
    let rows: Vec<Vec<String>> = changes
        .iter()
        .flat_map(|change| {
            change.fields.iter().map(move |field| {
                vec![
                    show(&change.edit.path),
                    field.as_str().to_owned(),
                    field.value(&change.current),
                    field.value(&change.proposed),
                ]
            })
        })
        .take(50)
        .collect();
    if !rows.is_empty() {
        context.say(&table(&["File", "Field", "From", "To"], &rows));
    }

    // Rows the sheet named that this could not act on. "Already what the sheet
    // asks for" is the usual answer and says nothing, so it is only counted.
    let mut unchanged = 0;
    for edit in planned {
        if let PlannedEdit::Skip { path, reason } = edit {
            if matches!(reason, tagsheet::Skipped::NoChange) {
                unchanged += 1;
            } else {
                context.say(&format!("skipped {}: {reason}", show(path)));
            }
        }
    }
    if unchanged > 0 {
        context.say(&format!(
            "{unchanged} row(s) already say what the sheet says"
        ));
    }

    if changes.is_empty() {
        context.say("Nothing to change.");
        return;
    }

    if apply {
        context.say(&format!(
            "Tagged {} file(s)",
            changes.len() - failures.len()
        ));
        for (path, reason) in failures {
            context.say(&format!("failed {path}: {reason}"));
        }
    } else {
        context.say(&format!(
            "{} file(s) would change. Nothing was written -- pass --apply to tag them.",
            changes.len()
        ));
    }
}
