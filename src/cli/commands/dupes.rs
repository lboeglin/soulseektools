//! `stk dupes` -- find the same track filed twice.

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{human_size, show, table, AppContext};
use crate::duplicates::{
    self, DuplicatePair, DEFAULT_AUDIO_THRESHOLD, DEFAULT_DURATION_TOLERANCE_SECONDS,
};
use crate::errors::Result;

#[derive(Subcommand, Debug)]
pub enum DupesCommand {
    /// Match on artist and title, across the whole library.
    Titles {
        /// Show at most this many pairs.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Match on the audio itself, whatever the files are named or tagged.
    Content {
        /// Minimum audio similarity.
        #[arg(long, default_value_t = DEFAULT_AUDIO_THRESHOLD)]
        threshold: f64,
        /// Duration difference, in seconds, still worth checking.
        #[arg(long, default_value_t = DEFAULT_DURATION_TOLERANCE_SECONDS)]
        tolerance: f64,
        /// Show at most this many pairs.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: DupesCommand) -> Result<i32> {
    match command {
        DupesCommand::Titles { limit } => titles(context, limit),
        DupesCommand::Content {
            threshold,
            tolerance,
            limit,
        } => content(context, threshold, tolerance, limit),
    }
}

fn titles(context: &AppContext, limit: usize) -> Result<i32> {
    let entries = context.open_index()?.entries()?;
    if !require_index(context, entries.len()) {
        return Ok(0);
    }

    let pairs = duplicates::find_by_title(&entries, &context.settings().matching.thresholds());
    report(context, &pairs, limit, entries.len());
    Ok(0)
}

fn content(context: &AppContext, threshold: f64, tolerance: f64, limit: usize) -> Result<i32> {
    let mut index = context.open_index()?;
    let entries = index.entries()?;
    if !require_index(context, entries.len()) {
        return Ok(0);
    }

    let mut source = duplicates::cached_fingerprints(&mut index);
    let pairs = duplicates::find_by_audio(&entries, threshold, tolerance, &mut source);

    report(context, &pairs, limit, entries.len());
    Ok(0)
}

fn require_index(context: &AppContext, count: usize) -> bool {
    if count > 0 {
        return true;
    }
    context.say("Index is empty. Run stk library scan first.");
    false
}

fn report(context: &AppContext, pairs: &[DuplicatePair], limit: usize, scanned: usize) {
    if context.as_json {
        context.emit(&json!(pairs
            .iter()
            .map(|pair| json!({
                "score": round(pair.score, 3),
                "evidence": pair.evidence,
                "keep": show(&pair.larger().path),
                "other": show(&pair.smaller().path),
                "keep_bytes": pair.larger().size,
                "other_bytes": pair.smaller().size,
            }))
            .collect::<Vec<_>>()));
        return;
    }

    if pairs.is_empty() {
        context.say(&format!("No duplicates found across {scanned} file(s)."));
        return;
    }

    let mut rows = Vec::new();
    for pair in pairs.iter().take(limit) {
        rows.push(vec![
            format!("{:.2}", pair.score),
            human_size(pair.larger().size),
            show(&pair.larger().path),
        ]);
        rows.push(vec![
            String::new(),
            human_size(pair.smaller().size),
            show(&pair.smaller().path),
        ]);
        rows.push(vec![String::new(), String::new(), pair.evidence.clone()]);
    }
    context.say(&table(&["Score", "Size", "File"], &rows));

    let shown = limit.min(pairs.len());
    let suffix = if shown < pairs.len() {
        format!(" (showing {shown})")
    } else {
        String::new()
    };
    context.say(&format!(
        "{} likely duplicate pair(s) across {scanned} file(s){suffix}",
        pairs.len()
    ));
    context.say("Nothing was deleted. Verify each pair before removing anything.");
}

/// Python's `round(value, 3)`, which breaks ties to even.
fn round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}
