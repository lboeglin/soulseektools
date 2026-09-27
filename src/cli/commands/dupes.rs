//! `stk dupes` -- find the same track filed twice, and keep one copy of it.

use std::collections::HashMap;
use std::path::Path;

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{human_size, show, table, unix_now, AppContext};
use crate::duplicates::{
    self, DuplicateGroup, DuplicatePair, Keep, Resolution, DEFAULT_AUDIO_THRESHOLD,
    DEFAULT_DURATION_TOLERANCE_SECONDS, RESOLVE_COMMAND,
};
use crate::errors::Result;
use crate::library::changes::ChangeLog;
use crate::organise::{Applied, PlannedMove};

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
    /// Keep one copy of each duplicated track and quarantine the rest.
    ///
    /// Both passes run, and the copies not kept are moved beside the index
    /// rather than deleted, so `stk changes undo --last --apply` puts them back.
    Resolve {
        /// Which copy stays: largest, lossless, oldest or tagged.
        #[arg(long, default_value_t = Keep::default())]
        keep: Keep,
        /// Minimum audio similarity.
        #[arg(long, default_value_t = DEFAULT_AUDIO_THRESHOLD)]
        threshold: f64,
        /// Duration difference, in seconds, still worth checking.
        #[arg(long, default_value_t = DEFAULT_DURATION_TOLERANCE_SECONDS)]
        tolerance: f64,
        /// Move the files. Without this, nothing is modified.
        #[arg(long)]
        apply: bool,
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
        DupesCommand::Resolve {
            keep,
            threshold,
            tolerance,
            apply,
        } => resolve(
            context,
            &Resolve {
                keep,
                threshold,
                tolerance,
                apply,
            },
        ),
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
    count > 0 || !context.nothing_to_do("Index is empty. Run stk library scan first.")
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
    context.say(
        "Nothing was moved. stk dupes resolve keeps one copy of each and quarantines the rest.",
    );
}

/// Round to `digits` places, ties to even.
fn round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

struct Resolve {
    keep: Keep,
    threshold: f64,
    tolerance: f64,
    apply: bool,
}

fn resolve(context: &AppContext, options: &Resolve) -> Result<i32> {
    let mut index = context.open_index()?;
    let entries = index.entries()?;
    if !require_index(context, entries.len()) {
        return Ok(0);
    }

    let settings = context.settings();
    let mut pairs = duplicates::find_by_title(&entries, &settings.matching.thresholds());
    pairs.extend(duplicates::find_by_audio(
        &entries,
        options.threshold,
        options.tolerance,
        &mut duplicates::cached_fingerprints(&mut index),
    ));

    let min_score = settings.dupes.resolve_min_score;
    let (sure, doubtful): (Vec<DuplicateGroup>, Vec<DuplicateGroup>) = duplicates::group(pairs)
        .into_iter()
        .partition(|group| group.lowest_score() >= min_score);
    let resolutions: Vec<Resolution> = sure
        .iter()
        .map(|group| duplicates::choose(group, options.keep))
        .collect();

    let roots = context.library_roots();
    let log = ChangeLog::open(&index, RESOLVE_COMMAND, unix_now())?;
    let directory = duplicates::quarantine_directory(&index, log.run_id());
    let planned = duplicates::plan_quarantine(&resolutions, &roots, &directory);
    let ready: Vec<&PlannedMove> = planned.iter().filter(|move_| move_.ready()).collect();

    let applied = if options.apply {
        duplicates::quarantine(&mut index, &log, &ready, &roots, &settings.organise)?
    } else {
        Applied::default()
    };

    let plan = Plan {
        groups: &sure,
        resolutions: &resolutions,
        planned: planned
            .iter()
            .map(|move_| (move_.entry.path.as_path(), move_))
            .collect(),
        doubtful: &doubtful,
        directory: &directory,
    };
    if context.as_json {
        context.emit(&resolve_json(&plan, options, min_score, &applied));
    } else {
        resolve_report(context, &plan, options, min_score, ready.len(), &applied);
    }
    Ok(0)
}

struct Plan<'a> {
    groups: &'a [DuplicateGroup],
    resolutions: &'a [Resolution],
    planned: HashMap<&'a Path, &'a PlannedMove>,
    doubtful: &'a [DuplicateGroup],
    directory: &'a Path,
}

fn failed_rows(applied: &Applied) -> Vec<serde_json::Value> {
    applied
        .failures
        .iter()
        .map(|(path, error)| json!({"path": show(path), "error": error}))
        .collect()
}

fn resolve_json(
    plan: &Plan<'_>,
    options: &Resolve,
    min_score: f64,
    applied: &Applied,
) -> serde_json::Value {
    let evidence = |group: &DuplicateGroup| {
        group
            .pairs
            .iter()
            .map(|pair| pair.evidence.clone())
            .collect::<Vec<_>>()
    };
    json!({
        "keep": options.keep.as_str(),
        "min_score": min_score,
        "quarantine": show(plan.directory),
        "groups": plan.groups.iter().zip(plan.resolutions).map(|(group, resolution)| json!({
            "score": round(group.lowest_score(), 3),
            "evidence": evidence(group),
            "keep": show(&resolution.keeper.path),
            "losers": resolution.losers.iter().map(|loser| {
                let planned = plan.planned.get(loser.entry.path.as_path());
                json!({
                    "path": show(&loser.entry.path),
                    "bytes": loser.entry.size,
                    "reason": loser.reason,
                    "to": planned.map(|move_| show(&move_.destination)),
                    "blocked": planned
                        .and_then(|move_| move_.blocked.as_ref())
                        .map(ToString::to_string),
                })
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "skipped": plan.doubtful.iter().map(|group| json!({
            "score": round(group.lowest_score(), 3),
            "evidence": evidence(group),
            "files": group.entries.iter().map(|entry| show(&entry.path)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "applied": options.apply,
        "failed": failed_rows(applied),
    })
}

fn resolve_report(
    context: &AppContext,
    plan: &Plan<'_>,
    options: &Resolve,
    min_score: f64,
    ready: usize,
    applied: &Applied,
) {
    if plan.groups.is_empty() && plan.doubtful.is_empty() {
        context.say("No duplicates found.");
        return;
    }

    let mut rows = Vec::new();
    let mut blocked = 0;
    for (group, resolution) in plan.groups.iter().zip(plan.resolutions) {
        rows.push(vec![
            format!("{:.2}", group.lowest_score()),
            human_size(resolution.keeper.size),
            show(&resolution.keeper.path),
            "keep".to_owned(),
        ]);
        for loser in &resolution.losers {
            let action = match plan
                .planned
                .get(loser.entry.path.as_path())
                .and_then(|move_| move_.blocked.as_ref())
            {
                Some(obstacle) => {
                    blocked += 1;
                    format!("blocked: {obstacle}")
                }
                None => format!("quarantine: {}", loser.reason),
            };
            rows.push(vec![
                String::new(),
                human_size(loser.entry.size),
                show(&loser.entry.path),
                action,
            ]);
        }
        for pair in &group.pairs {
            rows.push(vec![
                String::new(),
                String::new(),
                pair.evidence.clone(),
                String::new(),
            ]);
        }
    }
    if !rows.is_empty() {
        context.say(&table(&["Score", "Size", "File", "Action"], &rows));
    }

    if !plan.doubtful.is_empty() {
        let rows: Vec<Vec<String>> = plan
            .doubtful
            .iter()
            .flat_map(|group| {
                group.entries.iter().enumerate().map(|(position, entry)| {
                    vec![
                        if position == 0 {
                            format!("{:.2}", group.lowest_score())
                        } else {
                            String::new()
                        },
                        show(&entry.path),
                    ]
                })
            })
            .collect();
        context.say(&format!(
            "Left alone, a pair scoring below dupes.resolve_min_score ({min_score:.2}):"
        ));
        context.say(&table(&["Score", "File"], &rows));
    }

    context.say(&format!(
        "{} group(s) resolved keeping the {} copy; {ready} file(s) to quarantine; \
         {blocked} blocked; {} group(s) left alone",
        plan.groups.len(),
        options.keep,
        plan.doubtful.len()
    ));

    if options.apply {
        context.say(&format!(
            "Moved {} file(s) to {}",
            applied.moved,
            show(plan.directory)
        ));
        if applied.pruned > 0 {
            context.say(&format!("Removed {} empty directory(ies)", applied.pruned));
        }
        for (path, reason) in &applied.failures {
            context.say_failure(&show(path), reason);
        }
        if applied.moved > 0 {
            context.say("stk changes undo --last --apply puts them back.");
        }
    } else if ready > 0 {
        context.say(&format!(
            "Nothing was moved -- pass --apply to quarantine them in {}.",
            show(plan.directory)
        ));
    }
}
