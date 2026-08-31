//! `stk wantlist` -- what a streaming export contains that the library does not.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, AppContext};
use crate::errors::{Error, Result};
use crate::wantlist::{self, WantlistDiff};

#[derive(Subcommand, Debug)]
pub enum WantlistCommand {
    /// List the tracks in an export that the library does not already hold.
    Diff {
        /// CSV exported from a streaming service.
        export: PathBuf,
        /// Add the missing tracks to the review queue.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        queue: bool,
        /// Also write the missing tracks here.
        #[arg(long)]
        missing: Option<PathBuf>,
        /// Write the matched tracks and their scores here.
        #[arg(long)]
        owned: Option<PathBuf>,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: WantlistCommand) -> Result<i32> {
    match command {
        WantlistCommand::Diff {
            export,
            queue,
            missing,
            owned,
        } => diff(
            context,
            &export,
            queue,
            missing.as_deref(),
            owned.as_deref(),
        ),
    }
}

/// An uncertain match is reported as missing rather than owned: a duplicate
/// download costs a few megabytes, a track dropped by mistake is simply gone.
fn diff(
    context: &AppContext,
    export: &Path,
    queue: bool,
    missing_csv: Option<&Path>,
    owned_csv: Option<&Path>,
) -> Result<i32> {
    let tracks = match wantlist::read_export(export) {
        Ok(tracks) => tracks,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(1);
        }
    };

    let entries = context.open_index()?.entries()?;
    let result = wantlist::diff(&tracks, &entries, &context.settings().matching.thresholds());

    let mut queued = 0;
    if queue && !result.missing.is_empty() {
        queued = context.open_review()?.add(&result.missing)?;
    }

    if let Some(destination) = missing_csv {
        write_missing(destination, &result)?;
    }
    if let Some(destination) = owned_csv {
        write_owned(destination, &result)?;
    }

    if context.as_json {
        context.emit(&json!({
            "total": result.total(),
            "missing": result
                .missing
                .iter()
                .map(|track| json!({"artist": track.artist, "title": track.title}))
                .collect::<Vec<_>>(),
            "owned": result
                .owned
                .iter()
                .map(|owned| json!({
                    "artist": owned.track.artist,
                    "title": owned.track.title,
                    "path": show(&owned.entry.path),
                    "score": round(owned.score, 3),
                }))
                .collect::<Vec<_>>(),
            "queued": queued,
        }));
        return Ok(0);
    }

    if entries.is_empty() {
        context.say("The library index is empty -- run 'stk library scan' first.");
    }

    if !result.owned.is_empty() {
        let mut least_confident = result.owned.clone();
        least_confident.sort_by(|left, right| {
            left.score
                .partial_cmp(&right.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let rows: Vec<Vec<String>> = least_confident
            .iter()
            .take(20)
            .map(|owned| {
                vec![
                    format!("{:.2}", owned.score),
                    format!("{} - {}", owned.track.artist, owned.track.title),
                    show(&owned.entry.path),
                ]
            })
            .collect();
        context.say(&table(&["Score", "Wanted", "Matched"], &rows));
        context.say("Least confident matches shown first -- check these before trusting them.\n");
    }

    context.say(&format!(
        "{} of {} track(s) missing, {} owned",
        result.missing.len(),
        result.total(),
        result.owned.len()
    ));
    if queued > 0 {
        context.say(&format!("Queued {queued} new track(s) for searching"));
    }
    Ok(0)
}

/// The missing list keeps the export's own columns, so it can be fed straight
/// back into whatever produced it.
fn write_missing(destination: &Path, result: &WantlistDiff) -> Result<()> {
    let headers: Vec<String> = result
        .missing
        .first()
        .filter(|track| !track.source_row.is_empty())
        .map_or_else(
            || vec!["artist".to_owned(), "title".to_owned()],
            |track| {
                track
                    .source_row
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect()
            },
        );

    let rows: Vec<Vec<String>> = result
        .missing
        .iter()
        .map(|track| {
            if track.source_row.is_empty() {
                vec![track.artist.clone(), track.title.clone()]
            } else {
                headers
                    .iter()
                    .map(|header| {
                        track
                            .source_row
                            .iter()
                            .find(|(key, _)| key == header)
                            .map(|(_, value)| value.clone())
                            .unwrap_or_default()
                    })
                    .collect()
            }
        })
        .collect();

    write_csv(destination, &headers, &rows)
}

fn write_owned(destination: &Path, result: &WantlistDiff) -> Result<()> {
    let mut owned = result.owned.clone();
    owned.sort_by(|left, right| {
        left.score
            .partial_cmp(&right.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let rows: Vec<Vec<String>> = owned
        .iter()
        .map(|owned| {
            vec![
                owned.track.artist.clone(),
                owned.track.title.clone(),
                format!("{}", round(owned.score, 3)),
                show(&owned.entry.path),
            ]
        })
        .collect();

    write_csv(
        destination,
        &[
            "artist".to_owned(),
            "title".to_owned(),
            "score".to_owned(),
            "matched_path".to_owned(),
        ],
        &rows,
    )
}

/// Written as `utf-8-sig`, matching the Python's CSV writers.
fn write_csv(destination: &Path, headers: &[String], rows: &[Vec<String>]) -> Result<()> {
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
            .write_record(headers)
            .map_err(|error| Error::Config(error.to_string()))?;
        for row in rows {
            writer
                .write_record(row)
                .map_err(|error| Error::Config(error.to_string()))?;
        }
        writer
            .flush()
            .map_err(|error| Error::Config(error.to_string()))?;
    }

    std::fs::write(destination, buffer)
        .map_err(|error| Error::Config(format!("{}: {error}", destination.display())))
}

/// Python's `round(value, 3)`, which breaks ties to even.
fn round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}
