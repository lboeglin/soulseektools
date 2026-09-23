//! Ported from `py/tests/unit/test_fetch.py`, plus the retry path `review.db`
//! v2 adds (#2).
//!
//! The whole flow runs against a fake implementing `SoulseekClient`: no network,
//! no daemon, no Soulseek account. Preserving that boundary is what makes these
//! tests runnable in CI.

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
use support::{committed, encode, noise_source, transcode_to_flac, Encoded, Scratch};

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

/// The index a collect logs its tag writes against, beside the review store.
fn index(settings: &Settings) -> LibraryIndex {
    LibraryIndex::open(&settings.index.path).expect("an index")
}

/// `fetch::collect` with the library it needs opened from `settings`, so a
/// test about transfers does not have to spell the tag step out.
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

    assert!(collect(&mut store, &client, &settings, true)
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
    let outcomes = collect(&mut store, &client, &settings, true).expect("a collect");

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
    let outcome = &collect(&mut store, &client, &settings, false).expect("a collect")[0];

    assert_eq!(
        outcome.local_path,
        Some(scratch.join("downloads").join("DECO_27").join("Roki.flac"))
    );
}

#[test]
fn no_local_path_is_guessed_without_a_configured_download_directory() {
    let scratch = Scratch::new("fetch-no-download-dir");
    let mut settings = settings(&scratch);
    settings.slskd.download_directory = None;
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let client = FakeClient::with_transfers(vec![transfer(
        "@@peerA\\Music\\Roki.flac",
        TransferState::Completed,
        "",
    )]);
    let outcome = &collect(&mut store, &client, &settings, false).expect("a collect")[0];

    assert_eq!(outcome.local_path, None);
}

#[test]
fn a_downloaded_lossless_file_is_verified() {
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
    let outcome = &collect(&mut store, &client, &settings, true).expect("a collect")[0];

    assert_eq!(
        outcome.verdict.as_ref().expect("a verdict").verdict,
        Verdict::Ok
    );
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
    let outcome = &collect(&mut store, &client, &settings, true).expect("a collect")[0];

    assert!(outcome.verdict.is_none());
}

#[test]
fn verification_can_be_skipped() {
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
    let outcome = &collect(&mut store, &client, &settings, false).expect("a collect")[0];

    assert!(outcome.verdict.is_none());
}

/// A lossy file queued under `filename`, then reported downloaded with
/// `arrived` copied to where slskd would have put it.
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
fn a_downloaded_mp3_is_held_to_its_bitrate() {
    let scratch = Scratch::new("fetch-mp3-genuine");
    let settings = settings(&scratch);
    let mut store = store(&scratch);

    let outcome = lossy_download(
        &scratch,
        &settings,
        &mut store,
        "Roki.mp3",
        &committed("noise-320k.mp3"),
    );

    assert_eq!(
        outcome.verdict.as_ref().expect("a verdict").verdict,
        Verdict::Ok
    );
    let entry = store
        .track(outcome.track_id)
        .expect("a lookup")
        .expect("the track");
    assert_eq!(entry.detail, "downloaded, verified");
}

/// #16: a peer shipping upsampled MP3s is caught on the first one.
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
fn a_lossy_download_verify_has_no_bitrate_table_for_is_left_unjudged() {
    let scratch = Scratch::new("fetch-ogg");
    let settings = settings(&scratch);
    let mut store = store(&scratch);

    let outcome = lossy_download(
        &scratch,
        &settings,
        &mut store,
        "Roki.ogg",
        &committed("tone.ogg"),
    );

    assert!(outcome.verdict.is_none());
}

#[test]
fn no_transfers_at_all_does_nothing() {
    let scratch = Scratch::new("fetch-no-transfers");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    assert!(collect(&mut store, &FakeClient::default(), &settings, true)
        .expect("a collect")
        .is_empty());
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

/// A client whose shortlist depends on which track is being searched for.
///
/// The shape the batch pass exists for needs a *different* peer winning each
/// track on its own merits; one shared shortlist cannot express that.
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

/// A candidate that differs from its neighbours only in availability, so every
/// gap between them sits inside the uploader window and none of them is a
/// difference of codec, master or content.
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

/// Each track won by its own peer, with `bulk` a close second on every one.
fn batch_client(extra_fillers: bool) -> PerTrackClient {
    PerTrackClient {
        lists: BATCH
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let mut list = vec![from_peer(&format!("solo{index}"), title, 0, true)];
                if extra_fillers {
                    // Named per track, so only `bulk` covers the batch -- and
                    // placed above it, so `bulk` falls past a cap of two.
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
fn the_batch_pass_sees_candidates_below_the_review_cap() {
    // The regression the restructure exists for: ranking used to truncate to
    // `candidates_per_track` before anything looked across the batch, so a peer
    // holding most of it was invisible on every track where they placed fourth.
    let scratch = Scratch::new("fetch-prefer-deep");
    let mut settings = settings(&scratch);
    settings.review.candidates_per_track = 2;
    let mut store = store(&scratch);
    queued_with(&mut store, &batch_tracks());

    fetch::search_pending(&mut store, &batch_client(true), &settings, None).expect("a search");

    for entry in store.tracks(&[], None).expect("the queued tracks") {
        assert_eq!(entry.candidates.len(), 2, "the review cap still holds");
        assert_eq!(entry.candidates[0].candidate.username, "bulk");
    }
}

#[test]
fn ranks_stay_dense_after_the_batch_pass() {
    // `ReviewTrack::approved` resolves positionally and `next_candidate` assumes
    // 1..n, so the reorder has to happen before the ranks are written.
    let scratch = Scratch::new("fetch-prefer-ranks");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued_with(&mut store, &batch_tracks());

    fetch::search_pending(&mut store, &batch_client(true), &settings, None).expect("a search");

    for entry in store.tracks(&[], None).expect("the queued tracks") {
        let ranks = entry.candidates.len();
        store.approve(entry.id, 1).expect("an approval");
        let approved = store
            .track(entry.id)
            .expect("a lookup")
            .expect("the track")
            .approved()
            .expect("the approved candidate")
            .clone();

        assert_eq!(approved.candidate.username, "bulk");
        // Rank 1 resolved to the first candidate, so the ranks are 1..n in list
        // order and nothing was renumbered behind the positional lookup.
        assert_eq!(ranks, entry.candidates.len());
    }
}

#[test]
fn turning_the_preference_off_leaves_the_scores_in_charge() {
    let scratch = Scratch::new("fetch-prefer-off");
    let mut settings = settings(&scratch);
    settings.uploader.prefer_one = false;
    let mut store = store(&scratch);
    queued_with(&mut store, &batch_tracks());

    let outcomes =
        fetch::search_pending(&mut store, &batch_client(false), &settings, None).expect("a search");

    assert!(outcomes
        .iter()
        .all(|outcome| outcome.preferred_uploader.is_empty()));
    for entry in store.tracks(&[], None).expect("the queued tracks") {
        assert!(entry.candidates[0].candidate.username.starts_with("solo"));
    }
}

/// A candidate already scored, so an album can be assembled without a search
/// deciding the ranks for it.
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

/// Two tracks one peer holds in a single folder, each with a fallback elsewhere.
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

/// The shape album-aware fetching turns on (#6): a folder approved as one
/// decision is still one `remote_path` per track, so queueing, collecting and
/// the retry stay per-track and none of them had to change to allow it.
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
    // One call per track, into the artist folder each was queued for, because
    // `downloaded_path` rebuilds that path to find the finished file -- #5 is
    // what happens when the two stop agreeing.
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
    // Still `<download dir>/<artist>/<filename>`: the album changed which files
    // were chosen, not where any of them lands.
    assert_eq!(
        outcomes[0].local_path,
        Some(scratch.join("downloads").join("DECO_27").join("Roki.flac"))
    );
    let stored = store.tracks(&[], None).expect("tracks");
    assert_eq!(stored[0].status, TrackStatus::Downloaded);
    // Only the file that failed falls back, and to its own next candidate
    // rather than to anything the album decided.
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

/// The file as a peer typically leaves it: junk in the fields that matter,
/// a comment beside them, and a cover (#11).
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

/// Roki searched, approved and queued, with the file already on disk as the
/// peer tagged it.
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
    assert_eq!(tags::artwork_size(&path), 512);
    assert!(tags::read_extras(&path)
        .expect("readable extras")
        .is_empty());
    assert_eq!(outcome.tags.as_ref().expect("a tag outcome").dropped, 1);
}

/// An `stk get` row has no album, and a title-only query no artist: a blank
/// in the row must not blank a field the peer filled.
#[test]
fn a_blank_in_the_row_never_blanks_the_file() {
    let scratch = Scratch::new("fetch-tag-blank");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki());

    collect(&mut store, &completed(), &settings, false).expect("a collect");

    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.album, "www.something.ru");
    assert_eq!(read.artist, "DECO*27");
}

#[test]
fn a_track_number_in_the_export_row_is_written_over_the_peers() {
    let scratch = Scratch::new("fetch-tag-track-number");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track = Track {
        source_row: vec![
            ("Track Name".to_owned(), "Roki".to_owned()),
            ("Track Number".to_owned(), "3".to_owned()),
        ],
        ..roki_with_album()
    };
    let (_, path) = arrived(&mut store, &settings, &track);

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    assert_eq!(
        tags::read_tags(&path).expect("readable tags").track_number,
        Some(3)
    );
    assert!(outcome
        .tags
        .as_ref()
        .expect("a tag outcome")
        .fields
        .contains(&TagField::TrackNumber));
}

#[test]
fn every_tag_written_at_collect_is_logged_under_one_run() {
    let scratch = Scratch::new("fetch-tag-logged");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());

    collect(&mut store, &completed(), &settings, false).expect("a collect");

    let changes = index(&settings)
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    let mut fields: Vec<&str> = changes.iter().map(|row| row.field.as_str()).collect();
    fields.sort_unstable();
    assert_eq!(fields, vec!["album", "artist", "extras", "title"]);
    assert!(changes.iter().all(|row| row.command == "fetch download"));
    assert!(changes.iter().all(|row| row.run_id == changes[0].run_id));
    assert!(changes.iter().all(|row| row.path == path));
    let artist = changes
        .iter()
        .find(|row| row.field == "artist")
        .expect("the artist row");
    assert_eq!(artist.old_value.as_deref(), Some("Unknown Artist"));
    assert_eq!(artist.new_value.as_deref(), Some("DECO*27"));
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
    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].value, "ripped by someone");
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
    assert_eq!(tags::read_extras(&path).expect("readable extras").len(), 1);
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
fn a_download_that_never_arrived_is_not_tagged() {
    let scratch = Scratch::new("fetch-tag-missing");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    assert_eq!(outcome.tags, None);
}

#[test]
fn a_file_that_cannot_be_tagged_is_reported_rather_than_failing_the_collect() {
    let scratch = Scratch::new("fetch-tag-unreadable");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    queued(&mut store, &settings);
    let path = scratch_download(&settings);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    std::fs::write(&path, b"not audio at all").expect("a written file");

    let outcome = &collect(&mut store, &completed(), &settings, false).expect("a collect")[0];

    let retagged = outcome.tags.as_ref().expect("a tag outcome");
    assert!(!retagged.error.is_empty());
    assert!(retagged.fields.is_empty());
    assert_eq!(outcome.state, TransferState::Completed);
}

/// A download directory outside every library root has no index row to bring
/// up to date, and must not gain one the next scan would only delete.
#[test]
fn a_download_outside_the_library_is_not_added_to_the_index() {
    let scratch = Scratch::new("fetch-tag-outside");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let (_, path) = arrived(&mut store, &settings, &roki_with_album());

    collect(&mut store, &completed(), &settings, false).expect("a collect");

    assert_eq!(index(&settings).entry(&path).expect("a lookup"), None);
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

/// A backend whose transfer list the test rewrites between polls.
#[derive(Default)]
struct LiveClient {
    transfers: Mutex<Vec<Transfer>>,
    enqueued: Mutex<Vec<String>>,
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

/// Time that passes only when the wait sleeps, calling back on every sleep so
/// the test can change what the backend reports.
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

/// Roki, finished, from this peer.
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

/// A track searched and approved, with these peers' files as its candidates.
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
        limit,
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
fn the_wait_ends_once_every_queued_track_has_arrived() {
    let scratch = Scratch::new("fetch-wait-done");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let track_id = approved(&mut store, &settings, &roki(), &["peerA"]);
    let client = LiveClient::default();
    let mut clock = TickingClock::new(|_| client.show(vec![arrived_from("peerA")]));

    let waited = wait(&mut store, &client, &settings, None, PATIENCE, &mut clock);

    assert_eq!(waited.rounds, 2);
    assert_eq!(waited.unfinished, 0);
    assert_eq!(waited.queued.len(), 1);
    assert_eq!(waited.collected.len(), 1);
    assert_eq!(status(&store, track_id), TrackStatus::Downloaded);
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
fn the_limit_caps_what_is_sent_and_the_rest_is_not_waited_for() {
    let scratch = Scratch::new("fetch-wait-limit");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let first = approved(&mut store, &settings, &roki(), &["peerA"]);
    let second = approved(
        &mut store,
        &settings,
        &Track::new("DECO*27", "Ghost Rule"),
        &["peerB"],
    );
    let client = LiveClient::default();
    let mut clock = TickingClock::new(|_| client.show(vec![arrived_from("peerA")]));

    let waited = wait(
        &mut store,
        &client,
        &settings,
        Some(1),
        PATIENCE,
        &mut clock,
    );

    assert_eq!(waited.queued.len(), 1);
    assert_eq!(waited.unfinished, 0);
    assert_eq!(status(&store, first), TrackStatus::Downloaded);
    assert_eq!(status(&store, second), TrackStatus::Approved);
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

#[test]
fn with_nothing_on_its_way_the_wait_does_not_ask_the_backend() {
    let scratch = Scratch::new("fetch-wait-nothing");
    let settings = settings(&scratch);
    let mut store = store(&scratch);
    let mut clock = TickingClock::new(|_| panic!("nothing to wait for"));

    let waited = wait(
        &mut store,
        &FakeClient::default(),
        &settings,
        None,
        PATIENCE,
        &mut clock,
    );

    assert_eq!(waited.rounds, 0);
    assert_eq!(waited, fetch::Waited::default());
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
fn a_clean_download_is_nothing_to_own_up_to() {
    assert!(!waited_with(vec![collected("", None, Some(Verdict::Ok))]).went_wrong());
}

#[test]
fn a_fake_download_is_something_to_own_up_to() {
    assert!(waited_with(vec![collected("", None, Some(Verdict::Fake))]).went_wrong());
}

#[test]
fn a_failure_the_retry_picked_up_is_not_a_failure_of_the_run() {
    assert!(!waited_with(vec![collected("peer gone", Some(2), None)]).went_wrong());
    assert!(waited_with(vec![collected("peer gone", None, None)]).went_wrong());
}

#[test]
fn a_track_that_could_not_be_queued_is_something_to_own_up_to() {
    let waited = fetch::Waited {
        queued: vec![fetch::EnqueueOutcome {
            track_id: 1,
            query: "DECO*27 - Roki".to_owned(),
            destination: "DECO_27".to_owned(),
            queued: false,
            error: "slskd unreachable".to_owned(),
        }],
        ..fetch::Waited::default()
    };

    assert!(waited.went_wrong());
}

#[test]
fn a_timeout_alone_is_nothing_to_own_up_to() {
    let waited = fetch::Waited {
        unfinished: 3,
        ..fetch::Waited::default()
    };

    assert!(!waited.went_wrong());
}
