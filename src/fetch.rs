//! Driving a batch: search for what is wanted, queue what was approved, then
//! tag and verify what arrived. Nothing downloads until a candidate is approved.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use std::path::{Path, PathBuf};

use crate::audio::verify::verify_file;
use crate::config::{Settings, UploaderSettings};
use crate::library::changes::ChangeLog;
use crate::library::index::LibraryIndex;
use crate::library::scan::{LOSSLESS_EXTENSIONS, LOSSY_VERIFIED_EXTENSIONS};
use crate::models::{ScoredCandidate, TrackStatus, Verdict, VerifyResult};
use crate::retag::{self, Retagged};
use crate::review::store::{ReviewStore, ReviewTrack};
use crate::soulseek::base::{SoulseekClient, Transfer, TransferState};
use crate::soulseek::ranking;
use crate::text::normalize::{normalize, sanitize_folder_name, UNKNOWN_ARTIST};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    pub track_id: i64,
    pub query: String,
    pub found: usize,
    pub error: String,
    /// The peer the batch pass moved to the front for this track.
    pub preferred_uploader: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueOutcome {
    pub track_id: i64,
    pub query: String,
    pub destination: String,
    pub queued: bool,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollectOutcome {
    pub track_id: i64,
    pub query: String,
    pub state: TransferState,
    pub local_path: Option<PathBuf>,
    pub verdict: Option<VerifyResult>,
    /// Absent under the `keep` policy, or when the file was never found.
    pub tags: Option<Retagged>,
    pub error: String,
    pub retried_with_rank: Option<i64>,
}

pub fn search_pending(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    limit: Option<usize>,
) -> crate::errors::Result<Vec<SearchOutcome>> {
    let pending = store.tracks(&[TrackStatus::Pending], limit)?;

    // Each search waits out the slskd timeout, so they run concurrently; the
    // store is written afterwards, single-threaded.
    let searched: Vec<crate::errors::Result<Vec<crate::models::Candidate>>> =
        search_concurrently(&pending, client, settings);

    // Deeper than the review cap, so the batch pass can see a peer ranked
    // fourth on every track.
    let pool = settings
        .uploader
        .pool_per_track
        .max(settings.review.candidates_per_track);
    let mut ranked: Vec<Vec<ScoredCandidate>> = Vec::with_capacity(pending.len());
    let mut errors: Vec<String> = Vec::with_capacity(pending.len());
    for (entry, found) in pending.iter().zip(searched) {
        match found {
            Err(error) => {
                ranked.push(Vec::new());
                errors.push(error.to_string());
            }
            Ok(candidates) => {
                ranked.push(ranking::rank(
                    &entry.track,
                    &candidates,
                    &settings.ranking,
                    Some(pool),
                ));
                errors.push(String::new());
            }
        }
    }

    let preferred = settings
        .uploader
        .prefer_one
        .then(|| ranking::prefer_one_uploader(&mut ranked, &settings.uploader))
        .flatten();

    let mut outcomes = Vec::with_capacity(pending.len());
    for ((entry, mut scored), error) in pending.into_iter().zip(ranked).zip(errors) {
        if !error.is_empty() {
            // A failed request says nothing about the track: search it again next run.
            store.mark(entry.id, TrackStatus::Pending, &error)?;
            outcomes.push(SearchOutcome {
                track_id: entry.id,
                query: entry.track.query(),
                found: 0,
                error,
                preferred_uploader: String::new(),
            });
            continue;
        }

        scored.truncate(settings.review.candidates_per_track);
        let taken = preferred
            .as_ref()
            .filter(|preferred| {
                scored
                    .first()
                    .is_some_and(|first| first.candidate.username == preferred.username)
            })
            .map_or_else(String::new, |preferred| preferred.username.clone());

        store.set_candidates(entry.id, &scored)?;
        outcomes.push(SearchOutcome {
            track_id: entry.id,
            query: entry.track.query(),
            found: scored.len(),
            error: String::new(),
            preferred_uploader: taken,
        });
    }
    Ok(outcomes)
}

fn search_concurrently(
    pending: &[crate::review::store::ReviewTrack],
    client: &dyn SoulseekClient,
    settings: &Settings,
) -> Vec<crate::errors::Result<Vec<crate::models::Candidate>>> {
    let search = |entry: &crate::review::store::ReviewTrack| {
        let query = normalize(&format!("{} {}", entry.track.artist, entry.track.title));
        client.search(
            &query,
            settings.slskd.search_timeout_seconds,
            settings.slskd.search_response_limit,
        )
    };

    let workers = settings.slskd.search_concurrency.max(1);
    if workers == 1 || pending.len() < 2 {
        return pending.iter().map(search).collect();
    }

    // Its own pool: these threads wait on sockets, so core count is irrelevant.
    let mut results: Vec<_> = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_or_else(
            |_| pending.iter().map(search).collect(),
            |pool| pool.install(|| pending.par_iter().map(search).collect()),
        );
    // The Soulseek server silently drops searches that arrive in a burst, so
    // one that found nothing is asked again on its own.
    for (entry, result) in pending.iter().zip(results.iter_mut()) {
        if matches!(result, Ok(found) if found.is_empty()) {
            *result = search(entry);
        }
    }
    results
}

#[derive(Debug, Clone, PartialEq)]
pub struct StallOutcome {
    pub track_id: i64,
    pub query: String,
    pub waited_seconds: f64,
    pub retried_with_rank: Option<i64>,
    pub error: String,
    pub peer_group: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerStall {
    pub username: String,
    pub moved_to: Option<String>,
    pub moved: usize,
    pub individually: usize,
    pub exhausted: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stalled {
    pub tracks: Vec<StallOutcome>,
    pub peers: Vec<PeerStall>,
}

/// Give up on transfers that have sat in the queue too long, and try the next
/// candidate instead.
///
/// The stalled transfer is cancelled, or a peer waking up later would deliver
/// a second copy.
///
/// When a preferred peer stalls on more than one track, or slskd reports it
/// offline, its whole group is cancelled and moved together rather than
/// scattered one track per peer. A peer stalling on one file is not down.
pub fn retry_stalled(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    now: f64,
) -> crate::errors::Result<Stalled> {
    let stalled = store.stalled(now, settings.slskd.stall_after_seconds)?;
    if stalled.is_empty() {
        return Ok(Stalled::default());
    }

    let transfers = client.transfers()?;
    let by_path: HashMap<&str, &Transfer> = transfers
        .iter()
        .map(|transfer| (transfer.remote_path.as_str(), transfer))
        .collect();

    let mut outcome = Stalled::default();
    let mut moved: HashSet<i64> = HashSet::new();
    let mut peers: Vec<&str> = Vec::new();
    for peer in stalled.iter().filter_map(preferred_peer) {
        if !peers.contains(&peer) {
            peers.push(peer);
        }
    }
    if !peers.is_empty() {
        let stalled_ids: HashSet<i64> = stalled.iter().map(|entry| entry.id).collect();
        let queued = store.tracks(&[TrackStatus::Queued], None)?;
        for peer in peers {
            let group: Vec<&ReviewTrack> = queued
                .iter()
                .filter(|entry| preferred_peer(entry) == Some(peer))
                .filter(|entry| {
                    entry
                        .approved()
                        .and_then(|approved| by_path.get(approved.candidate.remote_path.as_str()))
                        .is_none_or(|transfer| transfer.state == TransferState::Queued)
                })
                .collect();
            let stalled_in_group = group
                .iter()
                .filter(|entry| stalled_ids.contains(&entry.id))
                .count();
            if stalled_in_group < 2 && !reported_offline(&transfers, peer) {
                continue;
            }
            moved.extend(group.iter().map(|entry| entry.id));
            let group = Group {
                username: peer,
                tracks: &group,
            };
            outcome.peers.push(move_group(
                store,
                client,
                settings,
                &by_path,
                &group,
                now,
                &mut outcome.tracks,
            )?);
        }
    }

    for entry in stalled.iter().filter(|entry| !moved.contains(&entry.id)) {
        let Some(approved) = entry.approved() else {
            continue;
        };
        let error = cancel(client, &by_path, approved);
        let retried = retry_next(store, entry.id)?;
        outcome
            .tracks
            .push(stall_outcome(entry, now, retried, error, String::new()));
    }
    Ok(outcome)
}

fn preferred_peer(entry: &ReviewTrack) -> Option<&str> {
    entry
        .approved()
        .filter(|approved| {
            approved
                .flags
                .iter()
                .any(|flag| flag == ranking::PREFERRED_FLAG)
        })
        .map(|approved| approved.candidate.username.as_str())
}

/// slskd reports an offline peer only in a transfer's exception text.
fn reported_offline(transfers: &[Transfer], username: &str) -> bool {
    transfers.iter().any(|transfer| {
        transfer.username == username && transfer.error.to_lowercase().contains("offline")
    })
}

struct Group<'a> {
    username: &'a str,
    tracks: &'a [&'a ReviewTrack],
}

/// Any peer covering two of the group qualifies; `minimum_coverage` is sized for a batch.
fn move_group(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    by_path: &HashMap<&str, &Transfer>,
    group: &Group<'_>,
    now: f64,
    outcomes: &mut Vec<StallOutcome>,
) -> crate::errors::Result<PeerStall> {
    let mut errors = Vec::with_capacity(group.tracks.len());
    let mut lists = Vec::with_capacity(group.tracks.len());
    for entry in group.tracks {
        errors.push(
            entry
                .approved()
                .map_or_else(String::new, |approved| cancel(client, by_path, approved)),
        );
        lists.push(store.untried(entry.id)?);
    }

    let regroup = UploaderSettings {
        minimum_coverage: 2,
        ..settings.uploader
    };
    let moved_to = ranking::prefer_one_uploader(&mut lists, &regroup).map(|peer| peer.username);

    let mut peer = PeerStall {
        username: group.username.to_owned(),
        moved_to: moved_to.clone(),
        moved: 0,
        individually: 0,
        exhausted: 0,
    };
    for ((entry, list), error) in group.tracks.iter().zip(lists).zip(errors) {
        let taken = moved_to.as_deref().is_some_and(|to| {
            list.first().is_some_and(|first| {
                first.candidate.username == to
                    && first
                        .flags
                        .iter()
                        .any(|flag| flag == ranking::PREFERRED_FLAG)
            })
        });
        store.reorder_untried(entry.id, &list)?;
        let retried = retry_next(store, entry.id)?;
        match retried {
            None => peer.exhausted += 1,
            Some(_) if taken => peer.moved += 1,
            Some(_) => peer.individually += 1,
        }
        outcomes.push(stall_outcome(
            entry,
            now,
            retried,
            error,
            group.username.to_owned(),
        ));
    }
    Ok(peer)
}

fn cancel(
    client: &dyn SoulseekClient,
    by_path: &HashMap<&str, &Transfer>,
    approved: &ScoredCandidate,
) -> String {
    by_path
        .get(approved.candidate.remote_path.as_str())
        .filter(|transfer| !transfer.state.finished())
        .and_then(|transfer| client.cancel(&transfer.username, &transfer.id).err())
        .map_or_else(String::new, |reason| reason.to_string())
}

fn retry_next(store: &mut ReviewStore, track_id: i64) -> crate::errors::Result<Option<i64>> {
    let retried = store.next_candidate(track_id)?;
    if retried.is_none() {
        store.mark(
            track_id,
            TrackStatus::Failed,
            "stalled, and no candidates left",
        )?;
    }
    Ok(retried)
}

fn stall_outcome(
    entry: &ReviewTrack,
    now: f64,
    retried_with_rank: Option<i64>,
    error: String,
    peer_group: String,
) -> StallOutcome {
    StallOutcome {
        track_id: entry.id,
        query: entry.track.query(),
        waited_seconds: entry.queued_at.map_or(0.0, |queued| now - queued),
        retried_with_rank,
        error,
        peer_group,
    }
}

pub fn enqueue_approved(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    limit: Option<usize>,
    now: f64,
) -> crate::errors::Result<Vec<EnqueueOutcome>> {
    let approved = store.tracks(&[TrackStatus::Approved], limit)?;
    enqueue(store, client, approved, now)
}

fn enqueue(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    approved: Vec<ReviewTrack>,
    now: f64,
) -> crate::errors::Result<Vec<EnqueueOutcome>> {
    let mut outcomes = Vec::new();

    for entry in approved {
        let Some(approved) = entry.approved().cloned() else {
            store.mark(
                entry.id,
                TrackStatus::Failed,
                "approved candidate is missing",
            )?;
            outcomes.push(EnqueueOutcome {
                track_id: entry.id,
                query: entry.track.query(),
                destination: String::new(),
                queued: false,
                error: "no candidate".to_owned(),
            });
            continue;
        };

        let destination = sanitize_folder_name(&entry.track.artist, UNKNOWN_ARTIST);
        if let Err(error) = client.enqueue(
            &approved.candidate.username,
            std::slice::from_ref(&approved.candidate),
            &destination,
        ) {
            store.mark(entry.id, TrackStatus::Failed, &error.to_string())?;
            outcomes.push(EnqueueOutcome {
                track_id: entry.id,
                query: entry.track.query(),
                destination,
                queued: false,
                error: error.to_string(),
            });
            continue;
        }

        store.mark_queued(
            entry.id,
            now,
            &format!("from {}", approved.candidate.username),
        )?;
        outcomes.push(EnqueueOutcome {
            track_id: entry.id,
            query: entry.track.query(),
            destination,
            queued: true,
            error: String::new(),
        });
    }
    Ok(outcomes)
}

/// A failure promotes the next untried candidate; an arrival is tagged, then verified.
pub fn collect(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    index: &mut LibraryIndex,
    roots: &[PathBuf],
    now: f64,
    verify: bool,
) -> crate::errors::Result<Vec<CollectOutcome>> {
    let finished: HashMap<String, Transfer> = client
        .transfers()?
        .into_iter()
        .filter(|transfer| transfer.state.finished())
        .map(|transfer| (transfer.remote_path.clone(), transfer))
        .collect();
    if finished.is_empty() {
        return Ok(Vec::new());
    }

    let queued = store.tracks(&[TrackStatus::Queued], None)?;
    let log = ChangeLog::open(index, "fetch download", now)?;
    let mut outcomes = Vec::new();

    for entry in queued {
        let Some(approved) = entry.approved() else {
            continue;
        };
        let Some(transfer) = finished.get(&approved.candidate.remote_path) else {
            continue;
        };

        if transfer.state != TransferState::Completed {
            let detail = if transfer.error.is_empty() {
                transfer.state.to_string()
            } else {
                transfer.error.clone()
            };
            let retried = store.next_candidate(entry.id)?;
            if retried.is_none() {
                store.mark(entry.id, TrackStatus::Failed, &detail)?;
            }
            outcomes.push(CollectOutcome {
                track_id: entry.id,
                query: entry.track.query(),
                state: transfer.state,
                local_path: None,
                verdict: None,
                tags: None,
                error: transfer.error.clone(),
                retried_with_rank: retried,
            });
            continue;
        }

        let local_path = downloaded_path(settings, &entry.track.artist, transfer);
        // slskd calls a transfer done before moving the file out of its
        // incomplete folder, a copy across Docker mounts; tagging a half-copied
        // file corrupts it. It is collected once all of it has arrived.
        let partial = local_path
            .as_deref()
            .and_then(|path| std::fs::metadata(path).ok())
            .is_some_and(|meta| meta.len() < transfer.size);
        if partial {
            continue;
        }
        let tags = match local_path.as_deref().filter(|path| path.is_file()) {
            Some(path) => {
                retag::apply(path, settings.fetch.tags, &entry.track, &log, index, roots)?
            }
            None => None,
        };
        let verdict = if verify {
            verify_downloaded(local_path.as_deref(), settings)
        } else {
            None
        };
        store.mark(
            entry.id,
            TrackStatus::Downloaded,
            &describe(verdict.as_ref()),
        )?;
        outcomes.push(CollectOutcome {
            track_id: entry.id,
            query: entry.track.query(),
            state: transfer.state,
            local_path,
            verdict,
            tags,
            error: String::new(),
            retried_with_rank: None,
        });
    }
    Ok(outcomes)
}

/// Time as the download wait sees it, so tests don't sleep.
pub trait Clock {
    fn now(&self) -> f64;
    fn sleep(&mut self, seconds: f64);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64())
    }

    fn sleep(&mut self, seconds: f64) {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patience {
    pub timeout_seconds: f64,
    pub poll_seconds: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Waited {
    pub stalled: Vec<StallOutcome>,
    pub stalled_peers: Vec<PeerStall>,
    pub queued: Vec<EnqueueOutcome>,
    pub collected: Vec<CollectOutcome>,
    pub rounds: usize,
    /// Nonzero only on a timeout.
    pub unfinished: usize,
}

impl Waited {
    #[must_use]
    pub fn went_wrong(&self) -> bool {
        downloads_went_wrong(&self.queued, &self.collected)
    }
}

/// Whether a download run failed: a track that couldn't be queued, one that
/// failed with nothing left to try, or a download `verify` calls fake or
/// upsampled, or can't read to the end. Retried failures and timeouts don't count.
#[must_use]
pub fn downloads_went_wrong(queued: &[EnqueueOutcome], collected: &[CollectOutcome]) -> bool {
    queued.iter().any(|outcome| !outcome.queued)
        || collected.iter().any(|outcome| {
            (!outcome.error.is_empty() && outcome.retried_with_rank.is_none())
                || outcome.verdict.as_ref().is_some_and(|verdict| {
                    matches!(
                        verdict.verdict,
                        Verdict::Fake | Verdict::Upsampled | Verdict::Error
                    )
                })
        })
}

/// On timeout slskd keeps the transfers, and the next collect records them.
#[allow(clippy::too_many_arguments)]
pub fn wait_for_downloads(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    index: &mut LibraryIndex,
    roots: &[PathBuf],
    limit: Option<usize>,
    patience: Patience,
    clock: &mut dyn Clock,
    progress: &mut dyn FnMut(&Waited),
) -> crate::errors::Result<Waited> {
    let start = clock.now();
    let mut waited = Waited::default();

    let mut ours: HashSet<i64> = store
        .tracks(&[TrackStatus::Queued], None)?
        .iter()
        .map(|entry| entry.id)
        .collect();
    let sent = enqueue_approved(store, client, limit, start)?;
    ours.extend(
        sent.iter()
            .filter(|outcome| outcome.queued)
            .map(|outcome| outcome.track_id),
    );
    waited.queued = sent;
    if ours.is_empty() {
        return Ok(waited);
    }

    loop {
        let now = clock.now();
        // Collect first, so a transfer that just finished isn't taken as stalled.
        waited
            .collected
            .extend(collect(store, client, settings, index, roots, now, true)?);
        let stalled = retry_stalled(store, client, settings, now)?;
        waited.stalled.extend(stalled.tracks);
        waited.stalled_peers.extend(stalled.peers);
        let promoted: Vec<ReviewTrack> = store
            .tracks(&[TrackStatus::Approved], None)?
            .into_iter()
            .filter(|entry| ours.contains(&entry.id))
            .collect();
        waited.queued.extend(enqueue(store, client, promoted, now)?);
        waited.rounds += 1;

        waited.unfinished = store
            .tracks(&[TrackStatus::Queued, TrackStatus::Approved], None)?
            .iter()
            .filter(|entry| ours.contains(&entry.id))
            .count();
        progress(&waited);
        if waited.unfinished == 0 || clock.now() - start >= patience.timeout_seconds {
            return Ok(waited);
        }
        clock.sleep(patience.poll_seconds);
    }
}

/// Rebuilt from download dir, artist folder and filename; must match `enqueue_approved`.
#[must_use]
pub fn downloaded_path(settings: &Settings, artist: &str, transfer: &Transfer) -> Option<PathBuf> {
    let root = settings.slskd.download_directory.as_ref()?;
    let filename = transfer
        .remote_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&transfer.remote_path);
    Some(
        root.join(sanitize_folder_name(artist, UNKNOWN_ARTIST))
            .join(filename),
    )
}

fn verify_downloaded(local_path: Option<&Path>, settings: &Settings) -> Option<VerifyResult> {
    let path = local_path?;
    if !path.is_file() {
        return None;
    }
    let suffix = suffix(path)?;
    if !LOSSLESS_EXTENSIONS.contains(&suffix.as_str())
        && !LOSSY_VERIFIED_EXTENSIONS.contains(&suffix.as_str())
    {
        return None;
    }
    Some(verify_file(path, &settings.verify))
}

fn suffix(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_lowercase()))
}

fn describe(verdict: Option<&VerifyResult>) -> String {
    match verdict {
        None => "downloaded".to_owned(),
        Some(result) if result.verdict == Verdict::Ok => {
            let lossless = suffix(&result.path)
                .is_some_and(|suffix| LOSSLESS_EXTENSIONS.contains(&suffix.as_str()));
            if lossless {
                "downloaded, verified lossless".to_owned()
            } else {
                "downloaded, verified".to_owned()
            }
        }
        Some(result) => format!("downloaded, {}: {}", result.verdict, result.detail),
    }
}
