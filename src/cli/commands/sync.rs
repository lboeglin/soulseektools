//! `stk sync` -- the recurring loop as one command (#14).
//!
//! Keeping a library in step with a growing export is six commands in a fixed
//! order: `library scan`, `wantlist diff --new`, `fetch search`, `review pick`,
//! `fetch download`, `organise --from <download dir>`. Each exists and works
//! alone; this runs them in that order and adds the one step that had no command
//! at all, waiting for the downloads to finish.
//!
//! The steps are the library calls those commands wrap, not the commands
//! themselves, so the report is one document rather than six interleaved ones.
//! One `--apply` gates the whole run. Without it the steps that only touch the
//! index and the review queue run -- as their own commands do without a flag --
//! and the rest is described: no file on disk changes.

use std::path::PathBuf;

use clap::Args;
use serde_json::json;

use crate::cli::commands::library::{self, Scanned};
use crate::cli::commands::{fetch as fetch_command, organise as organise_command};
use crate::cli::context::{show, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::fetch::{self, Patience, SearchOutcome, SystemClock, Waited};
use crate::library::index::LibraryIndex;
use crate::models::TrackStatus;
use crate::organise::{self, Applied, Pattern, PlannedMove};
use crate::review::picker;
use crate::review::store::{ReviewStore, ReviewTrack};
use crate::soulseek::base::SoulseekClient;
use crate::wantlist;

/// How often the wait looks at slskd's transfer list.
const POLL_SECONDS: f64 = 15.0;

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// CSV exported from a streaming service.
    pub export: PathBuf,

    /// Download and organise. Without this, only the index and the review queue change.
    #[arg(long)]
    pub apply: bool,

    /// Take the best candidate for every track instead of asking.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Search, and download, at most this many tracks.
    #[arg(long)]
    pub limit: Option<usize>,

    /// Stop waiting for downloads after this long [default: twice slskd.stall_after_seconds].
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<f64>,
}

/// What the diff found, and how much of it the queue had not seen.
struct Diffed {
    total: usize,
    missing: usize,
    owned: usize,
    queued: usize,
}

/// Where downloads land, and where organising sends them.
struct Target {
    downloads: PathBuf,
    destination: PathBuf,
}

/// The organise step: its plan, and what carrying it out did.
struct Organised {
    planned: Vec<PlannedMove>,
    applied: Applied,
}

impl Organised {
    fn ready(&self) -> Vec<&PlannedMove> {
        self.planned.iter().filter(|move_| move_.ready()).collect()
    }
}

/// The download step: what was waited for, or without `--apply`, what would be.
enum Download {
    Waited(Waited),
    Would(Vec<ReviewTrack>),
}

/// Every step's result, reported together once the run is over.
struct Synced {
    applied: bool,
    scanned: Scanned,
    diffed: Diffed,
    searched: Vec<SearchOutcome>,
    approved: usize,
    download: Download,
    organised: Option<Organised>,
    rescanned: Option<Scanned>,
}

pub fn run(context: &AppContext, args: &SyncArgs) -> Result<i32> {
    // Refused as `review pick` refuses it: under `--json` the prompt would be
    // invisible and a pipeline would wait on a keystroke nobody knew to give.
    if context.as_json && !args.yes {
        eprintln!("error: stk sync asks a question per track and cannot run with --json alone");
        eprintln!("hint: pass --yes to take the best candidate for every track");
        return Ok(1);
    }

    let target = target(context, args.apply)?;
    let pattern = Pattern::parse(&context.settings().organise.pattern)?;
    let export = match wantlist::read_export(&args.export) {
        Ok(tracks) => tracks,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(1);
        }
    };
    let settings = context.settings();
    let roots = context.library_roots();

    let scanned = library::refresh(context, false)?;
    scanned.report(context);

    let entries = context.open_index()?.entries()?;
    let difference = wantlist::diff(&export, &entries, &settings.matching.thresholds());
    let mut store = context.open_review()?;
    let diffed = Diffed {
        total: difference.total(),
        missing: difference.missing.len(),
        owned: difference.owned.len(),
        queued: store.add(&difference.missing)?,
    };
    context.say(&format!(
        "{} of {} track(s) missing, {} owned; {} new to the queue",
        diffed.missing, diffed.total, diffed.owned, diffed.queued
    ));

    let client = fetch_command::client(&settings.slskd)?;
    let searched = if store
        .tracks(&[TrackStatus::Pending], args.limit)?
        .is_empty()
    {
        Vec::new()
    } else {
        fetch::search_pending(&mut store, &client, settings, args.limit)?
    };
    if !searched.is_empty() {
        fetch_command::report_searched(context, &searched);
    }

    let approved = pick(context, &mut store, args.yes)?;

    let mut index = context.open_index()?;
    let download = if args.apply {
        let timeout = args
            .timeout
            .unwrap_or(2.0 * settings.slskd.stall_after_seconds);
        Download::Waited(wait(
            context, &mut store, &client, &mut index, &roots, args.limit, timeout,
        )?)
    } else {
        Download::Would(store.tracks(&[TrackStatus::Approved], args.limit)?)
    };
    let organised = organise_downloads(
        context,
        &mut index,
        target.as_ref(),
        &pattern,
        &roots,
        args.apply,
    )?;
    drop(index);
    let rescanned = if args.apply {
        Some(library::refresh(context, false)?)
    } else {
        None
    };

    let synced = Synced {
        applied: args.apply,
        scanned,
        diffed,
        searched,
        approved,
        download,
        organised,
        rescanned,
    };
    if context.as_json {
        context.emit(&synced.to_json());
    } else {
        synced.report(context);
    }
    Ok(i32::from(synced.went_wrong()))
}

/// Where the organise step reads from and writes to.
///
/// Settled before anything runs, so a layout the organise step would refuse is
/// not discovered after an hour of waiting for downloads.
fn target(context: &AppContext, apply: bool) -> Result<Option<Target>> {
    let Some(downloads) = context.settings().slskd.download_directory.clone() else {
        if apply {
            return Err(Error::Config(
                "sync --apply has nowhere to organise downloads from\n\
                 hint: set slskd.download_directory to where slskd saves them"
                    .to_owned(),
            ));
        }
        return Ok(None);
    };
    let roots = context.library_roots();
    let destination = organise_command::where_to(context, &roots)?;
    organise_command::inside_a_root(&destination, &roots)?;
    Ok(Some(Target {
        downloads,
        destination,
    }))
}

impl Synced {
    /// A search that failed, a download that failed or is fake, or a file that
    /// would not move: what `verify` and `fetch` would each own up to.
    fn went_wrong(&self) -> bool {
        self.searched
            .iter()
            .any(|outcome| !outcome.error.is_empty())
            || matches!(&self.download, Download::Waited(waited) if waited.went_wrong())
            || self
                .organised
                .as_ref()
                .is_some_and(|organised| !organised.applied.failures.is_empty())
    }

    fn to_json(&self) -> serde_json::Value {
        json!({
            "applied": self.applied,
            "scan": self.scanned.to_json(),
            "diff": {
                "total": self.diffed.total,
                "missing": self.diffed.missing,
                "owned": self.diffed.owned,
                "queued": self.diffed.queued,
            },
            "search": self.searched.iter().map(fetch_command::search_row).collect::<Vec<_>>(),
            "pick": {"approved": self.approved},
            "download": match &self.download {
                Download::Waited(waited) => download_json(waited),
                Download::Would(approved) => json!({
                    "would_queue": approved
                        .iter()
                        .map(|entry| json!({"track_id": entry.id, "query": entry.track.query()}))
                        .collect::<Vec<_>>(),
                }),
            },
            "organise": self.organised.as_ref().map(organise_json),
            "rescan": self.rescanned.as_ref().map(Scanned::to_json),
        })
    }

    /// The steps after the pick; the ones before it report as they run, since
    /// the pick has to come after them on screen.
    fn report(&self, context: &AppContext) {
        match &self.download {
            Download::Waited(waited) => report_download(context, waited),
            Download::Would(approved) if approved.is_empty() => {
                context.say("Nothing approved to download.");
            }
            Download::Would(approved) => context.say(&format!(
                "{} approved track(s) would be queued and waited for.",
                approved.len()
            )),
        }
        if let Some(organised) = &self.organised {
            let failures: Vec<(String, String)> = organised
                .applied
                .failures
                .iter()
                .map(|(path, error)| (show(path), error.clone()))
                .collect();
            organise_command::report(
                context,
                &organised.planned,
                &organised.ready(),
                &failures,
                self.applied,
                organised.applied.pruned,
            );
        }
        match &self.rescanned {
            Some(rescanned) => rescanned.report(context),
            None => context.say(
                "Nothing was downloaded or moved -- pass --apply to download, organise and scan again.",
            ),
        }
    }
}

/// Choose a candidate for every track awaiting review, or ask for each.
fn pick(context: &AppContext, store: &mut ReviewStore, yes: bool) -> Result<usize> {
    if yes {
        let waiting = store.tracks(&[TrackStatus::AwaitingReview], None)?;
        let mut approved = 0;
        for entry in &waiting {
            if store.approve(entry.id, 1).is_ok() {
                approved += 1;
            }
        }
        context.say(&format!("Took the best candidate for {approved} track(s)."));
        return Ok(approved);
    }

    let tally = picker::walk(
        store,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )?;
    if tally.waiting > 0 {
        context.say(&format!(
            "\n{} approved, {} rejected, {} left to review.",
            tally.approved,
            tally.rejected,
            tally.waiting - tally.approved - tally.rejected
        ));
    }
    Ok(tally.approved)
}

fn wait(
    context: &AppContext,
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    index: &mut LibraryIndex,
    roots: &[PathBuf],
    limit: Option<usize>,
    timeout: f64,
) -> Result<Waited> {
    // A line only when something moved: a poll every few seconds over an hour
    // saying nothing new is noise.
    let mut last = (usize::MAX, usize::MAX);
    fetch::wait_for_downloads(
        store,
        client,
        context.settings(),
        index,
        roots,
        limit,
        Patience {
            timeout_seconds: timeout,
            poll_seconds: POLL_SECONDS,
        },
        &mut SystemClock,
        &mut |waited| {
            let now = (waited.collected.len(), waited.unfinished);
            if now != last {
                context.say(&format!(
                    "Waiting: {} finished, {} still on their way",
                    now.0, now.1
                ));
                last = now;
            }
        },
    )
}

/// Plan, and with `apply` carry out, moving what is in the download directory.
///
/// A download directory that does not exist yet holds nothing to organise.
fn organise_downloads(
    context: &AppContext,
    index: &mut LibraryIndex,
    target: Option<&Target>,
    pattern: &Pattern,
    roots: &[PathBuf],
    apply: bool,
) -> Result<Option<Organised>> {
    let Some(Target {
        downloads,
        destination,
    }) = target
    else {
        return Ok(None);
    };
    let settings = &context.settings().organise;
    let entries = if downloads.is_dir() {
        let extensions: Vec<&str> = context
            .settings()
            .library
            .extensions
            .iter()
            .map(String::as_str)
            .collect();
        organise::loose_entries(downloads, &extensions, &context.excludes()?)?
    } else {
        Vec::new()
    };

    let planned = organise::plan_moves(
        &entries,
        pattern,
        settings,
        roots,
        Some(destination.as_path()),
    );
    let applied = if apply {
        let ready: Vec<&PlannedMove> = planned.iter().filter(|move_| move_.ready()).collect();
        organise::apply_moves(index, &ready, roots, settings, unix_now())?
    } else {
        Applied::default()
    };
    Ok(Some(Organised { planned, applied }))
}

fn download_json(waited: &Waited) -> serde_json::Value {
    json!({
        "stalled": waited.stalled.iter().map(fetch_command::stall_row).collect::<Vec<_>>(),
        "queued": waited.queued.iter().map(fetch_command::enqueue_row).collect::<Vec<_>>(),
        "collected": waited.collected.iter().map(fetch_command::collect_row).collect::<Vec<_>>(),
        "rounds": waited.rounds,
        "unfinished": waited.unfinished,
    })
}

fn organise_json(organised: &Organised) -> serde_json::Value {
    json!({
        "planned": organised.ready().iter().map(|planned| json!({
            "from": show(&planned.entry.path),
            "to": show(&planned.destination),
        })).collect::<Vec<_>>(),
        "blocked": organised
            .planned
            .iter()
            .filter_map(organise_command::blocked_row)
            .collect::<Vec<_>>(),
        "failed": organised
            .applied
            .failures
            .iter()
            .map(|(path, error)| json!({"path": show(path), "error": error}))
            .collect::<Vec<_>>(),
    })
}

fn report_download(context: &AppContext, waited: &Waited) {
    fetch_command::report_stalled(context, &waited.stalled);
    fetch_command::report_queued(context, &waited.queued);
    fetch_command::report_collected(context, &waited.collected);
    context.say(&format!(
        "Queued {} track(s); recorded {} finished transfer(s); gave up on {} stalled one(s)",
        waited
            .queued
            .iter()
            .filter(|outcome| outcome.queued)
            .count(),
        waited.collected.len(),
        waited.stalled.len()
    ));
    if waited.unfinished > 0 {
        context.say(&format!(
            "Stopped waiting with {} still on their way; the next sync or \
             stk fetch download --collect records them.",
            waited.unfinished
        ));
    }
}
