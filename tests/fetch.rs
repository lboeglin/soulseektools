//! Search, enqueue, collect, stall retry and the download wait, against a fake client.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use stk::config::Settings;
use stk::errors::{Error, Result};
use stk::fetch::{self, downloaded_path};
use stk::library::changes::{apply_undo, plan_undo, ChangeFilter, ChangeLog};
use stk::library::index::LibraryIndex;
use stk::library::tags::{self, TagField};
use stk::models::{Candidate, ScoredCandidate, Track, TrackStatus, Verdict};
use stk::retag::TagPolicy;
use stk::review::albums;
use stk::review::store::ReviewStore;
use stk::soulseek::base::{SoulseekClient, Transfer, TransferState};
use support::{committed, encode, transcode_to_flac, Encoded, Scratch};

#[derive(Default)]
struct FakeClient {
    candidates: Vec<Candidate>,
    transfers: Vec<Transfer>,
    search_error: Option<String>,
    enqueue_error: Option<String>,
    /// A peer that refuses every enqueue, as slskd does for one gone offline.
    offline: Option<String>,
    /// This many searches find nothing first, as dropped ones do.
    dropped_searches: usize,
    searched: Mutex<Vec<String>>,
    enqueued: Mutex<Vec<(String, String)>>,
    cancelled: Mutex<Vec<(String, String)>>,
}

impl FakeClient {
    fn with_candidates(candidates: Vec<Candidate>) -> Self {
        Self {
            candidates,
            ..Self::default()
        }
    }

    fn with_transfers(transfers: Vec<Transfer>) -> Self {
        Self {
            transfers,
            ..Self::default()
        }
    }
}

impl SoulseekClient for FakeClient {
    fn is_connected(&self) -> bool {
        true
    }

    fn search(&self, text: &str, _timeout: u32, _limit: u32) -> Result<Vec<Candidate>> {
        self.searched
            .lock()
            .expect("the fake is not poisoned")
            .push(text.to_owned());
        let asked = self
            .searched
            .lock()
            .expect("the fake is not poisoned")
            .len();
        match &self.search_error {
            Some(error) => Err(Error::SoulseekClient(error.clone())),
            None if asked <= self.dropped_searches => Ok(Vec::new()),
            None => Ok(self.candidates.clone()),
        }
    }

    fn enqueue(
        &self,
        username: &str,
        _files: &[Candidate],
        destination: &str,
    ) -> Result<Vec<Transfer>> {
        self.enqueued
            .lock()
            .expect("the fake is not poisoned")
            .push((username.to_owned(), destination.to_owned()));
        if self.offline.as_deref() == Some(username) {
            return Err(Error::SoulseekClient(format!(
                "User {username} appears to be offline"
            )));
        }
        match &self.enqueue_error {
            Some(error) => Err(Error::SoulseekClient(error.clone())),
            None => Ok(Vec::new()),
        }
    }

    fn transfers(&self) -> Result<Vec<Transfer>> {
        Ok(self.transfers.clone())
    }

    fn cancel(&self, username: &str, transfer_id: &str) -> Result<()> {
        self.cancelled
            .lock()
            .expect("the fake is not poisoned")
            .push((username.to_owned(), transfer_id.to_owned()));
        Ok(())
    }
}

fn candidate(remote_path: &str) -> Candidate {
    Candidate {
        username: "peerA".to_owned(),
        remote_path: remote_path.to_owned(),
        size: 30_000_000,
        bit_depth: Some(16),
        sample_rate: Some(44100),
        free_upload_slots: true,
        ..Candidate::default()
    }
}

fn transfer(remote_path: &str, state: TransferState, error: &str) -> Transfer {
    Transfer {
        id: "t1".to_owned(),
        username: "peerA".to_owned(),
        remote_path: remote_path.to_owned(),
        state,
        size: 100,
        bytes_transferred: 100,
        error: error.to_owned(),
        ..Transfer::default()
    }
}

fn settings(scratch: &Scratch) -> Settings {
    Settings {
        slskd: stk::config::SlskdSettings {
            download_directory: Some(scratch.join("downloads")),
            search_interval_seconds: 0.0,
            ..stk::config::SlskdSettings::default()
        },
        index: stk::config::IndexSettings {
            path: scratch.join("library.db"),
        },
        library: stk::config::LibrarySettings {
            root: scratch.join("Music"),
            ..stk::config::LibrarySettings::default()
        },
        ..Settings::default()
    }
}

fn store(scratch: &Scratch) -> ReviewStore {
    ReviewStore::open(&scratch.join("review.db")).expect("a store")
}

fn index(settings: &Settings) -> LibraryIndex {
    LibraryIndex::open(&settings.index.path).expect("an index")
}

fn collect(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    verify: bool,
) -> Result<Vec<fetch::CollectOutcome>> {
    let mut index = index(settings);
    fetch::collect(
        store,
        client,
        settings,
        &mut index,
        &settings.library.all_roots(),
        2000.0,
        verify,
    )
}

fn roki() -> Track {
    Track::new("DECO*27", "Roki")
}

fn queued(store: &mut ReviewStore, settings: &Settings) -> i64 {
    store.add(&[roki()]).expect("an added track");
    let client = FakeClient::with_candidates(vec![candidate("@@peerA\\Music\\Roki.flac")]);
    fetch::search_pending(store, &client, settings, None).expect("a search");

    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");
    fetch::enqueue_approved(store, &FakeClient::default(), None, 1000.0).expect("an enqueue");
    track_id
}

#[test]
fn candidates_are_ranked_and_stored() {
    let scratch = Scratch::new("fetch-rank");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");

    let client = FakeClient::with_candidates(vec![
        candidate("@@peerA\\Music\\Roki.flac"),
        candidate("@@peerB\\Music\\Roki.mp3"),
    ]);
    let outcomes = fetch::search_pending(&mut store, &client, &settings, None).expect("a search");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].found, 2);
    let entry = &store.tracks(&[], None).expect("tracks")[0];
    assert_eq!(entry.status, TrackStatus::AwaitingReview);
    assert_eq!(entry.candidates.len(), 2);
}

#[test]
fn a_failed_search_leaves_the_track_to_search_again() {
    // A failed request says nothing about the track.
    let scratch = Scratch::new("fetch-search-failure");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store
        .add(&[Track::new("A", "One"), Track::new("B", "Two")])
        .expect("added tracks");

    let client = FakeClient {
        search_error: Some("slskd unreachable".to_owned()),
        ..FakeClient::default()
    };
    let outcomes = fetch::search_pending(&mut store, &client, &settings, None).expect("a search");

    assert_eq!(outcomes.len(), 2);
    assert!(store
        .tracks(&[], None)
        .expect("tracks")
        .iter()
        .all(|entry| entry.status == TrackStatus::Pending));
    assert!(
        outcomes[0].error.contains("unreachable"),
        "{:?}",
        outcomes[0]
    );
}

#[test]
fn the_approved_candidate_is_queued() {
    let scratch = Scratch::new("fetch-enqueue");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let client = FakeClient::with_candidates(vec![candidate("@@peerA\\Music\\Roki.flac")]);
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");

    let enqueuer = FakeClient::default();
    let outcomes =
        fetch::enqueue_approved(&mut store, &enqueuer, None, 1000.0).expect("an enqueue");

    assert!(outcomes[0].queued);
    assert_eq!(
        outcomes[0].destination, "DECO_27",
        "a folder name the filesystem accepts"
    );
    assert_eq!(enqueuer.enqueued.lock().expect("the fake")[0].0, "peerA");
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::Queued);
    assert_eq!(entry.queued_at, Some(1000.0));
}

#[test]
fn an_enqueue_failure_marks_the_track() {
    let scratch = Scratch::new("fetch-enqueue-failure");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let client = FakeClient::with_candidates(vec![candidate("@@peerA\\Music\\Roki.flac")]);
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");

    let failing = FakeClient {
        enqueue_error: Some("peer offline".to_owned()),
        ..FakeClient::default()
    };
    let outcomes = fetch::enqueue_approved(&mut store, &failing, None, 1000.0).expect("an enqueue");

    assert!(!outcomes[0].queued);
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::Failed);
    assert!(entry.detail.contains("peer offline"), "{}", entry.detail);
}

#[test]
fn a_completed_transfer_is_recorded() {
    let scratch = Scratch::new("fetch-completed");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcomes = collect(&mut store, &client, &settings, true).expect("a collect");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(
        store
            .track(track_id)
            .expect("a lookup")
            .expect("the track")
            .status,
        TrackStatus::Downloaded
    );
}

#[test]
fn a_failed_transfer_with_only_flagged_candidates_left_waits_for_review() {
    let scratch = Scratch::new("fetch-retry-flagged");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store
        .add(&[Track::new("DECO*27", "Roki (Reloaded)")])
        .expect("an added track");

    let client = FakeClient::with_candidates(vec![
        candidate(r"@@peerA\Roki Reloaded\Roki.flac"),
        candidate(r"@@peerB\Music\Roki.flac"),
    ]);
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");
    fetch::enqueue_approved(&mut store, &FakeClient::default(), None, 1000.0).expect("an enqueue");

    let failing = FakeClient::with_transfers(vec![transfer(
        r"@@peerA\Roki Reloaded\Roki.flac",
        TransferState::Failed,
        "peer gone",
    )]);
    let outcomes = collect(&mut store, &failing, &settings, true).expect("a collect");

    assert_eq!(outcomes[0].retried_with_rank, None);
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::AwaitingReview);
}

#[test]
fn a_failed_transfer_promotes_the_next_candidate_when_there_is_one() {
    let scratch = Scratch::new("fetch-retry");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");

    let client = FakeClient::with_candidates(vec![
        candidate("@@peerA\\Music\\Roki.flac"),
        candidate("@@peerB\\Music\\Roki.flac"),
    ]);
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");
    fetch::enqueue_approved(&mut store, &FakeClient::default(), None, 1000.0).expect("an enqueue");

    let failing = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Failed,
        "peer gone",
    )]);
    let outcomes = collect(&mut store, &failing, &settings, true).expect("a collect");

    assert_eq!(outcomes[0].retried_with_rank, Some(2));
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::Approved);
    assert_eq!(entry.approved_rank, Some(2));
    assert_eq!(entry.attempts, 1);
}

#[test]
fn a_transfer_for_another_track_is_ignored() {
    let scratch = Scratch::new("fetch-other");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Something Else.flac",
        TransferState::Completed,
        "",
    )]);

    assert!(collect(&mut store, &client, &settings, true)
        .expect("a collect")
        .is_empty());
}

#[test]
fn a_downloaded_transcode_is_recorded_against_the_track() {
    let scratch = Scratch::new("fetch-transcode");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = queued(&mut store, &settings);

    let faked = transcode_to_flac(&scratch, "fake", "128k");
    let destination = scratch.join("downloads").join("DECO_27").join("Roki.flac");
    std::fs::create_dir_all(destination.parent().expect("a parent")).expect("a directory");
    std::fs::copy(&faked, &destination).expect("a copied file");

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &collect(&mut store, &client, &settings, true).expect("a collect")[0];

    assert_eq!(
        outcome.verdict.as_ref().expect("a verdict").verdict,
        Verdict::Fake
    );
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert!(entry.detail.contains("FAKE"), "{}", entry.detail);
}

fn lossy_download(
    scratch: &Scratch,
    settings: &Settings,
    store: &mut ReviewStore,
    filename: &str,
    arrived: &Path,
) -> fetch::CollectOutcome {
    let remote = format!("@@peerA\\Music\\{filename}");
    store.add(&[roki()]).expect("an added track");
    let client = FakeClient::with_candidates(vec![candidate(&remote)]);
    fetch::search_pending(store, &client, settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");
    fetch::enqueue_approved(store, &FakeClient::default(), None, 1000.0).expect("an enqueue");

    let destination = scratch.join("downloads").join("DECO_27").join(filename);
    std::fs::create_dir_all(destination.parent().expect("a parent")).expect("a directory");
    std::fs::copy(arrived, &destination).expect("a copied file");

    let finished =
        FakeClient::with_transfers(vec![transfer(&remote, TransferState::Completed, "")]);
    collect(store, &finished, settings, true)
        .expect("a collect")
        .remove(0)
}

#[test]
fn a_downloaded_upsampled_mp3_is_caught_and_counts_as_going_wrong() {
    let scratch = Scratch::new("fetch-mp3-upsampled");
    let settings = settings(&scratch);
    let mut store = store(&scratch);

    let outcome = lossy_download(
        &scratch,
        &settings,
        &mut store,
        "Roki.mp3",
        &committed("noise-128k-as-320k.mp3"),
    );

    assert_eq!(
        outcome.verdict.as_ref().expect("a verdict").verdict,
        Verdict::Upsampled
    );
    let entry = store
        .track(outcome.track_id)
        .expect("a lookup")
        .expect("the track");
    assert!(entry.detail.contains("UPSAMPLED"), "{}", entry.detail);
    let waited = fetch::Waited {
        collected: vec![outcome],
        ..fetch::Waited::default()
    };
    assert!(waited.went_wrong());
}

#[test]
fn downloaded_path_keeps_only_the_filename() {
    let scratch = Scratch::new("fetch-filename");
    let settings = settings(&scratch);
    let transfer = transfer(
        "@@peerA\\Deep\\Nested\\Roki.flac",
        TransferState::Completed,
        "",
    );

    assert_eq!(
        downloaded_path(&settings, "DECO*27", &transfer),
        Some(scratch.join("downloads").join("DECO_27").join("Roki.flac"))
    );
}

/// By rendezvous, not timing: each search blocks until a second one joins it.
#[test]
fn a_search_dropped_in_a_burst_is_asked_again_on_its_own() {
    let scratch = Scratch::new("fetch-dropped-search");
    let mut settings = settings(&scratch);
    settings.slskd.search_concurrency = 4;
    let mut store = store(&scratch);
    store
        .add(&[
            Track::new("DECO*27", "Roki"),
            Track::new("DECO*27", "Ghost Rule"),
        ])
        .expect("added tracks");
    let client = FakeClient {
        dropped_searches: 2,
        ..FakeClient::with_candidates(vec![
            candidate("@@peerA\\DECO*27\\Roki.flac"),
            candidate("@@peerA\\DECO*27\\Ghost Rule.flac"),
        ])
    };

    let outcomes = fetch::search_pending(&mut store, &client, &settings, None).expect("a search");

    assert_eq!(client.searched.lock().expect("the fake").len(), 4);
    assert!(
        outcomes.iter().all(|outcome| outcome.found > 0),
        "{outcomes:?}"
    );
}

#[test]
fn searches_run_concurrently_rather_than_one_after_another() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Condvar;

    /// Long enough for a loaded runner, short enough that serial code fails rather than hangs.
    const RENDEZVOUS: std::time::Duration = std::time::Duration::from_secs(5);

    const WORKERS: usize = 4;

    struct Meeting {
        inside: Mutex<usize>,
        joined: Condvar,
        peak: AtomicUsize,
        settled: AtomicBool,
    }

    impl SoulseekClient for Meeting {
        fn is_connected(&self) -> bool {
            true
        }

        fn search(&self, _text: &str, _timeout: u32, _limit: u32) -> Result<Vec<Candidate>> {
            let mut inside = self.inside.lock().expect("the rendezvous lock");
            *inside += 1;
            self.peak.fetch_max(*inside, Ordering::SeqCst);

            if *inside > 1 {
                self.settled.store(true, Ordering::SeqCst);
                self.joined.notify_all();
            } else if !self.settled.load(Ordering::SeqCst) {
                let (guard, _) = self
                    .joined
                    .wait_timeout_while(inside, RENDEZVOUS, |count| *count < 2)
                    .expect("the rendezvous lock");
                inside = guard;
                self.settled.store(true, Ordering::SeqCst);
            }

            *inside -= 1;
            Ok(Vec::new())
        }

        fn enqueue(&self, _user: &str, _files: &[Candidate], _to: &str) -> Result<Vec<Transfer>> {
            Ok(Vec::new())
        }

        fn transfers(&self) -> Result<Vec<Transfer>> {
            Ok(Vec::new())
        }

        fn cancel(&self, _user: &str, _transfer_id: &str) -> Result<()> {
            Ok(())
        }
    }

    let scratch = Scratch::new("fetch-concurrent");
    let mut settings = settings(&scratch);
    settings.slskd.search_concurrency = WORKERS;
    let mut store = store(&scratch);

    let tracks: Vec<Track> = (0..8)
        .map(|index| Track::new("A", &index.to_string()))
        .collect();
    store.add(&tracks).expect("added tracks");

    let client = Meeting {
        inside: Mutex::new(0),
        joined: Condvar::new(),
        peak: AtomicUsize::new(0),
        settled: AtomicBool::new(false),
    };

    let outcomes = fetch::search_pending(&mut store, &client, &settings, None).expect("a search");
    let peak = client.peak.load(Ordering::SeqCst);

    assert_eq!(outcomes.len(), 8);
    assert!(
        peak > 1,
        "no two searches ever overlapped: the fan-out ran one at a time"
    );
    assert!(
        peak <= WORKERS,
        "{peak} searches ran at once, above the configured {WORKERS}"
    );
}

#[test]
fn concurrent_searches_start_at_least_the_interval_apart() {
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

    let scratch = Scratch::new("fetch-paced");
    let mut settings = settings(&scratch);
    settings.slskd.search_concurrency = 4;
    settings.slskd.search_interval_seconds = INTERVAL.as_secs_f64();
    let mut store = store(&scratch);
    let tracks: Vec<Track> = (0..6)
        .map(|index| Track::new("A", &index.to_string()))
        .collect();
    store.add(&tracks).expect("added tracks");
    let client = FakeClient::with_candidates(vec![candidate(r"@@peerA\A\0.flac")]);

    let began = std::time::Instant::now();
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");
    let took = began.elapsed();

    // Measured around the whole batch: a worker paused between its turn and its
    // request can't make this shorter.
    assert_eq!(client.searched.lock().expect("the fake").len(), 6);
    assert!(took >= INTERVAL * 5, "six searches started within {took:?}");
}

#[test]
fn concurrent_results_stay_matched_to_their_tracks() {
    let scratch = Scratch::new("fetch-concurrent-order");
    let mut settings = settings(&scratch);
    settings.slskd.search_concurrency = 4;
    let mut store = store(&scratch);

    store
        .add(&[
            Track::new("DECO*27", "Roki"),
            Track::new("ZUN", "Heian Alien"),
        ])
        .expect("added tracks");

    let client = FakeClient::with_candidates(vec![candidate(r"@@peerA\\Music\\Roki.flac")]);
    let outcomes = fetch::search_pending(&mut store, &client, &settings, None).expect("a search");

    let by_query: std::collections::HashMap<&str, usize> = outcomes
        .iter()
        .map(|outcome| (outcome.query.as_str(), outcome.found))
        .collect();
    assert_eq!(by_query["DECO*27 - Roki"], 1);
    assert_eq!(by_query["ZUN - Heian Alien"], 0);
}

/// slskd never reports a transfer nobody uploads as finished, so `collect` never sees it.
#[test]
fn a_stalled_transfer_is_cancelled_and_the_next_candidate_promoted() {
    let scratch = Scratch::new("fetch-stalled");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let searcher = FakeClient::with_candidates(vec![
        candidate("@@peerA\\Music\\Roki.flac"),
        candidate("@@peerB\\Music\\Roki.flac"),
    ]);
    fetch::search_pending(&mut store, &searcher, &settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");
    fetch::enqueue_approved(&mut store, &FakeClient::default(), None, 1000.0).expect("an enqueue");

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Queued,
        "",
    )]);
    let now = 1000.0 + settings.slskd.stall_after_seconds + 1.0;
    let outcomes = fetch::retry_stalled(&mut store, &client, &settings, now)
        .expect("a retry")
        .tracks;

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].retried_with_rank, Some(2));
    assert_eq!(
        *client.cancelled.lock().expect("the fake is not poisoned"),
        vec![("peerA".to_owned(), "t1".to_owned())]
    );

    let entry = &store.tracks(&[], None).expect("tracks")[0];
    assert_eq!(entry.status, TrackStatus::Approved);
    assert_eq!(entry.approved_rank, Some(2));
    assert_eq!(entry.attempts, 1);
}

#[test]
fn a_transfer_still_inside_its_grace_period_is_left_alone() {
    let scratch = Scratch::new("fetch-not-stalled");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Queued,
        "",
    )]);
    let now = 1000.0 + settings.slskd.stall_after_seconds - 1.0;
    let outcomes = fetch::retry_stalled(&mut store, &client, &settings, now)
        .expect("a retry")
        .tracks;

    assert!(outcomes.is_empty());
    assert!(client
        .cancelled
        .lock()
        .expect("the fake is not poisoned")
        .is_empty());
    assert_eq!(
        store.tracks(&[], None).expect("tracks")[0].status,
        TrackStatus::Queued
    );
}

/// Each track needs a different winning peer, which one shared shortlist can't express.
struct PerTrackClient {
    lists: Vec<(String, Vec<Candidate>)>,
}

impl SoulseekClient for PerTrackClient {
    fn is_connected(&self) -> bool {
        true
    }

    fn search(&self, text: &str, _timeout: u32, _limit: u32) -> Result<Vec<Candidate>> {
        Ok(self
            .lists
            .iter()
            .find(|(needle, _)| text.contains(needle.as_str()))
            .map(|(_, list)| list.clone())
            .unwrap_or_default())
    }

    fn enqueue(
        &self,
        _username: &str,
        _files: &[Candidate],
        _destination: &str,
    ) -> Result<Vec<Transfer>> {
        Ok(Vec::new())
    }

    fn transfers(&self) -> Result<Vec<Transfer>> {
        Ok(Vec::new())
    }

    fn cancel(&self, _username: &str, _id: &str) -> Result<()> {
        Ok(())
    }
}

/// Differs only in availability, so every gap is inside the uploader window.
fn from_peer(username: &str, title: &str, queue_length: u32, free_upload_slots: bool) -> Candidate {
    Candidate {
        username: username.to_owned(),
        remote_path: format!(r"@@{username}\Music\{title}.flac"),
        size: 30_000_000,
        bit_depth: Some(16),
        sample_rate: Some(44100),
        free_upload_slots,
        queue_length,
        ..Candidate::default()
    }
}

const BATCH: [&str; 3] = ["roki", "ghost rule", "sanbika"];

fn batch_tracks() -> Vec<Track> {
    BATCH
        .iter()
        .map(|title| Track::new("DECO*27", title))
        .collect()
}

fn batch_client(extra_fillers: bool) -> PerTrackClient {
    PerTrackClient {
        lists: BATCH
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let mut list = vec![from_peer(&format!("solo{index}"), title, 0, true)];
                if extra_fillers {
                    // Only `bulk` covers the batch, and falls past a cap of two.
                    list.push(from_peer(&format!("fillerA{index}"), title, 300, true));
                    list.push(from_peer(&format!("fillerB{index}"), title, 600, true));
                }
                list.push(from_peer("bulk", title, 0, false));
                ((*title).to_owned(), list)
            })
            .collect(),
    }
}

fn queued_with(store: &mut ReviewStore, tracks: &[Track]) {
    store.add(tracks).expect("added tracks");
}

#[test]
fn a_peer_holding_most_of_the_batch_is_offered_first_for_all_of_them() {
    let scratch = Scratch::new("fetch-prefer-peer");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued_with(&mut store, &batch_tracks());

    let outcomes =
        fetch::search_pending(&mut store, &batch_client(false), &settings, None).expect("a search");

    assert!(outcomes
        .iter()
        .all(|outcome| outcome.preferred_uploader == "bulk"));
    for entry in store.tracks(&[], None).expect("the queued tracks") {
        assert_eq!(entry.candidates[0].candidate.username, "bulk");
    }
}

#[test]
fn the_batch_pass_sees_candidates_below_the_shown_count() {
    // Regression: ranking truncated per track before the batch pass could see `bulk`.
    let scratch = Scratch::new("fetch-prefer-deep");
    let mut settings = settings(&scratch);
    settings.review.candidates_per_track = 2;
    let mut store = store(&scratch);
    queued_with(&mut store, &batch_tracks());

    fetch::search_pending(&mut store, &batch_client(true), &settings, None).expect("a search");

    for entry in store.tracks(&[], None).expect("the queued tracks") {
        assert!(
            entry.candidates.len() > 2,
            "kept beyond what the picker shows"
        );
        assert_eq!(entry.candidates[0].candidate.username, "bulk");
    }
}

/// `bulk` a close second on every track; `bulk2` a further third on those in `second`.
fn steered_client(second: &[&str]) -> PerTrackClient {
    PerTrackClient {
        lists: BATCH
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let mut list = vec![
                    from_peer(&format!("solo{index}"), title, 0, true),
                    from_peer("bulk", title, 0, false),
                ];
                if second.contains(title) {
                    list.push(from_peer("bulk2", title, 100, false));
                }
                ((*title).to_owned(), list)
            })
            .collect(),
    }
}

fn steered_and_queued(
    store: &mut ReviewStore,
    settings: &Settings,
    client: &PerTrackClient,
    first: usize,
    later: f64,
) -> Vec<i64> {
    queued_with(store, &batch_tracks());
    fetch::search_pending(store, client, settings, None).expect("a search");
    let ids: Vec<i64> = store
        .tracks(&[], None)
        .expect("tracks")
        .iter()
        .map(|entry| entry.id)
        .collect();
    for id in &ids {
        store.approve(*id, 1).expect("an approval");
        assert_eq!(approved_peer(store, *id), "bulk");
    }
    fetch::enqueue_approved(store, &FakeClient::default(), Some(first), 1000.0)
        .expect("an enqueue");
    fetch::enqueue_approved(store, &FakeClient::default(), None, later).expect("an enqueue");
    ids
}

fn batch_transfer(username: &str, title: &str, state: TransferState, error: &str) -> Transfer {
    Transfer {
        id: format!("{username}:{title}"),
        username: username.to_owned(),
        ..transfer(&format!(r"@@{username}\Music\{title}.flac"), state, error)
    }
}

fn every_batch_transfer(username: &str, state: TransferState) -> Vec<Transfer> {
    BATCH
        .iter()
        .map(|title| batch_transfer(username, title, state, ""))
        .collect()
}

fn approved_candidate(store: &ReviewStore, track_id: i64) -> ScoredCandidate {
    store
        .track(track_id)
        .expect("a lookup")
        .expect("the track")
        .approved()
        .expect("an approval")
        .clone()
}

fn approved_peer(store: &ReviewStore, track_id: i64) -> String {
    approved_candidate(store, track_id).candidate.username
}

fn cancelled_ids(client: &FakeClient) -> Vec<String> {
    client
        .cancelled
        .lock()
        .expect("the fake is not poisoned")
        .iter()
        .map(|(_, id)| id.clone())
        .collect()
}

fn stalled_after(settings: &Settings, queued_at: f64) -> f64 {
    queued_at + settings.slskd.stall_after_seconds + 1.0
}

#[test]
fn a_preferred_peer_stalling_on_every_file_moves_its_whole_group_together() {
    let scratch = Scratch::new("fetch-group-stall");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let ids = steered_and_queued(&mut store, &settings, &steered_client(&BATCH), 3, 1000.0);
    let client = FakeClient::with_transfers(every_batch_transfer("bulk", TransferState::Queued));

    let stalled = fetch::retry_stalled(
        &mut store,
        &client,
        &settings,
        stalled_after(&settings, 1000.0),
    )
    .expect("a retry");

    assert_eq!(
        stalled.peers,
        vec![fetch::PeerStall {
            username: "bulk".to_owned(),
            moved_to: Some("bulk2".to_owned()),
            moved: 3,
            individually: 0,
            exhausted: 0,
        }]
    );
    assert_eq!(cancelled_ids(&client).len(), 3);
    assert_eq!(stalled.tracks.len(), 3);
    for (outcome, id) in stalled.tracks.iter().zip(&ids) {
        assert_eq!(outcome.peer_group, "bulk");
        // The tried `bulk` row is renumbered first, so the move lands on rank 2.
        assert_eq!(outcome.retried_with_rank, Some(2));
        let approved = approved_candidate(&store, *id);
        assert_eq!(approved.candidate.username, "bulk2");
        assert!(approved
            .flags
            .iter()
            .any(|flag| flag == stk::soulseek::ranking::PREFERRED_FLAG));
        assert_eq!(status(&store, *id), TrackStatus::Approved);
    }
}

#[test]
fn a_peer_stalling_on_one_file_while_delivering_the_rest_is_not_down() {
    let scratch = Scratch::new("fetch-group-one-stall");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let ids = steered_and_queued(&mut store, &settings, &steered_client(&BATCH), 3, 1000.0);
    let client = FakeClient::with_transfers(vec![
        batch_transfer("bulk", BATCH[0], TransferState::Queued, ""),
        batch_transfer("bulk", BATCH[1], TransferState::Completed, ""),
        batch_transfer("bulk", BATCH[2], TransferState::Completed, ""),
    ]);
    collect(&mut store, &client, &settings, false).expect("a collect");

    let stalled = fetch::retry_stalled(
        &mut store,
        &client,
        &settings,
        stalled_after(&settings, 1000.0),
    )
    .expect("a retry");

    assert!(stalled.peers.is_empty());
    assert_eq!(stalled.tracks.len(), 1);
    assert!(stalled.tracks[0].peer_group.is_empty());
    assert_eq!(cancelled_ids(&client), vec![format!("bulk:{}", BATCH[0])]);
    assert_eq!(approved_peer(&store, ids[0]), "solo0");
}

#[test]
fn a_peer_slskd_reports_offline_moves_its_group_on_a_single_stall() {
    let scratch = Scratch::new("fetch-group-offline");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let now = stalled_after(&settings, 1000.0);
    let ids = steered_and_queued(
        &mut store,
        &settings,
        &steered_client(&BATCH),
        1,
        now - 10.0,
    );
    let offline = FakeClient::with_transfers(vec![
        batch_transfer("bulk", BATCH[0], TransferState::Queued, ""),
        batch_transfer("bulk", BATCH[1], TransferState::Queued, ""),
        batch_transfer(
            "bulk",
            BATCH[2],
            TransferState::Failed,
            "User bulk appears to be offline",
        ),
    ]);

    let stalled = fetch::retry_stalled(&mut store, &offline, &settings, now).expect("a retry");

    assert_eq!(stalled.peers.len(), 1);
    assert_eq!(stalled.peers[0].moved, 2);
    assert_eq!(
        cancelled_ids(&offline),
        vec![format!("bulk:{}", BATCH[0]), format!("bulk:{}", BATCH[1])]
    );
    assert_eq!(approved_peer(&store, ids[0]), "bulk2");
    assert_eq!(approved_peer(&store, ids[1]), "bulk2");
    // A failed transfer is `collect`'s to retry, not the stall pass's.
    assert_eq!(status(&store, ids[2]), TrackStatus::Queued);
}

#[test]
fn a_moved_group_never_goes_back_to_the_peer_that_stalled() {
    let scratch = Scratch::new("fetch-group-no-return");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let ids = steered_and_queued(&mut store, &settings, &steered_client(&BATCH), 3, 1000.0);
    let first = stalled_after(&settings, 1000.0);
    fetch::retry_stalled(
        &mut store,
        &FakeClient::with_transfers(every_batch_transfer("bulk", TransferState::Queued)),
        &settings,
        first,
    )
    .expect("a retry");

    fetch::enqueue_approved(&mut store, &FakeClient::default(), None, first).expect("an enqueue");
    let second = stalled_after(&settings, first);
    let stalled = fetch::retry_stalled(
        &mut store,
        &FakeClient::with_transfers(every_batch_transfer("bulk2", TransferState::Queued)),
        &settings,
        second,
    )
    .expect("a retry");
    assert_eq!(stalled.peers[0].username, "bulk2");
    assert_eq!(stalled.peers[0].moved_to, None);
    assert_eq!(stalled.peers[0].individually, 3);
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(approved_peer(&store, *id), format!("solo{index}"));
    }

    // `bulk` is never offered back.
    fetch::enqueue_approved(&mut store, &FakeClient::default(), None, second).expect("an enqueue");
    let solos = FakeClient::with_transfers(
        BATCH
            .iter()
            .enumerate()
            .map(|(index, title)| {
                batch_transfer(&format!("solo{index}"), title, TransferState::Queued, "")
            })
            .collect(),
    );
    let stalled = fetch::retry_stalled(
        &mut store,
        &solos,
        &settings,
        stalled_after(&settings, second),
    )
    .expect("a retry");
    assert!(stalled.peers.is_empty());
    assert!(stalled
        .tracks
        .iter()
        .all(|outcome| outcome.retried_with_rank.is_none()));
    for id in &ids {
        assert_eq!(status(&store, *id), TrackStatus::Failed);
    }
}

fn from_folder(username: &str, remote_path: &str, score: i64) -> ScoredCandidate {
    ScoredCandidate {
        candidate: Candidate {
            username: username.to_owned(),
            remote_path: remote_path.to_owned(),
            size: 30_000_000,
            bit_depth: Some(16),
            sample_rate: Some(44100),
            free_upload_slots: true,
            ..Candidate::default()
        },
        score,
        title_ratio: 0.99,
        flags: Vec::new(),
    }
}

fn album_in_review(store: &mut ReviewStore) {
    for title in ["Roki", "Ghost Rule"] {
        store
            .add(&[Track::new("DECO*27", title)])
            .expect("an added track");
        let track_id = store
            .tracks(&[], None)
            .expect("tracks")
            .into_iter()
            .find(|entry| entry.track.title == title)
            .expect("the track just added")
            .id;
        store
            .set_candidates(
                track_id,
                &[
                    from_folder("bulk", &format!(r"@@bulk\Music\Best Of\{title}.flac"), 100),
                    from_folder("solo", &format!(r"@@solo\Music\{title}.flac"), 90),
                ],
            )
            .expect("recorded candidates");
    }
}

#[test]
fn an_album_approval_queues_and_collects_each_track_on_its_own() {
    let scratch = Scratch::new("fetch-album");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    album_in_review(&mut store);

    let waiting = store
        .tracks(&[TrackStatus::AwaitingReview], None)
        .expect("the waiting tracks");
    let offer = albums::offers(&waiting, 2)
        .into_iter()
        .find(|offer| offer.username == "bulk")
        .expect("the album offer");
    store.approve_all(&offer.picks()).expect("an approval");

    let enqueuer = FakeClient::default();
    let queued = fetch::enqueue_approved(&mut store, &enqueuer, None, 1000.0).expect("an enqueue");

    assert_eq!(queued.len(), 2);
    assert!(queued.iter().all(|outcome| outcome.queued));
    assert_eq!(
        *enqueuer.enqueued.lock().expect("the fake"),
        vec![("bulk".to_owned(), "DECO_27".to_owned()); 2]
    );

    let client = FakeClient::with_transfers(vec![
        transfer(
            r"@@bulk\Music\Best Of\Roki.flac",
            TransferState::Completed,
            "",
        ),
        transfer(
            r"@@bulk\Music\Best Of\Ghost Rule.flac",
            TransferState::Failed,
            "peer gone",
        ),
    ]);
    let outcomes = collect(&mut store, &client, &settings, false).expect("a collect");

    assert_eq!(outcomes.len(), 2);
    assert_eq!(
        outcomes[0].local_path,
        Some(scratch.join("downloads").join("DECO_27").join("Roki.flac"))
    );
    let stored = store.tracks(&[], None).expect("tracks");
    assert_eq!(stored[0].status, TrackStatus::Downloaded);
    // Only the failed file falls back, to its own next candidate.
    assert_eq!(stored[1].status, TrackStatus::Approved);
    assert_eq!(
        stored[1]
            .approved()
            .expect("the promoted candidate")
            .candidate
            .username,
        "solo"
    );
}

fn peer_tagged(path: &std::path::Path) {
    encode(
        path,
        &Encoded {
            metadata: &[
                ("artist", "Unknown Artist"),
                ("title", "Track 01"),
                ("album", "www.something.ru"),
                ("albumartist", "Various"),
                ("track", "7"),
                ("genre", "Vocaloid"),
                ("comment", "ripped by someone"),
                ("lyrics", "la la la"),
                ("date", "2016-11-02"),
            ],
            ..Encoded::default()
        },
    );
    tags::write_artwork(
        path,
        &stk::models::Artwork {
            data: vec![0xAB; 512],
            mime: "image/jpeg".to_owned(),
        },
    )
    .expect("an embedded cover");
}

fn roki_with_album() -> Track {
    Track {
        album: "Ghost".to_owned(),
        ..roki()
    }
}

fn arrived(store: &mut ReviewStore, settings: &Settings, track: &Track) -> (i64, PathBuf) {
    store
        .add(std::slice::from_ref(track))
        .expect("an added track");
    let client = FakeClient::with_candidates(vec![candidate("@@peerA\\Music\\Roki.flac")]);
    fetch::search_pending(store, &client, settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");
    fetch::enqueue_approved(store, &FakeClient::default(), None, 1000.0).expect("an enqueue");

    let path = scratch_download(settings);
    peer_tagged(&path);
    (track_id, path)
}

fn scratch_download(settings: &Settings) -> PathBuf {
    settings
        .slskd
        .download_directory
        .clone()
        .expect("a download directory")
        .join("DECO_27")
        .join("Roki.flac")
}

fn completed() -> FakeClient {
    FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )])
}

#[test]
fn a_completed_download_is_tagged_from_its_review_row() {
    let scratch = Scratch::new("fetch-tag-replace");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.artist, "DECO*27");
    assert_eq!(read.title, "Roki");
    assert_eq!(read.album, "Ghost");
    let retagged = outcome.tags.as_ref().expect("a tag outcome");
    assert_eq!(retagged.policy, TagPolicy::Replace);
    assert_eq!(
        retagged.fields,
        vec![TagField::Artist, TagField::Album, TagField::Title]
    );
    assert_eq!(retagged.before.artist, "Unknown Artist");
    assert!(retagged.error.is_empty(), "{}", retagged.error);
}

#[test]
fn a_typed_name_only_fills_even_where_downloads_replace() {
    let scratch = Scratch::new("fetch-tag-typed");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let typed = Track {
        typed: true,
        ..roki_with_album()
    };
    let (_, path) = arrived(&mut store, &settings, &typed);

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.title, "Track 01");
    assert_eq!(read.artist, "Unknown Artist");
    assert_eq!(
        outcome.tags.as_ref().expect("a tag outcome").policy,
        TagPolicy::Fill
    );
}

#[test]
fn a_download_still_being_moved_into_place_waits_for_the_next_collect() {
    let scratch = Scratch::new("fetch-partial-file");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (track_id, path) = arrived(&mut store, &settings, &roki_with_album());
    let whole = std::fs::metadata(&path).expect("the file").len();
    let moving = FakeClient::with_transfers(vec![Transfer {
        size: whole + 1,
        ..transfer("@@peerA\\Music\\Roki.flac", TransferState::Completed, "")
    }]);

    let outcomes = collect(&mut store, &moving, &settings, false).expect("a collect");

    assert!(outcomes.is_empty());
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::Queued);
    assert_eq!(
        tags::read_tags(&path).expect("readable tags").artist,
        "Unknown Artist"
    );

    let outcomes = collect(&mut store, &completed(), &settings, false).expect("a collect");
    assert_eq!(outcomes.len(), 1);
}

#[test]
fn a_finished_download_missing_from_the_download_directory_is_an_error() {
    let scratch = Scratch::new("fetch-missing-file");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());
    std::fs::remove_file(&path).expect("the file gone");

    let outcomes = collect(&mut store, &completed(), &settings, false).expect("a collect");

    assert!(
        outcomes[0].error.contains("slskd.download_directory"),
        "{}",
        outcomes[0].error
    );
    assert!(fetch::downloads_went_wrong(&[], &outcomes));
}

#[test]
fn replace_keeps_what_the_row_cannot_say_and_drops_the_rest() {
    let scratch = Scratch::new("fetch-tag-kept");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.album_artist, "Various");
    assert_eq!(read.track_number, Some(7));
    assert_eq!(read.genres, vec!["Vocaloid".to_owned()]);
    let cover = tags::read_artwork(&path).expect("readable tags");
    assert_eq!(cover.expect("the peer's cover").data.len(), 512);
    // Comments and lyrics go; the date is about the music and stays.
    let kept: Vec<String> = tags::read_extras(&path)
        .expect("readable extras")
        .into_iter()
        .map(|extra| extra.key.to_uppercase())
        .collect();
    assert_eq!(kept, vec!["DATE".to_owned()]);
    assert_eq!(outcome.tags.as_ref().expect("a tag outcome").dropped, 2);
}

#[test]
fn undoing_the_collect_restores_the_peers_block() {
    let scratch = Scratch::new("fetch-tag-undo");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());
    collect(&mut store, &completed(), &settings, false).expect("a collect");

    let mut index = index(&settings);
    let roots = settings.library.all_roots();
    let undo = ChangeLog::open(&index, "changes undo", 3000.0).expect("a run");
    for change in index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
    {
        let plan = plan_undo(&index, change).expect("a plan");
        assert!(plan.ready(), "{:?}", plan.blocked);
        apply_undo(&mut index, &plan, &undo, &roots).expect("an undo");
    }

    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.artist, "Unknown Artist");
    assert_eq!(read.title, "Track 01");
    assert_eq!(read.album, "www.something.ru");
    let extras = tags::read_extras(&path).expect("readable extras");
    assert_eq!(extras.len(), 3);
    assert!(extras
        .iter()
        .any(|extra| extra.value == "ripped by someone"));
}

#[test]
fn fill_writes_only_what_the_peer_left_empty() {
    let scratch = Scratch::new("fetch-tag-fill");
    let mut settings = settings(&scratch);
    settings.fetch.tags = TagPolicy::Fill;
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());
    let mut cleared = tags::read_tags(&path).expect("readable tags");
    cleared.title = String::new();
    tags::write_tags(&path, &cleared).expect("a cleared title");

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.title, "Roki");
    assert_eq!(read.artist, "Unknown Artist");
    assert_eq!(read.album, "www.something.ru");
    assert_eq!(tags::read_extras(&path).expect("readable extras").len(), 3);
    let retagged = outcome.tags.as_ref().expect("a tag outcome");
    assert_eq!(retagged.fields, vec![TagField::Title]);
    assert_eq!(retagged.dropped, 0);
}

#[test]
fn keep_leaves_the_file_and_the_log_alone() {
    let scratch = Scratch::new("fetch-tag-keep");
    let mut settings = settings(&scratch);
    settings.fetch.tags = TagPolicy::Keep;
    let mut store = store(&scratch);
    let (track_id, path) = arrived(&mut store, &settings, &roki_with_album());

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    assert_eq!(outcome.tags, None);
    assert_eq!(
        tags::read_tags(&path).expect("readable tags").artist,
        "Unknown Artist"
    );
    assert!(index(&settings)
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .is_empty());
    assert_eq!(
        store
            .track(track_id)
            .expect("a lookup")
            .expect("the track")
            .status,
        TrackStatus::Downloaded
    );
}

#[test]
fn a_download_inside_the_library_is_reindexed_after_the_write() {
    let scratch = Scratch::new("fetch-tag-inside");
    let mut settings = settings(&scratch);
    settings.slskd.download_directory = Some(settings.library.root.clone());
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());

    collect(&mut store, &completed(), &settings, false).expect("a collect");

    let entry = index(&settings)
        .entry(&path)
        .expect("a lookup")
        .expect("an index row");
    assert_eq!(entry.tags.artist, "DECO*27");
    assert_eq!(entry.tags.title, "Roki");
}

#[derive(Default)]
struct LiveClient {
    transfers: Mutex<Vec<Transfer>>,
    enqueued: Mutex<Vec<String>>,
    gone: std::sync::atomic::AtomicBool,
}

impl LiveClient {
    fn show(&self, transfers: Vec<Transfer>) {
        *self.transfers.lock().expect("the fake is not poisoned") = transfers;
    }
}

impl SoulseekClient for LiveClient {
    fn is_connected(&self) -> bool {
        true
    }

    fn search(&self, _text: &str, _timeout: u32, _limit: u32) -> Result<Vec<Candidate>> {
        Ok(Vec::new())
    }

    fn enqueue(&self, username: &str, _files: &[Candidate], _to: &str) -> Result<Vec<Transfer>> {
        self.enqueued
            .lock()
            .expect("the fake is not poisoned")
            .push(username.to_owned());
        Ok(Vec::new())
    }

    fn transfers(&self) -> Result<Vec<Transfer>> {
        if self.gone.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::SoulseekClient("cannot reach slskd".to_owned()));
        }
        Ok(self
            .transfers
            .lock()
            .expect("the fake is not poisoned")
            .clone())
    }

    fn cancel(&self, _username: &str, _transfer_id: &str) -> Result<()> {
        Ok(())
    }
}

/// Time passes only when the wait sleeps, so the test decides what each poll sees.
struct TickingClock<'a> {
    now: f64,
    sleeps: usize,
    on_sleep: Box<dyn FnMut(usize) + 'a>,
}

impl<'a> TickingClock<'a> {
    fn new(on_sleep: impl FnMut(usize) + 'a) -> Self {
        Self {
            now: 1000.0,
            sleeps: 0,
            on_sleep: Box::new(on_sleep),
        }
    }
}

impl fetch::Clock for TickingClock<'_> {
    fn now(&self) -> f64 {
        self.now
    }

    fn sleep(&mut self, seconds: f64) {
        self.now += seconds;
        self.sleeps += 1;
        (self.on_sleep)(self.sleeps);
    }
}

const PATIENCE: fetch::Patience = fetch::Patience {
    timeout_seconds: 600.0,
    poll_seconds: 10.0,
};

fn from_user(username: &str, title: &str) -> Candidate {
    Candidate {
        username: username.to_owned(),
        ..candidate(&format!("@@{username}\\Music\\{title}.flac"))
    }
}

fn arrived_from(username: &str) -> Transfer {
    Transfer {
        username: username.to_owned(),
        ..transfer(
            &format!("@@{username}\\Music\\Roki.flac"),
            TransferState::Completed,
            "",
        )
    }
}

fn approved(store: &mut ReviewStore, settings: &Settings, track: &Track, peers: &[&str]) -> i64 {
    store
        .add(std::slice::from_ref(track))
        .expect("an added track");
    let client = FakeClient::with_candidates(
        peers
            .iter()
            .map(|peer| from_user(peer, &track.title))
            .collect(),
    );
    fetch::search_pending(store, &client, settings, None).expect("a search");
    let track_id = store
        .tracks(&[], None)
        .expect("tracks")
        .into_iter()
        .find(|entry| entry.track.title == track.title)
        .expect("the track just added")
        .id;
    store.approve(track_id, 1).expect("an approval");
    track_id
}

fn wait(
    store: &mut ReviewStore,
    client: &dyn SoulseekClient,
    settings: &Settings,
    limit: Option<usize>,
    patience: fetch::Patience,
    clock: &mut dyn fetch::Clock,
) -> fetch::Waited {
    let mut index = index(settings);
    fetch::wait_for_downloads(
        store,
        client,
        settings,
        &mut index,
        &settings.library.all_roots(),
        fetch::Scope::Queue { limit },
        patience,
        clock,
        &mut |_| {},
    )
    .expect("a wait")
}

fn status(store: &ReviewStore, track_id: i64) -> TrackStatus {
    store
        .track(track_id)
        .expect("a lookup")
        .expect("the track")
        .status
}

#[test]
fn a_stall_during_the_wait_sends_the_next_candidate_in_the_same_wait() {
    let scratch = Scratch::new("fetch-wait-stall");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = approved(&mut store, &settings, &roki(), &["peerA", "peerB"]);
    let client = LiveClient::default();
    client.show(vec![Transfer {
        state: TransferState::Queued,
        ..arrived_from("peerA")
    }]);
    let mut clock = TickingClock::new(|sleeps| {
        if sleeps == 2 {
            client.show(vec![arrived_from("peerB")]);
        }
    });
    let patience = fetch::Patience {
        timeout_seconds: 3.0 * settings.slskd.stall_after_seconds,
        poll_seconds: settings.slskd.stall_after_seconds + 1.0,
    };

    let waited = wait(&mut store, &client, &settings, None, patience, &mut clock);

    assert_eq!(waited.stalled.len(), 1);
    assert_eq!(waited.stalled[0].retried_with_rank, Some(2));
    assert_eq!(
        *client.enqueued.lock().expect("the fake is not poisoned"),
        vec!["peerA".to_owned(), "peerB".to_owned()]
    );
    assert_eq!(waited.unfinished, 0);
    assert_eq!(status(&store, track_id), TrackStatus::Downloaded);
}

#[test]
fn slskd_going_away_ends_the_wait_with_what_already_arrived() {
    let scratch = Scratch::new("fetch-wait-slskd-gone");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let arrived_id = approved(&mut store, &settings, &roki(), &["peerA"]);
    let pending_id = approved(
        &mut store,
        &settings,
        &Track::new("DECO*27", "Ghost Rule"),
        &["peerB"],
    );
    let client = LiveClient::default();
    client.show(vec![arrived_from("peerA")]);
    let mut clock = TickingClock::new(|_| {
        client.gone.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let waited = wait(&mut store, &client, &settings, None, PATIENCE, &mut clock);

    assert_eq!(waited.slskd_lost.as_deref(), Some("cannot reach slskd"));
    assert_eq!(waited.collected.len(), 1);
    assert_eq!(waited.unfinished, 1);
    assert_eq!(status(&store, arrived_id), TrackStatus::Downloaded);
    assert_eq!(status(&store, pending_id), TrackStatus::Queued);
}

#[test]
fn the_wait_gives_up_after_its_timeout_and_leaves_the_track_queued() {
    let scratch = Scratch::new("fetch-wait-timeout");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = approved(&mut store, &settings, &roki(), &["peerA"]);
    let client = LiveClient::default();
    let mut clock = TickingClock::new(|_| {});
    let patience = fetch::Patience {
        timeout_seconds: 30.0,
        poll_seconds: 10.0,
    };

    let waited = wait(&mut store, &client, &settings, None, patience, &mut clock);

    assert_eq!(waited.rounds, 4);
    assert_eq!(clock.sleeps, 3);
    assert_eq!(waited.unfinished, 1);
    assert_eq!(status(&store, track_id), TrackStatus::Queued);
}

#[test]
fn a_track_queued_before_the_wait_began_is_waited_for_too() {
    let scratch = Scratch::new("fetch-wait-leftover");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = queued(&mut store, &settings);
    let client = LiveClient::default();
    let mut clock = TickingClock::new(|_| client.show(vec![arrived_from("peerA")]));

    let waited = wait(&mut store, &client, &settings, None, PATIENCE, &mut clock);

    assert!(waited.queued.is_empty());
    assert_eq!(waited.collected.len(), 1);
    assert_eq!(status(&store, track_id), TrackStatus::Downloaded);
}

fn collected(
    error: &str,
    retried_with_rank: Option<i64>,
    verdict: Option<Verdict>,
) -> fetch::CollectOutcome {
    fetch::CollectOutcome {
        track_id: 1,
        query: "DECO*27 - Roki".to_owned(),
        state: TransferState::Completed,
        local_path: None,
        verdict: verdict.map(|verdict| stk::models::VerifyResult {
            path: PathBuf::from("Roki.flac"),
            verdict,
            detail: String::new(),
            sample_rate: None,
            bit_depth: None,
            codec: String::new(),
            cutoff_hz: None,
            nyquist_hz: None,
            steepness_db_per_khz: None,
        }),
        tags: None,
        error: error.to_owned(),
        retried_with_rank,
    }
}

fn waited_with(collected: Vec<fetch::CollectOutcome>) -> fetch::Waited {
    fetch::Waited {
        collected,
        ..fetch::Waited::default()
    }
}

#[test]
fn what_counts_as_the_run_going_wrong() {
    let cases = [
        (collected("", None, Some(Verdict::Ok)), false),
        (collected("", None, Some(Verdict::Fake)), true),
        // A peer's copy that stops decoding before its end.
        (collected("", None, Some(Verdict::Error)), true),
        (collected("peer gone", Some(2), None), false),
        (collected("peer gone", None, None), true),
    ];
    for (outcome, wrong) in cases {
        let error = outcome.error.clone();
        assert_eq!(waited_with(vec![outcome]).went_wrong(), wrong, "{error:?}");
    }

    let unqueued = fetch::Waited {
        queued: vec![fetch::EnqueueOutcome {
            track_id: 1,
            query: "DECO*27 - Roki".to_owned(),
            destination: "DECO_27".to_owned(),
            queued: false,
            error: "slskd unreachable".to_owned(),
        }],
        ..fetch::Waited::default()
    };
    assert!(unqueued.went_wrong());

    let timed_out = fetch::Waited {
        unfinished: 3,
        ..fetch::Waited::default()
    };
    assert!(!timed_out.went_wrong(), "a timeout alone is not a failure");
}

#[test]
fn a_peer_gone_offline_hands_the_track_to_the_next_candidate() {
    let scratch = Scratch::new("fetch-offline-peer");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = approved(&mut store, &settings, &roki(), &["peerA", "peerB"]);
    let first = approved_peer(&store, track_id);
    let client = FakeClient {
        offline: Some(first.clone()),
        ..FakeClient::default()
    };

    let outcomes = fetch::enqueue_approved(&mut store, &client, None, 1000.0).expect("an enqueue");

    assert!(outcomes[0].queued, "{:?}", outcomes[0].error);
    assert_ne!(approved_peer(&store, track_id), first);
}

#[test]
fn enqueueing_named_tracks_leaves_the_rest_of_the_queue_alone() {
    let scratch = Scratch::new("fetch-named-enqueue");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let asked = approved(&mut store, &settings, &roki(), &["peerA"]);
    let earlier = approved(
        &mut store,
        &settings,
        &Track::new("DECO*27", "Ghost Rule"),
        &["peerA"],
    );

    fetch::enqueue_tracks(&mut store, &FakeClient::default(), &[asked], 1000.0)
        .expect("an enqueue");

    let status = |id| {
        store
            .track(id)
            .expect("a lookup")
            .expect("the track")
            .status
    };
    assert_eq!(status(asked), TrackStatus::Queued);
    assert_eq!(status(earlier), TrackStatus::Approved);
}

#[test]
fn a_track_queued_again_after_a_stall_is_reported_once_as_its_last_attempt() {
    let attempt = |track_id: i64, queued: bool, error: &str| fetch::EnqueueOutcome {
        track_id,
        query: format!("track {track_id}"),
        destination: String::new(),
        queued,
        error: error.to_owned(),
    };
    let queued = [
        attempt(1, true, ""),
        attempt(2, true, ""),
        attempt(1, false, "peer offline"),
        attempt(1, true, ""),
    ];

    let latest = fetch::latest_per_track(&queued);

    assert_eq!(latest.len(), 2);
    assert_eq!((latest[0].0, latest[0].1), (&queued[3], 3));
    assert_eq!((latest[1].0, latest[1].1), (&queued[1], 1));
}
