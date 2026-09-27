//! `stk genre` -- apply a consistent genre taxonomy across the library.

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::Result;
use crate::genres;
use crate::library::changes::ChangeLog;

#[derive(Subcommand, Debug)]
pub enum GenreCommand {
    /// Show the genres currently in use and which artists the map does not cover.
    Check,
    /// Set each mapped artist's genre tag across their files.
    Apply {
        /// Write the tags. Without this, nothing is modified.
        #[arg(long)]
        apply: bool,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: GenreCommand) -> Result<i32> {
    match command {
        GenreCommand::Check => check(context),
        GenreCommand::Apply { apply } => apply_genres(context, apply),
    }
}

fn check(context: &AppContext) -> Result<i32> {
    let entries = context.open_index()?.entries()?;

    // First-seen order for JSON; only the table sorts by count.
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut untagged = 0;
    for entry in &entries {
        if entry.tags.genres.is_empty() {
            untagged += 1;
        }
        for genre in &entry.tags.genres {
            match counts.iter_mut().find(|(seen, _)| seen == genre) {
                Some((_, count)) => *count += 1,
                None => counts.push((genre.clone(), 1)),
            }
        }
    }

    let map_path = context.resolved.resolved_genre_map_path();
    let unmapped = if map_path.is_file() {
        genres::unmapped_artists(&entries, &genres::load_genre_map(&map_path)?)
    } else {
        Vec::new()
    };

    if context.as_json {
        let mut genre_table = serde_json::Map::new();
        for (genre, count) in &counts {
            genre_table.insert(genre.clone(), json!(count));
        }
        let mut unmapped_table = serde_json::Map::new();
        for (artist, count) in &unmapped {
            unmapped_table.insert(artist.clone(), json!(count));
        }
        context.emit(&json!({
            "genres": genre_table,
            "untagged": untagged,
            "unmapped_artists": unmapped_table,
        }));
        return Ok(0);
    }

    let mut ordered = counts.clone();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    if !ordered.is_empty() {
        let rows: Vec<Vec<String>> = ordered
            .iter()
            .map(|(genre, count)| vec![count.to_string(), genre.clone()])
            .collect();
        context.say(&table(&["Files", "Genre"], &rows));
    }
    context.say(&format!(
        "{untagged} of {} file(s) have no genre tag",
        entries.len()
    ));

    if !map_path.is_file() {
        context.say(&format!(
            "No genre map at {} -- see data/genres.example.toml",
            show(&map_path)
        ));
        return Ok(0);
    }
    if !unmapped.is_empty() {
        context.say(&format!(
            "\n{} artist folder(s) missing from {}:",
            unmapped.len(),
            map_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        for (artist, count) in &unmapped {
            context.say(&format!("  {count:4}  {artist}"));
        }
    }
    Ok(0)
}

fn apply_genres(context: &AppContext, apply: bool) -> Result<i32> {
    let map_path = context.resolved.resolved_genre_map_path();
    let genre_map = match genres::load_genre_map(&map_path) {
        Ok(genre_map) => genre_map,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("hint: copy data/genres.example.toml and edit it for your library");
            return Ok(1);
        }
    };

    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let entries = index.entries()?;
    let changes: Vec<genres::GenreChange> = genres::plan_changes(&entries, &genre_map)
        .into_iter()
        .filter(genres::GenreChange::is_change)
        .collect();

    let mut failures: Vec<(String, String)> = Vec::new();
    if apply {
        let log = ChangeLog::open(&index, "genre apply", unix_now())?;
        for change in &changes {
            match genres::apply_change(change) {
                Ok(()) => {
                    log.genre(
                        &mut index,
                        &change.entry.path,
                        &change.current,
                        &change.proposed,
                    )?;
                    index.reindex(&change.entry.path, &roots)?;
                }
                Err(error) => {
                    failures.push((show(&change.entry.path), error.to_string()));
                }
            }
        }
    }

    if context.as_json {
        context.emit(&json!({
            "planned": changes
                .iter()
                .map(|change| json!({
                    "path": show(&change.entry.path),
                    "from": change.current,
                    "to": change.proposed,
                }))
                .collect::<Vec<_>>(),
            "applied": apply,
            "failed": failures
                .iter()
                .map(|(path, error)| json!({"path": path, "error": error}))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }

    if changes.is_empty() {
        context.say(&format!(
            "Every mapped file already carries its genre ({} artists).",
            genre_map.len()
        ));
        return Ok(0);
    }

    let rows: Vec<Vec<String>> = changes
        .iter()
        .take(50)
        .map(|change| {
            let current = if change.current.is_empty() {
                "none".to_owned()
            } else {
                change.current.join(", ")
            };
            vec![
                show(&change.entry.path),
                current,
                change.proposed.join(", "),
            ]
        })
        .collect();
    context.say(&table(&["File", "From", "To"], &rows));

    if apply {
        context.say(&format!(
            "Tagged {} file(s)",
            changes.len() - failures.len()
        ));
        for (path, reason) in &failures {
            context.say_failure(path, reason);
        }
    } else {
        context.say(&format!(
            "{} file(s) would change. Nothing was written -- pass --apply to tag them.",
            changes.len()
        ));
    }
    Ok(0)
}
