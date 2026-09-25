//! `stk tags`: dump metadata, write an edited dump back, identify unknown files.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::identify::acoustid::{self, AcoustId};
use crate::identify::{self, Identification, Outcome};
use crate::library::changes::ChangeLog;
use crate::library::index::LibraryIndex;
use crate::library::tags::TagField;
use crate::models::LibraryEntry;
use crate::services::HttpTransport;
use crate::tagsheet::{self, Blanks, PlannedEdit, TagChange};

/// The dump's columns. `--round-trip` adds the two a write-back needs.
pub const DUMP_FIELDS: &[&str] = &[
    "path",
    "folder_artist",
    "artist",
    "album",
    "title",
    "genres",
    "duration_seconds",
];

/// `path` stays first: it is how the sheet is keyed.
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
    /// Identify files from their audio through AcoustID, and tag them.
    Identify {
        /// File or folder to identify. Defaults to the whole library.
        path: Option<PathBuf>,
        /// Look up every file, not only those missing an artist or a title.
        #[arg(long)]
        all: bool,
        /// Write the tags. Without this, nothing is modified.
        #[arg(long)]
        apply: bool,
        /// Look up at most this many files.
        #[arg(long)]
        limit: Option<usize>,
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
        TagsCommand::Identify {
            path,
            all,
            apply,
            limit,
        } => identify_files(context, path.as_deref(), all, apply, limit),
    }
}

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

    // A plain "\n" terminator on stdout, so the output pipes cleanly.
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

/// Written with a BOM, or Excel mangles every non-ASCII path.
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
        failures = write_changes(&mut index, &roots, &log, &changes)?;
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

/// A failed write, including a file changed since the plan, is reported, not fatal.
fn write_changes(
    index: &mut LibraryIndex,
    roots: &[PathBuf],
    log: &ChangeLog,
    changes: &[&TagChange],
) -> Result<Vec<(String, String)>> {
    let mut failures = Vec::new();
    for change in changes {
        let path = &change.edit.path;
        match tagsheet::apply_change(change) {
            Ok(before) => {
                for field in &change.fields {
                    log.tag(index, path, *field, &before, &change.proposed)?;
                }
                index.reindex(path, roots)?;
            }
            Err(error) => failures.push((show(path), error.to_string())),
        }
    }
    Ok(failures)
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

    // "Already what the sheet asks for" is only counted.
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

/// Only files missing an artist or title unless `--all`.
fn identify_files(
    context: &AppContext,
    path: Option<&Path>,
    all: bool,
    apply: bool,
    limit: Option<usize>,
) -> Result<i32> {
    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let entries = index.entries()?;
    if entries.is_empty() && context.nothing_to_do("Index is empty. Run stk library scan first.") {
        return Ok(0);
    }

    // The index keys files by absolute path.
    let path = path.map(|path| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    let under = |entry: &LibraryEntry| {
        path.as_ref()
            .is_none_or(|path| entry.path.starts_with(path))
    };
    let mut targets: Vec<LibraryEntry> = entries
        .iter()
        .filter(|entry| under(entry))
        .filter(|entry| tagsheet::taggable(&entry.path))
        .filter(|entry| all || entry.tags.artist.is_empty() || entry.tags.title.is_empty())
        .cloned()
        .collect();
    if let Some(limit) = limit {
        targets.truncate(limit);
    }
    if targets.is_empty()
        && context.nothing_to_do(&match &path {
            Some(path) if !entries.iter().any(under) => {
                format!(
                    "Nothing indexed under {}. Run stk library scan first.",
                    show(path)
                )
            }
            _ if all => "Nothing to identify: no taggable file is indexed there.".to_owned(),
            _ => "Nothing to identify: every indexed file names an artist and a title. \
                  Pass --all to look them up anyway."
                .to_owned(),
        })
    {
        return Ok(0);
    }

    let settings = &context.settings().identify;
    let http = HttpTransport::new(
        settings.request_timeout_seconds,
        &context.settings().artwork.contact,
    )?;
    let client = AcoustId::new(acoustid::MIN_INTERVAL_SECONDS);
    let prints = index.fingerprints(&targets);
    let identifications = identify::identify(&http, &client, &targets, prints, settings);

    let planned = tagsheet::plan_changes(&identify::edits(&identifications), &entries);
    let changes: Vec<&TagChange> = planned
        .iter()
        .filter_map(|edit| match edit {
            PlannedEdit::Change(change) => Some(change.as_ref()),
            PlannedEdit::Skip { .. } => None,
        })
        .collect();

    let mut failures: Vec<(String, String)> = identifications
        .iter()
        .filter_map(|identification| match &identification.outcome {
            Outcome::Failed(error) => Some((show(&identification.entry.path), error.clone())),
            Outcome::Match(_) | Outcome::NoMatch(_) => None,
        })
        .collect();
    if apply {
        let log = ChangeLog::open(&index, "tags identify", unix_now())?;
        failures.extend(write_changes(&mut index, &roots, &log, &changes)?);
        keep_prints(&mut index, &changes, &targets);
    }

    if context.as_json {
        context.emit(&json!({
            "identified": identifications.iter().filter_map(identified_row).collect::<Vec<_>>(),
            "unmatched": identifications.iter().filter_map(unmatched_row).collect::<Vec<_>>(),
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

    report_identifications(context, &identifications);
    report(context, &planned, &changes, &failures, apply);
    Ok(0)
}

/// Writing tags moves the mtime the fingerprint cache is keyed on; the audio is unchanged.
fn keep_prints(index: &mut LibraryIndex, changes: &[&TagChange], targets: &[LibraryEntry]) {
    for change in changes {
        let path = &change.edit.path;
        let Some(before) = targets.iter().find(|entry| &entry.path == path) else {
            continue;
        };
        let Ok(Some(print)) = index.fingerprint(path, before.mtime) else {
            continue;
        };
        if let Ok(Some(after)) = index.entry(path) {
            let _ = index.store_fingerprint(path, after.mtime, &print);
        }
    }
}

fn identified_row(identification: &Identification) -> Option<serde_json::Value> {
    let Outcome::Match(found) = &identification.outcome else {
        return None;
    };
    Some(json!({
        "path": show(&identification.entry.path),
        "score": found.score,
        "recording_id": found.recording_id,
        "release_id": found.release_id,
        "year": found.year,
        "reason": found.reason,
        "proposed": {
            "artist": found.proposed.artist,
            "album_artist": found.proposed.album_artist,
            "album": found.proposed.album,
            "title": found.proposed.title,
            "track_number": found.proposed.track_number,
        },
    }))
}

fn unmatched_row(identification: &Identification) -> Option<serde_json::Value> {
    let Outcome::NoMatch(reason) = &identification.outcome else {
        return None;
    };
    Some(json!({
        "path": show(&identification.entry.path),
        "reason": reason,
    }))
}

fn report_identifications(context: &AppContext, identifications: &[Identification]) {
    let rows: Vec<Vec<String>> = identifications
        .iter()
        .map(|identification| {
            let name = identification
                .entry
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match &identification.outcome {
                Outcome::Match(found) => vec![
                    format!("{:.2}", found.score),
                    name,
                    format!("{} - {}", found.proposed.artist, found.proposed.title),
                    match found.year {
                        Some(year) => format!("{} ({year})", found.proposed.album),
                        None => found.proposed.album.clone(),
                    },
                    found.reason.clone(),
                ],
                Outcome::NoMatch(reason) => vec!["--".to_owned(), name, reason.clone()],
                Outcome::Failed(error) => vec!["!!".to_owned(), name, error.clone()],
            }
        })
        .collect();
    context.say(&table(&["Score", "File", "Track", "Release", "Why"], &rows));
}
