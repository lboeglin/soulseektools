//! `stk loudness` -- measure how loud files are and write the ReplayGain tags.

use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;

use crate::audio::loudness::{measure_album, LoudnessResult};
use crate::cli::context::{show, table, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::library::changes::ChangeLog;
use crate::library::scan::{iter_audio_files, Excludes, TAGGABLE_EXTENSIONS};
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
/// A file that already carries a gain is left alone unless `--force` says
/// otherwise: a library scanned in foobar2000, `rsgain` or `loudgain` carries
/// figures somebody chose, and replacing them with a differently configured
/// measurement is a silent regression for whoever owns it.
pub fn run(context: &AppContext, args: &LoudnessArgs) -> Result<i32> {
    let paths = targets(context, args.target.as_deref())?;
    if paths.is_empty() {
        context.say("No taggable files to scan.");
        return Ok(0);
    }

    let (targets, unreadable) = inspect(&paths);
    let selection = replaygain::select(replaygain::group(targets, !args.track_only), args.force);

    let mut failures: Vec<(String, String)> = unreadable;
    let scanned = scan(context, &selection, &mut failures);

    let changes: Vec<GainChange> = replaygain::plan(&scanned, &context.settings().loudness)
        .into_iter()
        .filter(GainChange::is_change)
        .collect();

    if args.apply {
        apply(context, &changes, &mut failures)?;
    }

    if context.as_json {
        emit(context, &changes, &selection, &failures, args.apply);
        return Ok(0);
    }

    report(context, &changes, &selection, &failures, args.apply);
    Ok(0)
}

/// Read what each file says it is and what it already carries.
///
/// One header parse apiece -- nothing next to the full decode a measurement
/// costs -- and it is read from the file rather than the index so a folder the
/// library has never seen still scans.
fn inspect(paths: &[PathBuf]) -> (Vec<Target>, Vec<(String, String)>) {
    let mut targets = Vec::new();
    let mut failed = Vec::new();
    for path in paths {
        match tags::describe_file(path) {
            Ok(described) => targets.push(Target {
                path: path.clone(),
                tags: described.tags,
                current: described.replay_gain,
                r128: tags::carries_r128(path),
            }),
            Err(error) => failed.push((show(path), error.to_string())),
        }
    }
    (targets, failed)
}

/// Measure one record at a time.
///
/// A record is one programme to EBU R 128, so its tracks are scanned together;
/// the analyser states that makes the exact album figure from never outlive the
/// group they belong to, which is what keeps a library-wide run's memory to one
/// album rather than every file.
fn scan(
    context: &AppContext,
    selection: &Selection,
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
            // many of them share a folder.
            album: group.album.as_ref().and(measured.album),
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

fn apply(
    context: &AppContext,
    changes: &[GainChange],
    failures: &mut Vec<(String, String)>,
) -> Result<()> {
    let roots = context.library_roots();
    let mut index = context.open_index()?;
    let log = ChangeLog::open(&index, "loudness", unix_now())?;

    for change in changes {
        match tags::write_replay_gain(&change.path, &change.proposed) {
            Ok(()) => {
                log.replay_gain(&mut index, &change.path, &change.current, &change.proposed)?;
                index.reindex(&change.path, &roots)?;
            }
            Err(error) => failures.push((show(&change.path), error.to_string())),
        }
    }
    Ok(())
}

/// Only the six taggable containers, since nothing else can hold the result.
fn targets(context: &AppContext, target: Option<&Path>) -> Result<Vec<PathBuf>> {
    match target {
        None => {
            let exclude = context.excludes()?;
            Ok(context
                .require_library_roots()?
                .iter()
                .flat_map(|root| iter_audio_files(root, TAGGABLE_EXTENSIONS, &exclude))
                .collect())
        }
        Some(path) if path.is_file() => Ok(vec![path.to_path_buf()]),
        Some(path) if path.is_dir() => Ok(iter_audio_files(
            path,
            TAGGABLE_EXTENSIONS,
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
    changes: &[GainChange],
    selection: &Selection,
    failures: &[(String, String)],
    applied: bool,
) {
    context.emit(&json!({
        "planned": changes
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
        "applied": applied,
        "failed": failures
            .iter()
            .map(|(path, error)| json!({"path": path, "error": error}))
            .collect::<Vec<_>>(),
    }));
}

fn report(
    context: &AppContext,
    changes: &[GainChange],
    selection: &Selection,
    failures: &[(String, String)],
    applied: bool,
) {
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

    if changes.is_empty() {
        // Two different nothings: a run with no work to do, and a `--force`
        // re-measurement that agreed with what the files already carried.
        context.say(if selection.wanted.is_empty() {
            "Nothing to measure."
        } else {
            "Every file measured already carries the figures this scan found."
        });
    } else if applied {
        context.say(&format!(
            "Wrote loudness tags to {} file(s)",
            changes.len() - failures.len()
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
