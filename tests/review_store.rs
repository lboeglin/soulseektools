//! The review queue: states, choices, retries and migrations.

mod support;

use stk::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use stk::review::store::{Retry, ReviewStore};
use stk::soulseek::ranking::PREFERRED_FLAG;
use support::Scratch;

fn store(scratch: &Scratch) -> ReviewStore {
    ReviewStore::open(&scratch.join("review.db")).expect("a store")
}

fn track(artist: &str, title: &str) -> Track {
    Track {
        artist: artist.to_owned(),
        title: title.to_owned(),
        album: "Ghost".to_owned(),
        duration_seconds: Some(215.0),
        title_guessed: false,
        typed: false,
        source_row: Vec::new(),
    }
}

fn roki() -> Track {
    track("DECO*27", "Roki")
}

fn scored(rank: u32, flags: &[&str]) -> ScoredCandidate {
    ScoredCandidate {
        candidate: Candidate {
            username: format!("peer{rank}"),
            remote_path: format!("@@x\\Music\\{rank}\\Roki.flac"),
            size: 30_000_000 - u64::from(rank),
            bitrate: Some(1000),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            is_vbr: false,
            free_upload_slots: rank == 1,
            queue_length: rank,
            duration_seconds: Some(215.0),
        },
        score: i64::from(1000 - rank),
        title_ratio: 0.99,
        flags: flags.iter().map(|flag| (*flag).to_owned()).collect(),
    }
}

fn first_id(store: &ReviewStore) -> i64 {
    store.tracks(&[], None).expect("tracks")[0].id
}

fn reviewable(store: &mut ReviewStore) -> i64 {
    store.add(&[roki()]).expect("an added track");
    let track_id = first_id(store);
    store
        .set_candidates(track_id, &[scored(1, &[]), scored(2, &[]), scored(3, &[])])
        .expect("recorded candidates");
    track_id
}

#[test]
fn the_same_track_is_not_queued_twice() {
    let scratch = Scratch::new("review-duplicate");
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");

    assert_eq!(store.add(&[roki()]).expect("no second add"), 0);
    assert_eq!(store.tracks(&[], None).expect("tracks").len(), 1);
}

#[test]
fn a_typed_name_is_remembered_as_typed() {
    let scratch = Scratch::new("review-typed");
    let mut store = store(&scratch);
    let typed = Track {
        typed: true,
        ..track("DECO*27", "Vampire")
    };
    store.add(&[typed, roki()]).expect("added tracks");

    let tracks = store.tracks(&[], None).expect("tracks");
    let typed_of = |title: &str| {
        tracks
            .iter()
            .find(|entry| entry.track.title == title)
            .expect("a queued track")
            .track
            .typed
    };
    assert!(typed_of("Vampire"));
    assert!(!typed_of("Roki"));
}

#[test]
fn the_original_export_row_survives_the_round_trip() {
    let scratch = Scratch::new("review-source-row");
    let mut store = store(&scratch);
    store
        .add(&[Track {
            artist: "A".to_owned(),
            title: "B".to_owned(),
            source_row: vec![("Added at".to_owned(), "2026-01-01".to_owned())],
            ..Track::default()
        }])
        .expect("an added track");

    assert_eq!(
        store.tracks(&[], None).expect("tracks")[0].track.source_row,
        vec![("Added at".to_owned(), "2026-01-01".to_owned())]
    );
}

#[test]
fn candidates_round_trip_with_their_flags() {
    let scratch = Scratch::new("review-flags");
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let track_id = first_id(&store);

    store
        .set_candidates(track_id, &[scored(1, &["live", "remix"])])
        .expect("recorded candidates");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(
        stored.candidates[0].flags,
        vec!["live".to_owned(), "remix".to_owned()]
    );
    assert!(stored.candidates[0].candidate.free_upload_slots);
}

#[test]
fn changing_the_choice_clears_the_previous_one() {
    let scratch = Scratch::new("review-reapprove");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store.approve(track_id, 1).expect("an approval");

    store.approve(track_id, 3).expect("a second approval");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.approved_rank, Some(3));
}

fn second_reviewable(store: &mut ReviewStore) -> i64 {
    store
        .add(&[track("ZUN", "Heian Alien")])
        .expect("a second track");
    let track_id = store.tracks(&[], None).expect("tracks")[1].id;
    store
        .set_candidates(track_id, &[scored(1, &[]), scored(2, &[])])
        .expect("recorded candidates");
    track_id
}

#[test]
fn approving_several_tracks_at_once_is_all_or_nothing() {
    let scratch = Scratch::new("review-approve-all-fails");
    let mut store = store(&scratch);
    let first = reviewable(&mut store);
    let second = second_reviewable(&mut store);

    assert!(store.approve_all(&[(first, 1), (second, 9)]).is_err());

    for track_id in [first, second] {
        let stored = store.track(track_id).expect("a lookup").expect("the track");
        assert_eq!(stored.status, TrackStatus::AwaitingReview);
        assert!(stored.approved().is_none());
    }
}

#[test]
fn rejecting_records_the_reason_and_clears_any_choice() {
    let scratch = Scratch::new("review-reject");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store.approve(track_id, 1).expect("an approval");

    store
        .reject(track_id, "all candidates are live recordings")
        .expect("a rejection");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::Rejected);
    assert!(stored.approved().is_none());
    assert!(
        stored.detail.contains("live recordings"),
        "{}",
        stored.detail
    );
}

#[test]
fn the_next_candidate_is_promoted_without_losing_the_search() {
    let scratch = Scratch::new("review-next");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store.approve(track_id, 1).expect("an approval");

    assert_eq!(
        store.next_candidate(track_id).expect("a promotion"),
        Retry::Approved(2)
    );

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.approved_rank, Some(2));
    assert_eq!(stored.attempts, 1);
    // Unlike `reset`, the search results are kept.
    assert_eq!(stored.candidates.len(), 3);
}

#[test]
fn a_track_only_fails_once_its_candidates_are_exhausted() {
    let scratch = Scratch::new("review-exhausted");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    // Started on the last candidate, as an album approval may; the two above are untried.
    store.approve(track_id, 3).expect("an approval");

    assert_eq!(
        store.next_candidate(track_id).expect("a promotion"),
        Retry::Approved(1)
    );
    assert_eq!(
        store.next_candidate(track_id).expect("a promotion"),
        Retry::Approved(2)
    );
    assert_eq!(
        store.next_candidate(track_id).expect("no more"),
        Retry::Exhausted
    );

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::Failed);
    assert!(
        stored.detail.contains("no candidates left"),
        "{}",
        stored.detail
    );
}

#[test]
fn a_transfer_sitting_too_long_is_reported_as_stalled() {
    let scratch = Scratch::new("review-stalled");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store
        .mark_queued(track_id, 1000.0, "queued")
        .expect("a queued mark");

    assert_eq!(store.stalled(1500.0, 3600.0).expect("stalled"), []);
    assert_eq!(store.stalled(9000.0, 3600.0).expect("stalled").len(), 1);
}

#[test]
fn reordering_the_untried_candidates_keeps_the_tried_ones_first_and_tried() {
    let scratch = Scratch::new("review-reorder");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store.approve(track_id, 2).expect("an approval");

    store
        .reorder_untried(track_id, &[scored(3, &[PREFERRED_FLAG]), scored(1, &[])])
        .expect("a reorder");

    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.approved_rank, None);
    assert_eq!(
        entry.candidates,
        vec![scored(2, &[]), scored(3, &[PREFERRED_FLAG]), scored(1, &[])]
    );
    assert_eq!(store.untried(track_id).expect("untried").len(), 2);
    assert_eq!(
        store.next_candidate(track_id).expect("a retry"),
        Retry::Approved(2)
    );
    assert_eq!(
        store
            .track(track_id)
            .expect("a lookup")
            .expect("the track")
            .approved()
            .expect("an approval")
            .candidate
            .username,
        "peer3"
    );
}

#[test]
fn a_v1_database_is_upgraded_in_place_rather_than_rewritten() {
    let scratch = Scratch::new("review-v1");
    let database = scratch.join("v1.db");

    let connection = rusqlite::Connection::open(&database).expect("a database");
    connection
        .execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE tracks (
                id INTEGER PRIMARY KEY, query TEXT NOT NULL UNIQUE, artist TEXT NOT NULL,
                title TEXT NOT NULL, album TEXT NOT NULL DEFAULT '', duration_seconds REAL,
                status TEXT NOT NULL DEFAULT 'pending', detail TEXT NOT NULL DEFAULT '',
                source_row TEXT NOT NULL DEFAULT '{}', added_at TEXT NOT NULL
             );
             INSERT INTO meta VALUES ('schema_version', '1');
             INSERT INTO tracks (query, artist, title, added_at)
                VALUES ('DECO*27 - Roki', 'DECO*27', 'Roki', '2026-01-01');",
        )
        .expect("a v1 database");
    drop(connection);

    let store = ReviewStore::open(&database).expect("an upgraded store");

    let stored = &store.tracks(&[], None).expect("tracks")[0];
    assert_eq!(stored.track.artist, "DECO*27");
    assert_eq!(stored.queued_at, None);
    assert_eq!(stored.attempts, 0);
}

#[test]
fn a_retry_passes_over_a_flagged_candidate() {
    let scratch = Scratch::new("review-retry-clean");
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let track_id = first_id(&store);
    store
        .set_candidates(
            track_id,
            &[
                scored(1, &[]),
                scored(2, &["missing:reloaded"]),
                scored(3, &[]),
            ],
        )
        .expect("recorded candidates");
    store.approve(track_id, 1).expect("an approval");

    assert_eq!(
        store.next_candidate(track_id).expect("a retry"),
        Retry::Approved(3)
    );
}

#[test]
fn a_retry_with_only_flagged_candidates_left_goes_back_to_review() {
    let scratch = Scratch::new("review-retry-flagged");
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let track_id = first_id(&store);
    store
        .set_candidates(
            track_id,
            &[scored(1, &[]), scored(2, &["missing:reloaded"])],
        )
        .expect("recorded candidates");
    store.approve(track_id, 1).expect("an approval");

    assert_eq!(
        store.next_candidate(track_id).expect("a retry"),
        Retry::ForReview
    );
    let entry = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(entry.status, TrackStatus::AwaitingReview);
    assert_eq!(entry.approved_rank, None);
}
