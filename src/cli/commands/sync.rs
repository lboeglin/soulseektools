//! `stk sync`: scan, diff, search, pick, download (waiting for it) and organise
//! in one run. Without `--apply` only the index and review queue change.

use std::path::PathBuf;

use clap::Args;
use serde_json::json;

use crate::artwork::service::{self, ArtworkResult};
use crate::cli::commands::library::{self, Scanned};
use crate::cli::commands::wantlist::ExportArgs;
use crate::cli::commands::{
    art as art_command, fetch as fetch_command, organise as organise_command,
};
use crate::cli::context::{candidate_count, show, table, unix_now, AppContext};
use crate::errors::{Error, Result};
use crate::fetch::{self, CollectOutcome, Filing, Patience, SearchOutcome, SystemClock, Waited};
use crate::library::index::LibraryIndex;
use crate::models::{LibraryEntry, TrackStatus};
use crate::organise::{self, Applied, Pattern, PlannedMove};
use crate::retag::TagPolicy;
use crate::review::picker;
use crate::review::store::{ReviewStore, ReviewTrack};
use crate::soulseek::base::SoulseekClient;
use crate::wantlist;

const POLL_SECONDS: f64 = 15.0;

#[derive(Args, Debug)]
pub struct SyncArgs {
    #[command(flatten)]
    pub export: ExportArgs,

    /// Download and organise. Without this, only the index and the review queue change.
    #[arg(long)]
    pub apply: bool,

    /// Take the best candidate for every track instead of asking.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// With --yes, take the best candidate even when it is flagged.
    #[arg(long, requires = "yes")]
    pub force: bool,

    /// Offer this many candidates per track [default: review.candidates_per_track].
    #[arg(long, value_name = "N", value_parser = candidate_count)]
    pub candidates: Option<usize>,

    /// Search, and download, at most this many tracks.
    #[arg(long)]
    pub limit: Option<usize>,

    /// Stop waiting for downloads after this long [default: twice slskd.stall_after_seconds].
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<f64>,
}

struct Diffed {
    total: usize,
    missing: usize,
    owned: usize,
    queued: usize,
}

struct Target {
    downloads: PathBuf,
    destination: PathBuf,
}

struct Organised {
    planned: Vec<PlannedMove>,
    applied: Applied,
    covers: Vec<ArtworkResult>,
}

impl Organised {
    fn ready(&self) -> Vec<&PlannedMove> {
        self.planned.iter().filter(|move_| move_.ready()).collect()
    }
}

enum Download {
    Waited(Waited),
    Would(Vec<ReviewTrack>),
}

impl Download {
    fn collected(&self) -> &[CollectOutcome] {
        match self {
            Self::Waited(waited) => &waited.collected,
            Self::Would(_) => &[],
        }
    }
}

struct Synced {
    applied: bool,
    scanned: Scanned,
    diffed: Diffed,
    searched: Vec<SearchOutcome>,
    approved: usize,
    held: Vec<ReviewTrack>,
    download: Download,
    organised: Option<Organised>,
    rescanned: Option<Scanned>,
}

pub fn run(context: &AppContext, args: &SyncArgs) -> Result<i32> {
    // Under `--json` the prompt would be invisible.
    if context.as_json && !args.yes {
        eprintln!("error: stk sync asks a question per track and cannot run with --json alone");
        eprintln!("hint: pass --yes to take the best candidate for every track");
        return Ok(1);
    }

    let target = target(context, args.apply)?;
    let pattern = Pattern::parse(&context.settings().organise.pattern)?;
    let export = match args.export.read(context.settings()) {
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

    // slskd is only needed to search or download: a run with neither still
    // diffs and organises without it.
    let mut client = None;
    let searched = if store
        .tracks(&[TrackStatus::Pending], args.limit)?
        .is_empty()
    {
        Vec::new()
    } else {
        let client = client.insert(fetch_command::client(&settings.slskd)?);
        fetch::search_pending(&mut store, client, settings, args.limit)?
    };
    if !searched.is_empty() {
        fetch_command::report_searched(context, &searched);
    }

    let (approved, held) = pick(context, &mut store, args)?;

    let mut index = context.open_index()?;
    let download = if args.apply {
        let timeout = args
            .timeout
            .unwrap_or(2.0 * settings.slskd.stall_after_seconds);
        let in_flight = [TrackStatus::Approved, TrackStatus::Queued];
        if store.tracks(&in_flight, args.limit)?.is_empty() {
            Download::Waited(fetch::Waited::default())
        } else {
            let client = match client.take() {
                Some(client) => client,
                None => fetch_command::client(&settings.slskd)?,
            };
            let scope = fetch::Scope::Queue { limit: args.limit };
            let tags = settings.fetch.tags;
            Download::Waited(wait(
                context, &mut store, &client, &mut index, scope, tags, timeout,
            )?)
        }
    } else {
        Download::Would(store.tracks(&[TrackStatus::Approved], args.limit)?)
    };
    let organised = organise_downloads(
        context,
        &mut index,
        target.as_ref(),
        &pattern,
        &roots,
        download.collected(),
        Filing::Everything,
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
        held,
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

/// Settled up front, not after an hour of waiting for downloads.
fn target(context: &AppContext, apply: bool) -> Result<Option<Target>> {
    let Some(downloads) = context.settings().slskd.download_directory.clone() else {
        if apply {
            return Err(Error::Config(
                "nowhere to organise downloads from\n\
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
            "pick": {
                "approved": self.approved,
                "needs_a_look": self.held.iter().map(|entry| json!({
                    "track_id": entry.id,
                    "query": entry.track.query(),
                    "flags": entry.candidates.first().map(|best| best.flags.clone()),
                })).collect::<Vec<_>>(),
            },
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

    /// The steps before the pick report as they run, since the pick prompts after them.
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
            report_organised(context, organised, self.applied);
        }
        match &self.rescanned {
            Some(rescanned) => rescanned.report(context),
            None => context.say(
                "Nothing was downloaded or moved -- pass --apply to download, organise and scan again.",
            ),
        }
    }
}

fn report_organised(context: &AppContext, organised: &Organised, applied: bool) {
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
        applied,
        organised.applied.pruned,
    );
    if !organised.covers.is_empty() {
        art_command::report(context, &organised.covers, applied);
    }
}

/// `true` when something went wrong.
pub(crate) fn download_and_file(
    context: &AppContext,
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    ids: &[i64],
    tags: TagPolicy,
    timeout: f64,
) -> Result<bool> {
    let Some(target) = target(context, true)? else {
        return Ok(false);
    };
    let roots = context.library_roots();
    let pattern = Pattern::parse(&context.settings().organise.pattern)?;
    let mut index = context.open_index()?;

    let scope = fetch::Scope::Tracks(ids);
    let waited = wait(context, store, client, &mut index, scope, tags, timeout)?;
    report_download(context, &waited);
    let organised = organise_downloads(
        context,
        &mut index,
        Some(&target),
        &pattern,
        &roots,
        &waited.collected,
        Filing::Collected,
        true,
    )?;
    let mut failed = waited.went_wrong();
    if let Some(organised) = &organised {
        report_organised(context, organised, true);
        failed |= !organised.applied.failures.is_empty();
    }
    Ok(failed)
}

fn pick(
    context: &AppContext,
    store: &mut ReviewStore,
    args: &SyncArgs,
) -> Result<(usize, Vec<ReviewTrack>)> {
    if args.yes {
        let waiting = store.tracks(&[TrackStatus::AwaitingReview], None)?;
        let taken = picker::take_best(store, &waiting, args.force)?;
        context.say(&format!(
            "Took the best candidate for {} track(s).",
            taken.approved.len()
        ));
        report_held(context, &taken.held);
        return Ok((taken.approved.len(), taken.held));
    }

    let shown = args
        .candidates
        .unwrap_or(context.settings().review.candidates_per_track);
    let tally = picker::walk(
        store,
        shown,
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
    Ok((tally.approved, Vec::new()))
}

pub(crate) fn report_held(context: &AppContext, held: &[ReviewTrack]) {
    if held.is_empty() {
        return;
    }
    let rows: Vec<Vec<String>> = held
        .iter()
        .map(|entry| {
            let flags = entry
                .candidates
                .first()
                .map(|best| best.flags.join(", "))
                .unwrap_or_default();
            vec![entry.track.query(), flags]
        })
        .collect();
    context.say(&table(&["Needs a look", "Rank 1 flagged"], &rows));
    context.say(&format!(
        "Left {} track(s) for stk review pick; pass --force to take them anyway.",
        held.len()
    ));
}

fn wait(
    context: &AppContext,
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    index: &mut LibraryIndex,
    scope: fetch::Scope<'_>,
    tags: TagPolicy,
    timeout: f64,
) -> Result<Waited> {
    let mut settings = context.settings().clone();
    settings.fetch.tags = tags;
    // A line only when something changed.
    let mut last = (usize::MAX, usize::MAX);
    fetch::wait_for_downloads(
        store,
        client,
        &settings,
        index,
        &context.library_roots(),
        scope,
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

#[allow(clippy::too_many_arguments)]
fn organise_downloads(
    context: &AppContext,
    index: &mut LibraryIndex,
    target: Option<&Target>,
    pattern: &Pattern,
    roots: &[PathBuf],
    collected: &[CollectOutcome],
    filing: Filing,
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
    let entries = fetch::fileable(entries, collected, filing);

    let planned = organise::plan_moves(
        &entries,
        pattern,
        settings,
        roots,
        Some(destination.as_path()),
    );
    let (applied, covers) = if apply {
        let ready: Vec<&PlannedMove> = planned.iter().filter(|move_| move_.ready()).collect();
        let applied =
            organise::apply_moves(index, &ready, roots, Some(downloads), settings, unix_now())?;
        let filed: Vec<&PathBuf> = ready.iter().map(|move_| &move_.destination).collect();
        (applied, cover_arrivals(context, index, &filed)?)
    } else {
        (Applied::default(), Vec::new())
    };
    Ok(Some(Organised {
        planned,
        applied,
        covers,
    }))
}

/// A lookup service that can't be set up is reported, not fatal: the files
/// are already filed.
fn cover_arrivals(
    context: &AppContext,
    index: &mut LibraryIndex,
    filed: &[&PathBuf],
) -> Result<Vec<ArtworkResult>> {
    let settings = &context.settings().artwork;
    let arrived: Vec<LibraryEntry> = index
        .entries()?
        .into_iter()
        .filter(|entry| filed.contains(&&entry.path))
        .collect();
    let needing = art_command::needing_artwork(&arrived, settings);
    if needing.is_empty() {
        return Ok(Vec::new());
    }
    match service::build_providers(settings) {
        Ok(providers) => {
            art_command::fetch_covers(context, index, &needing, &providers, settings, true)
        }
        Err(error) => {
            context.say(&format!("Covers not looked up: {error}"));
            Ok(Vec::new())
        }
    }
}

fn download_json(waited: &Waited) -> serde_json::Value {
    json!({
        "stalled": waited.stalled.iter().map(fetch_command::stall_row).collect::<Vec<_>>(),
        "stalled_peers": waited
            .stalled_peers
            .iter()
            .map(fetch_command::peer_stall_row)
            .collect::<Vec<_>>(),
        "queued": waited.queued.iter().map(fetch_command::enqueue_row).collect::<Vec<_>>(),
        "collected": waited.collected.iter().map(fetch_command::collect_row).collect::<Vec<_>>(),
        "rounds": waited.rounds,
        "unfinished": waited.unfinished,
        "slskd_lost": waited.slskd_lost,
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
        "artwork": organised
            .covers
            .iter()
            .map(|result| art_command::artwork_row(result, true))
            .collect::<Vec<_>>(),
    })
}

fn report_download(context: &AppContext, waited: &Waited) {
    fetch_command::report_stalled(context, &waited.stalled, &waited.stalled_peers);
    fetch_command::report_queued(context, &waited.queued);
    fetch_command::report_collected(context, &waited.collected);
    context.say(&format!(
        "Queued {} track(s); recorded {} finished transfer(s); cancelled {} stalled one(s)",
        fetch_command::queued_tracks(&waited.queued),
        waited.collected.len(),
        waited.stalled.len()
    ));
    if let Some(error) = &waited.slskd_lost {
        context.say(&format!(
            "slskd stopped answering ({error}); filing what arrived. {} still on their way \
             are collected by the next sync once slskd is back.",
            waited.unfinished
        ));
    } else if waited.unfinished > 0 {
        context.say(&format!(
            "Stopped waiting with {} still on their way; the next sync or \
             stk fetch download --collect records them.",
            waited.unfinished
        ));
    }
    if waited.others > 0 {
        context.say(&format!(
            "{} other download(s) in progress; stk fetch download --collect files them.",
            waited.others
        ));
    }
}
