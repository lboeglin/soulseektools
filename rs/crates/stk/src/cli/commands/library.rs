//! `stk library` -- build and inspect the cached view of the library.

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, AppContext};
use crate::errors::Result;

#[derive(Subcommand, Debug)]
pub enum LibraryCommand {
    /// Bring the index in line with what is on disk.
    Scan {
        /// Re-read every file, ignoring the cached size and mtime.
        #[arg(long)]
        full: bool,
    },
    /// Summarise what the index currently holds.
    Stats {
        /// How many artists to list.
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: LibraryCommand) -> Result<i32> {
    match command {
        LibraryCommand::Scan { full } => scan(context, full),
        LibraryCommand::Stats { top } => stats(context, top),
    }
}

fn scan(context: &AppContext, full: bool) -> Result<i32> {
    let roots = context.require_library_roots()?;
    let exclude = context.excludes()?;
    let extensions: Vec<&str> = context
        .settings()
        .library
        .extensions
        .iter()
        .map(String::as_str)
        .collect();

    let mut index = context.open_index()?;
    let result = index.refresh(&roots, &extensions, &exclude, full, None)?;
    let total = index.count()?;

    if context.as_json {
        context.emit(&json!({
            "roots": roots.iter().map(|root| show(root)).collect::<Vec<_>>(),
            "added": result.added,
            "updated": result.updated,
            "unchanged": result.unchanged,
            "removed": result.removed,
            "failed": result
                .failed
                .iter()
                .map(|(path, reason)| json!([show(path), reason]))
                .collect::<Vec<_>>(),
            "total": total,
        }));
        return Ok(0);
    }

    context.say(&format!(
        "Indexed {total} file(s) under {}: {} added, {} updated, {} unchanged, {} removed",
        roots
            .iter()
            .map(|root| show(root))
            .collect::<Vec<_>>()
            .join(", "),
        result.added,
        result.updated,
        result.unchanged,
        result.removed
    ));
    for (path, reason) in &result.failed {
        context.say(&format!("skipped {}: {reason}", show(path)));
    }
    Ok(0)
}

fn stats(context: &AppContext, top: usize) -> Result<i32> {
    let index = context.open_index()?;
    let total = index.count()?;
    let by_artist = index.artist_counts()?;
    let entries = index.entries()?;

    let tagged = entries
        .iter()
        .filter(|entry| !entry.tags.title.is_empty())
        .count();
    let with_art = entries.iter().filter(|entry| entry.has_artwork()).count();
    let genres = entries
        .iter()
        .filter(|entry| !entry.tags.genres.is_empty())
        .count();

    if context.as_json {
        let mut artists = serde_json::Map::new();
        for (artist, count) in &by_artist {
            artists.insert(artist.clone(), json!(count));
        }
        context.emit(&json!({
            "total": total,
            "tagged": tagged,
            "with_artwork": with_art,
            "with_genre": genres,
            "artists": artists,
        }));
        return Ok(0);
    }

    if total == 0 {
        context.say("Index is empty. Run stk library scan first.");
        return Ok(0);
    }

    context.say(&format!(
        "{total} file(s) across {} artist folder(s): {tagged} titled, {with_art} with artwork, \
         {genres} with a genre",
        by_artist.len()
    ));

    let rows: Vec<Vec<String>> = by_artist
        .iter()
        .take(top)
        .map(|(artist, count)| {
            let name = if artist.is_empty() {
                "<library root>"
            } else {
                artist
            };
            vec![name.to_owned(), count.to_string()]
        })
        .collect();
    context.say(&format!(
        "Largest {} artist folders",
        top.min(by_artist.len())
    ));
    context.say(&table(&["Artist", "Files"], &rows));
    Ok(0)
}
