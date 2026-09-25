//! `stk loudness` -- measure how loud files are and write the ReplayGain tags.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;

use crate::audio::loudness::{measure_album, LoudnessResult};
use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::library::changes::ChangeLog;
use crate::library::scan::{iter_audio_files, Excludes, MEASURABLE_EXTENSIONS};
use crate::library::tags;
use crate::replaygain::{self, GainChange, Scanned, Selection, Target};

#[derive(Args, Debug)]
pub struct LoudnessArgs {
    /// File or folder to scan. Defaults to the whole library.
    pub target: Option<PathBuf>,

    /// Write the tags. Without this, nothing is modified.
    #[arg(long)]
    pub apply: bool,

    /// Measure files that already carry a gain, replacing what they hold.
    #[arg(long)]
    pub force: bool,

    /// Write only the track gain, leaving albums ungrouped.
    #[arg(long)]
    pub track_only: bool,
}

/// Measure every target and show what its loudness tags would become.
///
/// A file that already carries a gain is left alone unless `--force`.
pub fn run(context: &AppContext, args: &LoudnessArgs) -> Result<i32> {
    let paths = targets(context, args.target.as_deref())?;
    if paths.is_empty() && context.nothing_to_do("No taggable files to scan.") {
        return Ok(0);
    }

    let (targets, unreadable) = inspect(&paths);
    let groups = replaygain::group(targets, !args.track_only);
    let partial = replaygain::partial_records(
        &groups,
        &replaygain::album_counts(&context.open_index()?.entries()?),
    );
    let selection = replaygain::select(groups, args.force);

    let mut failures: Vec<(String, String)> = unreadable;
    let scanned = scan(context, &selection, &partial, &mut failures);

    let changes: Vec<GainChange> = replaygain::plan(&scanned, &context.settings().loudness)
        .into_iter()
        .filter(GainChange::is_change)
        .collect();

    let mut written = 0;
    if args.apply {
        written = apply(context, &changes, &mut failures)?;
    }

    let outcome = Outcome {
        changes,
        partial,
        written,
        applied: args.apply,
    };
    if context.as_json {
        emit(context, &outcome, &selection, &failures);
        return Ok(0);
    }

    report(context, &outcome, &selection, &failures);
    Ok(0)
}

/// What a run did, as both reports read it.
struct Outcome {
    changes: Vec<GainChange>,
    /// Records this run held only part of, which therefore get no album gain.
    partial: HashSet<(String, String)>,
    /// Files actually written, which is not `changes` less `failures`: a file
    /// that could not be measured never became a change in the first place.
    written: usize,
    applied: bool,
}

/// Read what each file says it is and what it already carries.
///
/// Read from the file, not the index, so an unindexed folder still scans.
fn inspect(paths: &[PathBuf]) -> (Vec<Target>, Vec<(String, String)>) {
    let mut targets = Vec::new();
    let mut failed = Vec::new();
    for path in paths {
        match tags::describe_file(path) {
            Ok(described) => targets.push(Target {
                path: path.clone(),
                tags: described.tags,
                current: described.replay_gain,
                r128: described.r128,
            }),
            Err(error) => failed.push((show(path), error.to_string())),
        }
    }
    (targets, failed)
}

/// Measure one record at a time.
///
/// A record's tracks are scanned together, one record at a time.
fn scan(
    context: &AppContext,
    selection: &Selection,
    partial: &HashSet<(String, String)>,
    failures: &mut Vec<(String, String)>,
) -> Vec<Scanned> {
    let settings = &context.settings().loudness;
    let mut scanned = Vec::new();
    for group in &selection.wanted {
        let paths: Vec<PathBuf> = group
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect();
        let measured = measure_album(&paths, settings);

        failures.extend(
            measured
                .tracks
                .iter()
                .filter(|result| result.loudness.is_none())
                .map(|result| (show(&result.path), reason(result))),
        );
        scanned.push(Scanned {
            // A group of files that name no album is not a record, however
            // many of them share a folder -- and neither is part of one.
            album: group
                .album
                .as_ref()
                .filter(|key| !partial.contains(*key))
                .and(measured.album),
            tracks: group
                .targets
                .iter()
                .cloned()
                .zip(measured.tracks.iter().map(|result| result.loudness))
                .collect(),
        });
    }
    scanned
}

/// Write the planned tags, returning how many files were actually written.
fn apply(
    context: &AppContext,
    changes: &[GainChange],
    failures: &mut Vec<(String, String)>,
) -> Result<usize> {
    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let log = ChangeLog::open(&index, "loudness", unix_now())?;

    let mut written = 0;
    for change in changes {
        match tags::write_replay_gain(&change.path, &change.proposed) {
            Ok(()) => {
                log.replay_gain(&mut index, &change.path, &change.current, &change.proposed)?;
                index.reindex(&change.path, &roots)?;
                written += 1;
            }
            Err(error) => failures.push((show(&change.path), error.to_string())),
        }
    }
    Ok(written)
}

/// Only the containers a measurement can be both taken from and written to.
fn targets(context: &AppContext, target: Option<&Path>) -> Result<Vec<PathBuf>> {
    match target {
        None => {
            let exclude = context.excludes()?;
            Ok(context
                .require_library_roots()?
                .iter()
                .flat_map(|root| iter_audio_files(root, MEASURABLE_EXTENSIONS, &exclude))
                .collect())
        }
        Some(path) if path.is_file() => Ok(vec![path.to_path_buf()]),
        Some(path) if path.is_dir() => Ok(iter_audio_files(
            path,
            MEASURABLE_EXTENSIONS,
            &Excludes::default(),
        )),
        Some(path) => Err(Error::Config(format!(
            "no such file or folder: {}",
            show(path)
        ))),
    }
}

fn reason(result: &LoudnessResult) -> String {
    if result.detail.is_empty() {
        "no measurable loudness".to_owned()
    } else {
        result.detail.clone()
    }
}

fn emit(
    context: &AppContext,
    outcome: &Outcome,
    selection: &Selection,
    failures: &[(String, String)],
) {
    context.emit(&json!({
        "planned": outcome
            .changes
            .iter()
            .map(|change| json!({
                "path": show(&change.path),
                "track_gain": change.proposed.track_gain,
                "track_peak": change.proposed.track_peak,
                "album_gain": change.proposed.album_gain,
                "album_peak": change.proposed.album_peak,
                "r128_track_gain": change.proposed.r128_track_gain,
                "r128_album_gain": change.proposed.r128_album_gain,
            }))
            .collect::<Vec<_>>(),
        "skipped": selection.skipped.iter().map(|target| show(&target.path)).collect::<Vec<_>>(),
        "partial_albums": outcome
            .partial
            .iter()
            .map(|(artist, album)| json!([artist, album]))
            .collect::<Vec<_>>(),
        "applied": outcome.applied,
        "written": outcome.written,
        "failed": failures
            .iter()
            .map(|(path, error)| json!({"path": path, "error": error}))
            .collect::<Vec<_>>(),
    }));
}

fn report(
    context: &AppContext,
    outcome: &Outcome,
    selection: &Selection,
    failures: &[(String, String)],
) {
    let changes = &outcome.changes;
    if !changes.is_empty() {
        let rows: Vec<Vec<String>> = changes
            .iter()
            .take(50)
            .map(|change| {
                vec![
                    show(&change.path),
                    change
                        .current
                        .track_gain
                        .clone()
                        .unwrap_or_else(|| "none".to_owned()),
                    change.proposed.track_gain.clone().unwrap_or_default(),
                    change
                        .proposed
                        .album_gain
                        .clone()
                        .unwrap_or_else(|| "-".to_owned()),
                ]
            })
            .collect();
        context.say(&table(&["File", "From", "Track gain", "Album gain"], &rows));
    }

    if !selection.skipped.is_empty() {
        context.say(&format!(
            "Left {} file(s) alone: they already carry a gain. Pass --force to measure them again.",
            selection.skipped.len()
        ));
    }

    if !outcome.partial.is_empty() {
        context.say(&format!(
            "Track gain only for {} record(s): this run covers part of each, and a figure \
             measured from part of a record is not an album gain.",
            outcome.partial.len()
        ));
    }

    if changes.is_empty() {
        // Two different nothings: a run with no work to do, and a `--force`
        // re-measurement that agreed with what the files already carried.
        context.say(if selection.wanted.is_empty() {
            "Nothing to measure."
        } else {
            "Every file measured already carries the figures this scan found."
        });
    } else if outcome.applied {
        context.say(&format!(
            "Wrote loudness tags to {} file(s)",
            outcome.written
        ));
    } else {
        context.say(&format!(
            "{} file(s) would change. Nothing was written -- pass --apply to tag them.",
            changes.len()
        ));
    }

    for (path, error) in failures {
        context.say(&format!("failed {path}: {error}"));
    }
}
