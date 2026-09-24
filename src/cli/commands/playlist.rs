//! `stk playlist` -- get an order back out of the library as M3U.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use crate::cli::commands::wantlist::ExportArgs;
use crate::cli::context::{show, AppContext};
use crate::errors::{Error, Result};
use crate::genres::TrackKey;
use crate::models::LibraryEntry;
use crate::playlist::{self, PlaylistEntry};
use crate::text::normalize::normalize;
use crate::wantlist;

#[derive(Subcommand, Debug)]
pub enum PlaylistCommand {
    /// Write the tracks of an export the library holds, in its order.
    Export {
        #[command(flatten)]
        export: ExportArgs,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Write every file carrying a genre.
    Genre {
        genre: String,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Write every file filed under an artist.
    Artist {
        artist: String,
        #[command(flatten)]
        output: OutputArgs,
    },
}

#[derive(clap::Args, Debug)]
pub struct OutputArgs {
    /// Write the playlist here instead of to stdout.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Write paths relative to the playlist, so it can be moved with the music.
    #[arg(long)]
    relative: bool,
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: PlaylistCommand) -> Result<i32> {
    match command {
        PlaylistCommand::Export { export, output } => {
            let Some(entries) = exported(context, &export)? else {
                return Ok(1);
            };
            write(context, &entries, &output)
        }
        PlaylistCommand::Genre { genre, output } => {
            let wanted = normalize(&genre);
            write(
                context,
                &matching(context, |entry| {
                    entry
                        .tags
                        .genres
                        .iter()
                        .any(|genre| normalize(genre) == wanted)
                })?,
                &output,
            )
        }
        PlaylistCommand::Artist { artist, output } => {
            let wanted = normalize(&artist);
            write(
                context,
                &matching(context, |entry| {
                    normalize(TrackKey::of(entry).artist) == wanted
                })?,
                &output,
            )
        }
    }
}

/// One file's line, described by the file rather than by whatever asked for it.
///
/// A fuzzy match against an export can land on a file whose tags spell the
/// artist differently; the playlist says what will actually play.
fn line(entry: &LibraryEntry) -> PlaylistEntry {
    PlaylistEntry {
        path: entry.path.clone(),
        artist: entry.artist.raw.clone(),
        title: entry.title.raw.clone(),
        duration_seconds: entry.duration_seconds,
    }
}

/// The export's own rows, in its own order, for the ones the library holds.
///
/// `wantlist::diff` preserves the export order in `owned`, which is the whole
/// point: the order was in the export and nowhere else.
fn exported(context: &AppContext, export: &ExportArgs) -> Result<Option<Vec<PlaylistEntry>>> {
    let tracks = match export.read() {
        Ok(tracks) => tracks,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(None);
        }
    };

    let entries = context.open_index()?.entries()?;
    let result = wantlist::diff(&tracks, &entries, &context.settings().matching.thresholds());
    Ok(Some(
        result
            .owned
            .iter()
            .map(|owned| line(&owned.entry))
            .collect(),
    ))
}

fn matching(
    context: &AppContext,
    wanted: impl Fn(&LibraryEntry) -> bool,
) -> Result<Vec<PlaylistEntry>> {
    Ok(context
        .open_index()?
        .entries()?
        .iter()
        .filter(|entry| wanted(entry))
        .map(line)
        .collect())
}

fn write(context: &AppContext, entries: &[PlaylistEntry], output: &OutputArgs) -> Result<i32> {
    // Checked before the JSON branch, or a script -- the one caller most likely
    // to pass both -- would be the only one the flag was silently ignored for.
    if output.relative && output.output.is_none() {
        return Err(Error::Config(
            "--relative needs --output: the paths are relative to the playlist file".to_owned(),
        ));
    }

    if context.as_json {
        context.emit(&json!(entries
            .iter()
            .map(|entry| json!({
                "path": show(&entry.path),
                "artist": entry.artist,
                "title": entry.title,
                "duration_seconds": entry.duration_seconds,
            }))
            .collect::<Vec<_>>()));
        return Ok(0);
    }

    // Absolute, and only then the parent: `-o list.m3u8` names the working
    // directory, whose relative spelling is "" and shares nothing with anything.
    let base = output
        .relative
        .then_some(output.output.as_deref())
        .flatten()
        .and_then(|destination| std::path::absolute(destination).ok())
        .and_then(|destination| destination.parent().map(Path::to_path_buf));
    let text = playlist::render(entries, base.as_deref());

    let Some(destination) = &output.output else {
        print!("{text}");
        return Ok(0);
    };

    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::Config(format!("{}: {error}", parent.display())))?;
        }
    }
    // No BOM, unlike this project's CSV writers: .m3u8 means UTF-8 by
    // definition, and a BOM makes several players fail on the first path.
    std::fs::write(destination, text)
        .map_err(|error| Error::Config(format!("{}: {error}", destination.display())))?;

    context.say(&format!(
        "Wrote {} track(s) to {}",
        entries.len(),
        show(destination)
    ));
    Ok(0)
}
