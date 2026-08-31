//! `stk tags` -- dump the library's metadata for eyeballing or grepping.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, AppContext};
use crate::errors::{Error, Result};
use crate::models::LibraryEntry;

pub const DUMP_FIELDS: &[&str] = &[
    "path",
    "folder_artist",
    "artist",
    "album",
    "title",
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
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: TagsCommand) -> Result<i32> {
    match command {
        TagsCommand::Dump { output } => dump(context, output.as_deref()),
    }
}

fn cells(entry: &LibraryEntry) -> Vec<String> {
    vec![
        show(&entry.path),
        entry.folder_artist.clone(),
        entry.tags.artist.clone(),
        entry.tags.album.clone(),
        entry.tags.title.clone(),
        entry.tags.genres.join("; "),
        entry
            .duration_seconds
            .filter(|seconds| *seconds != 0.0)
            .map(|seconds| format!("{seconds:.1}"))
            .unwrap_or_default(),
    ]
}

fn dump(context: &AppContext, output: Option<&Path>) -> Result<i32> {
    let entries = context.open_index()?.entries()?;

    if context.as_json {
        let rows: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                let cells = cells(entry);
                let mut row = serde_json::Map::new();
                for (field, value) in DUMP_FIELDS.iter().zip(cells) {
                    row.insert((*field).to_owned(), json!(value));
                }
                serde_json::Value::Object(row)
            })
            .collect();
        context.emit(&json!(rows));
        return Ok(0);
    }

    if let Some(destination) = output {
        write_csv(destination, &entries)?;
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
        .write_record(DUMP_FIELDS)
        .map_err(|error| Error::Config(error.to_string()))?;
    for entry in &entries {
        writer
            .write_record(cells(entry))
            .map_err(|error| Error::Config(error.to_string()))?;
    }
    writer
        .flush()
        .map_err(|error| Error::Config(error.to_string()))?;
    Ok(0)
}

/// Written as `utf-8-sig`, matching the Python: a BOM is what stops Excel
/// mangling every non-ASCII path.
fn write_csv(destination: &Path, entries: &[LibraryEntry]) -> Result<()> {
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
            .write_record(DUMP_FIELDS)
            .map_err(|error| Error::Config(error.to_string()))?;
        for entry in entries {
            writer
                .write_record(cells(entry))
                .map_err(|error| Error::Config(error.to_string()))?;
        }
        writer
            .flush()
            .map_err(|error| Error::Config(error.to_string()))?;
    }

    std::fs::write(destination, buffer)
        .map_err(|error| Error::Config(format!("{}: {error}", destination.display())))
}
