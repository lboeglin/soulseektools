//! Ported from `py/tests/unit/test_review_store.py`, plus the retry and stall
//! tracking `review.db` v2 adds (#2) and the guessed titles v4 adds (#13).

mod support;

use std::collections::BTreeMap;

use stk::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use stk::review::store::ReviewStore;
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

/// A track with three candidates found and none chosen yet.
fn reviewable(store: &mut ReviewStore) -> i64 {
    store.add(&[roki()]).expect("an added track");
    let track_id = first_id(store);
    store
        .set_candidates(track_id, &[scored(1, &[]), scored(2, &[]), scored(3, &[])])
        .expect("recorded candidates");
    track_id
}

#[test]
fn tracks_are_queued_as_pending() {
    let scratch = Scratch::new("review-add");
    let mut store = store(&scratch);

    assert_eq!(store.add(&[roki()]).expect("an added track"), 1);

    let queued = store.tracks(&[], None).expect("tracks");
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].status, TrackStatus::Pending);
    assert_eq!(queued[0].track.album, "Ghost");
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
fn recording_candidates_moves_a_track_to_review() {
    let scratch = Scratch::new("review-candidates");
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let track_id = first_id(&store);

    store
        .set_candidates(track_id, &[scored(1, &[]), scored(2, &[])])
        .expect("recorded candidates");

    let reviewed = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(reviewed.status, TrackStatus::AwaitingReview);
    assert_eq!(reviewed.candidates.len(), 2);
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
fn no_candidates_is_a_distinct_state() {
    let scratch = Scratch::new("review-none");
    let mut store = store(&scratch);
    store.add(&[roki()]).expect("an added track");
    let track_id = first_id(&store);

    store.set_candidates(track_id, &[]).expect("no candidates");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::NoCandidates);
    assert!(stored.detail.contains("no usable"), "{}", stored.detail);
}

#[test]
fn a_second_search_replaces_the_first() {
    let scratch = Scratch::new("review-research");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);

    store
        .set_candidates(track_id, &[scored(1, &[])])
        .expect("recorded candidates");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.candidates.len(), 1);
}

#[test]
fn approving_records_the_choice() {
    let scratch = Scratch::new("review-approve");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);

    store.approve(track_id, 2).expect("an approval");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::Approved);
    assert_eq!(stored.approved_rank, Some(2));
    assert_eq!(
        stored
            .approved()
            .expect("the approved candidate")
            .candidate
            .username,
        "peer2"
    );
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

#[test]
fn approving_a_candidate_that_does_not_exist_is_an_error() {
    let scratch = Scratch::new("review-bad-rank");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);

    assert!(store.approve(track_id, 9).is_err());

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::AwaitingReview);
}

/// A second track with candidates of its own, for the batch approvals (#6).
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
fn several_tracks_can_be_approved_as_one_decision() {
    let scratch = Scratch::new("review-approve-all");
    let mut store = store(&scratch);
    let first = reviewable(&mut store);
    let second = second_reviewable(&mut store);

    store
        .approve_all(&[(first, 2), (second, 1)])
        .expect("an approval");

    for (track_id, rank) in [(first, 2), (second, 1)] {
        let stored = store.track(track_id).expect("a lookup").expect("the track");
        assert_eq!(stored.status, TrackStatus::Approved);
        assert_eq!(stored.approved_rank, Some(rank));
    }
}

/// An album is taken or left as a unit, so one rank that does not exist has to
/// leave every other track in the offer undecided (#6).
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
fn a_track_can_be_returned_to_the_queue() {
    let scratch = Scratch::new("review-reset");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store.approve(track_id, 1).expect("an approval");

    store.reset(track_id).expect("a reset");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::Pending);
    assert!(stored.candidates.is_empty());
}

#[test]
fn tracks_can_be_filtered_by_status() {
    let scratch = Scratch::new("review-filter");
    let mut store = store(&scratch);
    store
        .add(&[track("A", "One"), track("B", "Two")])
        .expect("added tracks");
    let track_id = first_id(&store);
    store
        .mark(track_id, TrackStatus::Queued, "")
        .expect("a mark");

    assert_eq!(
        store
            .tracks(&[TrackStatus::Pending], None)
            .expect("tracks")
            .len(),
        1
    );
    assert_eq!(
        store
            .tracks(&[TrackStatus::Queued], None)
            .expect("tracks")
            .len(),
        1
    );
    assert_eq!(
        store
            .tracks(&[TrackStatus::Pending, TrackStatus::Queued], None)
            .expect("tracks")
            .len(),
        2
    );
}

#[test]
fn the_limit_is_honoured() {
    let scratch = Scratch::new("review-limit");
    let mut store = store(&scratch);
    let tracks: Vec<Track> = (0..5).map(|index| track("A", &index.to_string())).collect();
    store.add(&tracks).expect("added tracks");

    assert_eq!(store.tracks(&[], Some(2)).expect("tracks").len(), 2);
}

#[test]
fn counts_summarise_the_queue() {
    let scratch = Scratch::new("review-counts");
    let mut store = store(&scratch);
    store
        .add(&[track("A", "One"), track("B", "Two")])
        .expect("added tracks");
    let track_id = first_id(&store);
    store
        .mark(track_id, TrackStatus::Downloaded, "")
        .expect("a mark");

    assert_eq!(
        store.counts().expect("counts"),
        BTreeMap::from([(TrackStatus::Pending, 1), (TrackStatus::Downloaded, 1)])
    );
}

#[test]
fn an_unknown_track_id_returns_nothing() {
    let scratch = Scratch::new("review-unknown");
    let store = store(&scratch);
    assert!(store.track(999).expect("a lookup").is_none());
}

#[test]
fn the_queue_survives_being_reopened() {
    let scratch = Scratch::new("review-reopen");
    {
        let mut first = store(&scratch);
        let track_id = reviewable(&mut first);
        first.approve(track_id, 1).expect("an approval");
    }

    let second = store(&scratch);
    let stored = &second.tracks(&[], None).expect("tracks")[0];
    assert_eq!(stored.status, TrackStatus::Approved);
    assert!(stored.approved().is_some());
}

/// `review.db` v2 (#2). `FAILED` used to be terminal even though the next
/// candidate was already in the database.
#[test]
fn the_next_candidate_is_promoted_without_losing_the_search() {
    let scratch = Scratch::new("review-next");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store.approve(track_id, 1).expect("an approval");

    assert_eq!(
        store.next_candidate(track_id).expect("a promotion"),
        Some(2)
    );

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.approved_rank, Some(2));
    assert_eq!(stored.attempts, 1);
    // The point of this over `reset`: the search results are still here.
    assert_eq!(stored.candidates.len(), 3);
}

#[test]
fn a_track_only_fails_once_its_candidates_are_exhausted() {
    let scratch = Scratch::new("review-exhausted");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    // Started on the last candidate, the way an album approval does when the
    // folder is not the track's own best. The two the scorer preferred are
    // untried, so exhaustion means all of them rather than all of them above.
    store.approve(track_id, 3).expect("an approval");

    assert_eq!(
        store.next_candidate(track_id).expect("a promotion"),
        Some(1)
    );
    assert_eq!(
        store.next_candidate(track_id).expect("a promotion"),
        Some(2)
    );
    assert_eq!(store.next_candidate(track_id).expect("no more"), None);

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::Failed);
    assert!(
        stored.detail.contains("no candidates left"),
        "{}",
        stored.detail
    );
}

#[test]
fn a_queued_track_records_when_it_was_queued() {
    let scratch = Scratch::new("review-queued-at");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);

    store
        .mark_queued(track_id, 1000.0, "queued")
        .expect("a queued mark");

    let stored = store.track(track_id).expect("a lookup").expect("the track");
    assert_eq!(stored.status, TrackStatus::Queued);
    assert_eq!(stored.queued_at, Some(1000.0));
}

#[test]
fn a_transfer_sitting_too_long_is_reported_as_stalled() {
    // Nothing recorded when a track entered QUEUED before, so a transfer sitting
    // for six hours looked exactly like one queued a minute ago.
    let scratch = Scratch::new("review-stalled");
    let mut store = store(&scratch);
    let track_id = reviewable(&mut store);
    store
        .mark_queued(track_id, 1000.0, "queued")
        .expect("a queued mark");

    assert!(store.stalled(1500.0, 3600.0).expect("stalled").is_empty());
    assert_eq!(store.stalled(9000.0, 3600.0).expect("stalled").len(), 1);
}

/// v3 adds `attempted` to a table an existing database already holds rows in,
/// so the column has to arrive without disturbing them (#6).
#[test]
fn a_v2_database_keeps_its_candidates_when_the_retry_column_arrives() {
    let scratch = Scratch::new("review-v2");
    let database = scratch.join("v2.db");

    {
        let mut store = ReviewStore::open(&database).expect("a store");
        reviewable(&mut store);
    }
    let connection = rusqlite::Connection::open(&database).expect("a database");
    connection
        .execute_batch("ALTER TABLE candidates DROP COLUMN attempted")
        .expect("a v2 candidates table");
    drop(connection);

    let mut store = ReviewStore::open(&database).expect("an upgraded store");
    let track_id = first_id(&store);

    assert_eq!(
        store
            .track(track_id)
            .expect("a lookup")
            .expect("the track")
            .candidates
            .len(),
        3
    );
    // Nothing was tried before the column existed, so every candidate is still
    // reachable rather than all of them or none.
    assert_eq!(store.next_candidate(track_id).expect("a retry"), Some(1));
}

/// A queue the Python build wrote is upgraded in place, so a review session
/// begun before the port carries on after it.
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

/// A row `stk get --album` writes says so, because its title came off a
/// filename rather than out of anything anyone asked for by name (#13).
#[test]
fn a_row_records_whether_its_title_was_guessed() {
    let scratch = Scratch::new("review-guessed");
    let mut store = store(&scratch);

    store
        .add(&[
            roki(),
            Track {
                title: "Aimai Elegy".to_owned(),
                title_guessed: true,
                ..roki()
            },
        ])
        .expect("two added tracks");

    let stored = store.tracks(&[], None).expect("tracks");
    assert!(!stored[0].track.title_guessed);
    assert!(stored[1].track.title_guessed);
}

/// v4 adds `title_guessed` to a table an existing database already holds rows
/// in, so the column has to arrive without disturbing them (#13).
#[test]
fn a_v3_database_keeps_its_tracks_when_the_guessed_column_arrives() {
    let scratch = Scratch::new("review-v3");
    let database = scratch.join("v3.db");

    {
        let mut store = ReviewStore::open(&database).expect("a store");
        reviewable(&mut store);
    }
    let connection = rusqlite::Connection::open(&database).expect("a database");
    connection
        .execute_batch("ALTER TABLE tracks DROP COLUMN title_guessed")
        .expect("a v3 tracks table");
    drop(connection);

    let store = ReviewStore::open(&database).expect("an upgraded store");
    let stored = &store.tracks(&[], None).expect("tracks")[0];

    assert_eq!(stored.track.query(), "DECO*27 - Roki");
    assert_eq!(stored.candidates.len(), 3);
    // Nothing written before the column existed was a guess, which is what the
    // default says, rather than leaving it unknown.
    assert!(!stored.track.title_guessed);
}
