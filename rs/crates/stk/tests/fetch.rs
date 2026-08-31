//! Ported from `py/tests/unit/test_fetch.py`, plus the retry path `review.db`
//! v2 adds (#2).
//!
//! The whole flow runs against a fake implementing `SoulseekClient`: no network,
//! no daemon, no Soulseek account. Preserving that boundary is what makes these
//! tests runnable in CI.

mod support;

use std::path::PathBuf;
use std::sync::Mutex;

use stk::config::Settings;
use stk::errors::{Error, Result};
use stk::fetch::{self, downloaded_path};
use stk::models::{Candidate, Track, TrackStatus, Verdict};
use stk::review::store::ReviewStore;
use stk::soulseek::base::{SoulseekClient, Transfer, TransferState};
use support::{encode, noise_source, transcode_to_flac, Encoded, Scratch};

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

#[derive(Default)]
struct FakeClient {
    candidates: Vec<Candidate>,
    transfers: Vec<Transfer>,
    search_error: Option<String>,
    enqueue_error: Option<String>,
    // `Mutex` rather than `RefCell`: the client boundary is `Sync` now that
    // searches fan out, and a fake has to satisfy the same bound.
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
        match &self.search_error {
            Some(error) => Err(Error::SoulseekClient(error.clone())),
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
            ..stk::config::SlskdSettings::default()
        },
        ..Settings::default()
    }
}

fn store(scratch: &Scratch) -> ReviewStore {
    ReviewStore::open(&scratch.join("review.db")).expect("a store")
}

fn roki() -> Track {
    Track::new("DECO*27", "Roki")
}

/// A track searched, approved and queued, ready for `collect`.
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
fn the_search_text_is_normalised() {
    let scratch = Scratch::new("fetch-normalised");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");

    let client = FakeClient::default();
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");

    assert_eq!(
        client.searched.lock().expect("the fake").as_slice(),
        ["deco 27 roki"]
    );
}

#[test]
fn only_pending_tracks_are_searched() {
    let scratch = Scratch::new("fetch-pending-only");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store
        .add(&[roki(), Track::new("ZUN", "Heian Alien")])
        .expect("added tracks");
    let first = store.tracks(&[], None).expect("tracks")[0].id;
    store
        .mark(first, TrackStatus::Downloaded, "")
        .expect("a mark");

    let outcomes = fetch::search_pending(&mut store, &FakeClient::default(), &settings, None)
        .expect("a search");

    assert_eq!(outcomes.len(), 1);
}

#[test]
fn a_track_with_no_results_is_marked_rather_than_left_pending() {
    let scratch = Scratch::new("fetch-no-results");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");

    fetch::search_pending(&mut store, &FakeClient::default(), &settings, None).expect("a search");

    assert_eq!(
        store.tracks(&[], None).expect("tracks")[0].status,
        TrackStatus::NoCandidates
    );
}

#[test]
fn the_number_of_candidates_kept_is_configurable() {
    let scratch = Scratch::new("fetch-candidate-limit");
    let mut settings = settings(&scratch);
    settings.review.candidates_per_track = 1;
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");

    let client = FakeClient::with_candidates(vec![
        candidate("@@peerA\\Music\\Roki.flac"),
        candidate("@@peerB\\Music\\Roki.flac"),
    ]);
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");

    assert_eq!(
        store.tracks(&[], None).expect("tracks")[0].candidates.len(),
        1
    );
}

#[test]
fn a_backend_failure_marks_the_track_and_continues() {
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
        .all(|entry| entry.status == TrackStatus::Failed));
    assert!(
        outcomes[0].error.contains("unreachable"),
        "{:?}",
        outcomes[0]
    );
}

#[test]
fn the_number_searched_can_be_capped() {
    let scratch = Scratch::new("fetch-search-cap");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let tracks: Vec<Track> = (0..5)
        .map(|index| Track::new("A", &index.to_string()))
        .collect();
    store.add(&tracks).expect("added tracks");

    let outcomes = fetch::search_pending(&mut store, &FakeClient::default(), &settings, Some(2))
        .expect("a search");

    assert_eq!(outcomes.len(), 2);
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
    assert_eq!(enqueuer.enqueued.lock().expect("the fake")[0].0, "peerA");
}

#[test]
fn the_destination_folder_is_safe_for_the_filesystem() {
    let scratch = Scratch::new("fetch-destination");
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

    assert_eq!(outcomes[0].destination, "DECO_27");
}

#[test]
fn the_track_moves_to_queued() {
    let scratch = Scratch::new("fetch-queued");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = queued(&mut store, &settings);

    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::Queued);
    // v2 (#2): the queue time is recorded, so a stall can be seen later.
    assert_eq!(entry.queued_at, Some(1000.0));
}

#[test]
fn nothing_unapproved_is_queued() {
    let scratch = Scratch::new("fetch-unapproved");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    fetch::search_pending(&mut store, &FakeClient::default(), &settings, None).expect("a search");

    let outcomes =
        fetch::enqueue_approved(&mut store, &FakeClient::default(), None, 1000.0).expect("no work");

    assert!(outcomes.is_empty());
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
fn nothing_is_recorded_while_a_transfer_is_still_running() {
    let scratch = Scratch::new("fetch-running");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::InProgress,
        "",
    )]);

    assert!(fetch::collect(&mut store, &client, &settings, true)
        .expect("a collect")
        .is_empty());
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
    let outcomes = fetch::collect(&mut store, &client, &settings, true).expect("a collect");

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
fn a_failed_transfer_is_recorded_as_a_failure() {
    let scratch = Scratch::new("fetch-failed");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Failed,
        "peer gone",
    )]);
    let outcomes = fetch::collect(&mut store, &client, &settings, true).expect("a collect");

    assert_eq!(outcomes[0].state, TransferState::Failed);
    // One candidate was found, so there is nothing left to promote and the track
    // does end as FAILED -- the retry only has somewhere to go with more.
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::Failed);
    assert!(entry.detail.contains("peer gone"), "{}", entry.detail);
}

/// The retry path from #2: rank 2 was always in the database, and nothing ever
/// reached for it.
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
    let outcomes = fetch::collect(&mut store, &failing, &settings, true).expect("a collect");

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

    assert!(fetch::collect(&mut store, &client, &settings, true)
        .expect("a collect")
        .is_empty());
}

#[test]
fn the_local_path_is_rebuilt_from_the_destination() {
    let scratch = Scratch::new("fetch-local-path");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &fetch::collect(&mut store, &client, &settings, false).expect("a collect")[0];

    assert_eq!(
        outcome.local_path,
        Some(scratch.join("downloads").join("DECO_27").join("Roki.flac"))
    );
}

#[test]
fn no_local_path_is_guessed_without_a_configured_download_directory() {
    let scratch = Scratch::new("fetch-no-download-dir");
    let settings = Settings::default();
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &fetch::collect(&mut store, &client, &settings, false).expect("a collect")[0];

    assert_eq!(outcome.local_path, None);
}

#[test]
fn a_downloaded_lossless_file_is_verified() {
    needs_ffmpeg!();
    let scratch = Scratch::new("fetch-verify");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    noise_source(
        &scratch.join("downloads").join("DECO_27").join("Roki.flac"),
        8,
        44100,
    );

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &fetch::collect(&mut store, &client, &settings, true).expect("a collect")[0];

    assert_eq!(
        outcome.verdict.as_ref().expect("a verdict").verdict,
        Verdict::Ok
    );
}

#[test]
fn a_downloaded_transcode_is_recorded_against_the_track() {
    needs_ffmpeg!();
    let scratch = Scratch::new("fetch-transcode");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = queued(&mut store, &settings);

    let source = noise_source(&scratch.join("source.flac"), 8, 44100);
    let faked = transcode_to_flac(&source, &scratch, "fake", "128k");
    let destination = scratch.join("downloads").join("DECO_27").join("Roki.flac");
    std::fs::create_dir_all(destination.parent().expect("a parent")).expect("a directory");
    std::fs::copy(&faked, &destination).expect("a copied file");

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &fetch::collect(&mut store, &client, &settings, true).expect("a collect")[0];

    assert_eq!(
        outcome.verdict.as_ref().expect("a verdict").verdict,
        Verdict::Fake
    );
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert!(entry.detail.contains("FAKE"), "{}", entry.detail);
}

#[test]
fn a_missing_file_is_recorded_without_a_verdict() {
    let scratch = Scratch::new("fetch-missing-file");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &fetch::collect(&mut store, &client, &settings, true).expect("a collect")[0];

    assert!(outcome.verdict.is_none());
}

#[test]
fn verification_can_be_skipped() {
    needs_ffmpeg!();
    let scratch = Scratch::new("fetch-skip-verify");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    noise_source(
        &scratch.join("downloads").join("DECO_27").join("Roki.flac"),
        8,
        44100,
    );

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &fetch::collect(&mut store, &client, &settings, false).expect("a collect")[0];

    assert!(outcome.verdict.is_none());
}

#[test]
fn a_lossy_download_is_not_put_through_the_lossless_check() {
    needs_ffmpeg!();
    let scratch = Scratch::new("fetch-lossy");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let client = FakeClient::with_candidates(vec![candidate("@@peerA\\Music\\Roki.mp3")]);
    fetch::search_pending(&mut store, &client, &settings, None).expect("a search");
    let track_id = store.tracks(&[], None).expect("tracks")[0].id;
    store.approve(track_id, 1).expect("an approval");
    fetch::enqueue_approved(&mut store, &FakeClient::default(), None, 1000.0).expect("an enqueue");

    encode(
        &scratch.join("downloads").join("DECO_27").join("Roki.mp3"),
        &Encoded::default(),
    );

    let finished = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.mp3",
        TransferState::Completed,
        "",
    )]);
    let outcome = &fetch::collect(&mut store, &finished, &settings, true).expect("a collect")[0];

    assert!(outcome.verdict.is_none());
}

#[test]
fn no_transfers_at_all_does_nothing() {
    let scratch = Scratch::new("fetch-no-transfers");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    assert!(
        fetch::collect(&mut store, &FakeClient::default(), &settings, true)
            .expect("a collect")
            .is_empty()
    );
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

#[test]
fn the_fake_satisfies_the_client_trait() {
    // The boundary that keeps every flow above testable without a daemon.
    let client: &dyn SoulseekClient = &FakeClient::default();
    assert!(client.is_connected());
    assert!(client.transfers().expect("transfers").is_empty());
}

/// The Rust build must not guess a path the Python build would not.
#[test]
fn downloaded_path_needs_a_configured_download_directory() {
    let transfer = transfer("@@peerA\\Music\\Roki.flac", TransferState::Completed, "");
    assert_eq!(
        downloaded_path(&Settings::default(), "DECO*27", &transfer),
        None::<PathBuf>
    );
}

/// The parallelism #1 asks for, established by rendezvous rather than by clock.
///
/// Every slskd search is a socket wait, so the property under test is that two
/// of them are in flight at once. Timing it instead -- a fixed sleep per search
/// against a wall-clock bound -- reads a contended CI runner as a serial one,
/// which is how this test first failed. Here each search blocks until a second
/// joins it: serial execution cannot satisfy that however fast the box is, and
/// concurrent execution cannot miss it however slow.
#[test]
fn searches_run_concurrently_rather_than_one_after_another() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Condvar;

    /// Long enough for a loaded runner to get a second thread in, short enough
    /// that a genuinely serial fan-out fails the suite rather than hanging it.
    /// Paid once: the first search to wait it out settles the question.
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
    // The pool is built with exactly this many threads, so the ceiling is not a
    // timing observation -- it is the bound `search_concurrency` promises.
    assert!(
        peak <= WORKERS,
        "{peak} searches ran at once, above the configured {WORKERS}"
    );
}

/// Results still line up with the tracks they came from, whatever order the
/// threads finished in.
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

    // Only Roki matches the one candidate offered, so the pairing is visible in
    // the results themselves rather than assumed.
    let by_query: std::collections::HashMap<&str, usize> = outcomes
        .iter()
        .map(|outcome| (outcome.query.as_str(), outcome.found))
        .collect();
    assert_eq!(by_query["DECO*27 - Roki"], 1);
    assert_eq!(by_query["ZUN - Heian Alien"], 0);
}

/// #2's other half of the retry: a transfer nobody is uploading never reaches
/// `collect`, because slskd does not consider it finished.
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
    let outcomes = fetch::retry_stalled(&mut store, &client, &settings, now).expect("a retry");

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
    let outcomes = fetch::retry_stalled(&mut store, &client, &settings, now).expect("a retry");

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

#[test]
fn a_stalled_track_with_nothing_left_to_try_fails() {
    let scratch = Scratch::new("fetch-stalled-exhausted");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Queued,
        "",
    )]);
    let now = 1000.0 + settings.slskd.stall_after_seconds + 1.0;
    let outcomes = fetch::retry_stalled(&mut store, &client, &settings, now).expect("a retry");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].retried_with_rank, None);
    let entry = &store.tracks(&[], None).expect("tracks")[0];
    assert_eq!(entry.status, TrackStatus::Failed);
    assert_eq!(entry.detail, "stalled, and no candidates left");
}

/// A finished transfer is `collect`'s business, not the stall pass's.
#[test]
fn a_finished_transfer_is_not_cancelled_by_the_stall_pass() {
    let scratch = Scratch::new("fetch-stalled-finished");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let now = 1000.0 + settings.slskd.stall_after_seconds + 1.0;
    fetch::retry_stalled(&mut store, &client, &settings, now).expect("a retry");

    assert!(client
        .cancelled
        .lock()
        .expect("the fake is not poisoned")
        .is_empty());
}
