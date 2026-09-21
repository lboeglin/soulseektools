//! Driving a batch: search for what is wanted, queue what was approved, check
//! what arrived.
//!
//! Nothing downloads without a human choosing a candidate. That was true of the
//! plugin this replaces and stays true here; the difference is that the choice
//! is recorded in the review queue rather than typed into a spreadsheet column.
//!
//! Completed downloads are run through the spectral check in-process, rather
//! than by shelling out to a script at a hardcoded path as the plugin did, and
//! tagged from the review row they were fetched for (#11).

use std::collections::HashMap;

use rayon::prelude::*;
use std::path::{Path, PathBuf};

use crate::audio::verify::verify_file;
use crate::config::Settings;
use crate::library::changes::ChangeLog;
use crate::library::index::LibraryIndex;
use crate::library::scan::LOSSLESS_EXTENSIONS;
use crate::models::{ScoredCandidate, TrackStatus, Verdict, VerifyResult};
use crate::retag::{self, Retagged};
use crate::review::store::ReviewStore;
use crate::soulseek::base::{SoulseekClient, Transfer, TransferState};
use crate::soulseek::ranking;
use crate::text::normalize::{normalize, sanitize_folder_name, UNKNOWN_ARTIST};

/// What one track's search produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    pub track_id: i64,
    pub query: String,
    pub found: usize,
    pub error: String,
    /// The peer the batch pass moved to the front for this track (#3).
    pub preferred_uploader: String,
}

/// What happened when an approved candidate was queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueOutcome {
    pub track_id: i64,
    pub query: String,
    pub destination: String,
    pub queued: bool,
    pub error: String,
}

/// A finished transfer, and what the file turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectOutcome {
    pub track_id: i64,
    pub query: String,
    pub state: TransferState,
    pub local_path: Option<PathBuf>,
    pub verdict: Option<VerifyResult>,
    /// What the file was tagged with from its review row; absent under the
    /// `keep` policy, or when the file was never found (#11).
    pub tags: Option<Retagged>,
    pub error: String,
    /// Set when a failure promoted the next candidate rather than giving up (#2).
    pub retried_with_rank: Option<i64>,
}

/// Search Soulseek for every track still waiting, and record the candidates.
pub fn search_pending(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    limit: Option<usize>,
) -> crate::errors::Result<Vec<SearchOutcome>> {
    let pending = store.tracks(&[TrackStatus::Pending], limit)?;

    // The searches run concurrently and the results are written afterwards.
    //
    // Every search is a socket wait -- slskd polls the Soulseek network for the
    // configured timeout, one whole second at a time -- so a serial run costs
    // roughly `timeout x tracks`, which for a full wantlist is the difference
    // between minutes and most of an hour. The store stays single-threaded:
    // SQLite is not the slow part and does not need to be shared.
    let searched: Vec<crate::errors::Result<Vec<crate::models::Candidate>>> =
        search_concurrently(&pending, client, settings);

    // Scored first, and deeper than the review cap: truncating per track before
    // anything looked across the batch is what made a peer holding most of it
    // invisible on every track where they placed fourth (#3).
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
            store.mark(entry.id, TrackStatus::Failed, &error)?;
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

/// Run every pending track's search at once, bounded by `search_concurrency`.
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

    // Its own pool rather than the global one: these threads are parked on a
    // socket, not computing, so the right number has nothing to do with the
    // number of cores rayon would otherwise pick.
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_or_else(
            |_| pending.iter().map(search).collect(),
            |pool| pool.install(|| pending.par_iter().map(search).collect()),
        )
}

/// A transfer that sat too long, and what was done about it.
#[derive(Debug, Clone, PartialEq)]
pub struct StallOutcome {
    pub track_id: i64,
    pub query: String,
    pub waited_seconds: f64,
    /// The rank promoted in its place, absent when the candidates ran out.
    pub retried_with_rank: Option<i64>,
    pub error: String,
}

/// Give up on transfers that have sat in the queue too long, and try the next
/// candidate instead.
///
/// `collect` only ever sees transfers slskd already considers finished, so a
/// download sitting behind a peer that never uploads is invisible to it: this
/// is what makes `queued_at` worth recording (#2). Soulseek queues stall
/// constantly, and the candidate that would have worked was in the database all
/// along.
///
/// The stalled transfer is cancelled rather than left in place. A peer that
/// wakes up hours later would otherwise deliver the file a second time, beside
/// the copy the retry already fetched.
pub fn retry_stalled(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    now: f64,
) -> crate::errors::Result<Vec<StallOutcome>> {
    let stalled = store.stalled(now, settings.slskd.stall_after_seconds)?;
    if stalled.is_empty() {
        return Ok(Vec::new());
    }

    let unfinished: HashMap<String, Transfer> = client
        .transfers()?
        .into_iter()
        .filter(|transfer| !transfer.state.finished())
        .map(|transfer| (transfer.remote_path.clone(), transfer))
        .collect();

    let mut outcomes = Vec::new();
    for entry in stalled {
        let Some(approved) = entry.approved() else {
            continue;
        };

        let mut error = String::new();
        if let Some(transfer) = unfinished.get(&approved.candidate.remote_path) {
            if let Err(reason) = client.cancel(&transfer.username, &transfer.id) {
                error = reason.to_string();
            }
        }

        let retried = store.next_candidate(entry.id)?;
        if retried.is_none() {
            store.mark(
                entry.id,
                TrackStatus::Failed,
                "stalled, and no candidates left",
            )?;
        }
        outcomes.push(StallOutcome {
            track_id: entry.id,
            query: entry.track.query(),
            waited_seconds: entry.queued_at.map_or(0.0, |queued| now - queued),
            retried_with_rank: retried,
            error,
        });
    }
    Ok(outcomes)
}

/// Queue every approved pick, sorting each into a folder named for its artist.
pub fn enqueue_approved(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    limit: Option<usize>,
    now: f64,
) -> crate::errors::Result<Vec<EnqueueOutcome>> {
    let approved_tracks = store.tracks(&[TrackStatus::Approved], limit)?;
    let mut outcomes = Vec::new();

    for entry in approved_tracks {
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

        // `queued_at` is what makes a stall visible later (#2).
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

/// Update queued tracks from the backend's transfer list, tagging and
/// verifying what arrived.
///
/// A failure promotes the next unused candidate rather than ending the track,
/// which is the retry path #2 asked for: rank 2 was always sitting in the
/// database, and nothing ever reached for it.
///
/// A completed file is tagged from its review row under `fetch.tags` before it
/// is verified, and every tag written is logged against `index` under one
/// `fetch download` run, so the whole collect can be undone at once (#11).
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

/// Where the finished file should be on disk.
///
/// slskd reports the remote path, not the local one, so the destination is
/// rebuilt from the configured download directory, the artist folder the batch
/// was queued into, and the file's own name. `sanitize_folder_name` has to match
/// what `enqueue_approved` used or the file stops being found.
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
    let suffix = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_lowercase()))?;
    if !LOSSLESS_EXTENSIONS.contains(&suffix.as_str()) {
        return None;
    }
    Some(verify_file(path, &settings.verify))
}

fn describe(verdict: Option<&VerifyResult>) -> String {
    match verdict {
        None => "downloaded".to_owned(),
        Some(result) if result.verdict == Verdict::Ok => "downloaded, verified lossless".to_owned(),
        Some(result) => format!("downloaded, {}: {}", result.verdict, result.detail),
    }
}
