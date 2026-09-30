//! The interactive picker, driven by a string of answers and read back from the store.

mod support;

use std::io::Cursor;

use stk::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use stk::review::picker::{self, Decision, Tally};
use stk::review::store::{Retry, ReviewStore};
use support::Scratch;

const TITLES: [&str; 2] = ["Roki", "Ghost Rule"];

fn scored(title: &str, rank: u32, flags: &[&str]) -> ScoredCandidate {
    ScoredCandidate {
        candidate: Candidate {
            username: format!("peer{rank}"),
            remote_path: format!("@@peer{rank}\\Music\\Ghost Rule EP\\{title}.flac"),
            size: 30_000_000,
            bitrate: None,
            sample_rate: Some(44100),
            bit_depth: Some(16),
            is_vbr: false,
            free_upload_slots: rank == 1,
            queue_length: rank,
            duration_seconds: Some(215.0),
        },
        score: 100 - i64::from(rank),
        title_ratio: 0.99,
        flags: flags.iter().map(|flag| (*flag).to_owned()).collect(),
    }
}

/// The first candidate carries `preferred-uploader`.
fn queue(scratch: &Scratch) -> ReviewStore {
    let mut store = ReviewStore::open(&scratch.join("review.db")).expect("a store");
    for title in TITLES {
        store
            .add(&[Track::new("DECO*27", title)])
            .expect("an added track");
        let id = id_of(&store, title);
        store
            .set_candidates(
                id,
                &[
                    scored(title, 1, &["preferred-uploader"]),
                    scored(title, 2, &[]),
                    scored(title, 3, &[]),
                ],
            )
            .expect("recorded candidates");
    }
    store
}

fn id_of(store: &ReviewStore, title: &str) -> i64 {
    store
        .tracks(&[], None)
        .expect("tracks")
        .into_iter()
        .find(|entry| entry.track.title == title)
        .expect("a queued track")
        .id
}

fn status_of(store: &ReviewStore, title: &str) -> TrackStatus {
    store
        .track(id_of(store, title))
        .expect("the track")
        .expect("a queued track")
        .status
}

fn approved_rank(store: &ReviewStore, title: &str) -> Option<i64> {
    store
        .track(id_of(store, title))
        .expect("the track")
        .expect("a queued track")
        .approved_rank
}

fn walk(store: &mut ReviewStore, script: &str) -> (Tally, String) {
    let mut input = Cursor::new(script.as_bytes().to_vec());
    let mut transcript: Vec<u8> = Vec::new();
    let tally = picker::walk(store, 10, &mut input, &mut transcript).expect("a completed walk");
    (
        tally,
        String::from_utf8(transcript).expect("a utf-8 transcript"),
    )
}

#[test]
fn an_empty_answer_takes_the_first_candidate() {
    let scratch = Scratch::new("picker-default");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "\n\n");

    assert_eq!(tally.approved, 2);
    assert_eq!(tally.waiting, 2);
    for title in TITLES {
        assert_eq!(status_of(&store, title), TrackStatus::Approved);
        assert_eq!(approved_rank(&store, title), Some(1));
    }
}

#[test]
fn r_rejects_the_track_and_moves_on() {
    let scratch = Scratch::new("picker-reject");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "r\n1\n");

    assert_eq!((tally.rejected, tally.approved), (1, 1));
    assert_eq!(status_of(&store, "Roki"), TrackStatus::Rejected);
    assert_eq!(approved_rank(&store, "Roki"), None);
    assert_eq!(status_of(&store, "Ghost Rule"), TrackStatus::Approved);
}

#[test]
fn u_takes_back_the_previous_decision_and_offers_it_again() {
    let scratch = Scratch::new("picker-undo");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "1\nu\n2\nq\n");

    assert_eq!(tally.approved, 1);
    assert_eq!(approved_rank(&store, "Roki"), Some(2));
    // Still marked tried, the retry would skip past it and the best copy would never be fetched.
    let id = id_of(&store, "Roki");
    assert_eq!(
        store.next_candidate(id).expect("a retry"),
        Retry::Approved(1)
    );
}

#[test]
fn q_stops_the_walk_with_the_rest_undecided() {
    let scratch = Scratch::new("picker-quit");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "1\nq\n");

    assert_eq!(tally.approved, 1);
    assert_eq!(status_of(&store, "Ghost Rule"), TrackStatus::AwaitingReview);
}

#[test]
fn exhausted_input_ends_the_walk_rather_than_approving() {
    // Regression: end of input read as an empty answer, which takes the first candidate.
    let scratch = Scratch::new("picker-eof");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "");

    assert_eq!(tally.approved, 0);
    for title in TITLES {
        assert_eq!(status_of(&store, title), TrackStatus::AwaitingReview);
    }
}

#[test]
fn every_answer_the_prompt_offers_is_understood() {
    assert_eq!(picker::parse("", 3), Some(Decision::Approve(1)));
    assert_eq!(picker::parse(" 2 \n", 3), Some(Decision::Approve(2)));
    assert_eq!(picker::parse("R\n", 3), Some(Decision::Reject));
    assert_eq!(picker::parse("skip", 3), Some(Decision::Skip));
    assert_eq!(picker::parse("u", 3), Some(Decision::Undo));
    assert_eq!(picker::parse("q", 3), Some(Decision::Quit));
    assert_eq!(picker::parse("4", 3), None);
    assert_eq!(picker::parse("0", 3), None);
    assert_eq!(picker::parse("maybe", 3), None);
}

#[test]
fn declining_a_track_is_not_abandoning_the_queue() {
    // In a long walk, `n` meaning quit would drop the rest of the queue.
    for answer in ["n", "no", "none"] {
        assert_eq!(picker::parse(answer, 3), Some(Decision::Skip), "{answer}");
    }

    let scratch = Scratch::new("picker-none");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "n\n2\n");

    assert_eq!(tally.skipped, 1);
    assert_eq!(status_of(&store, "Roki"), TrackStatus::AwaitingReview);
    assert_eq!(approved_rank(&store, "Ghost Rule"), Some(2));
}

fn flag_first(store: &mut ReviewStore, title: &str, flag: &str) {
    let id = id_of(store, title);
    store
        .set_candidates(id, &[scored(title, 1, &[flag]), scored(title, 2, &[])])
        .expect("recorded candidates");
}

fn waiting(store: &ReviewStore) -> Vec<stk::review::store::ReviewTrack> {
    store
        .tracks(&[TrackStatus::AwaitingReview], None)
        .expect("tracks")
}

#[test]
fn yes_takes_a_clean_best_candidate_even_from_the_preferred_peer() {
    let scratch = Scratch::new("picker-take-clean");
    let mut store = queue(&scratch);
    let entries = waiting(&store);

    let taken = picker::take_best(&mut store, &entries, false).expect("taken");

    assert_eq!(taken.approved.len(), 2);
    assert!(taken.held.is_empty());
    assert_eq!(approved_rank(&store, "Roki"), Some(1));
}

#[test]
fn yes_leaves_a_flagged_best_candidate_for_review() {
    let scratch = Scratch::new("picker-take-flagged");
    let mut store = queue(&scratch);
    flag_first(&mut store, "Roki", "extra:eurobeat mix");
    let entries = waiting(&store);

    let taken = picker::take_best(&mut store, &entries, false).expect("taken");

    assert_eq!(taken.approved, vec![id_of(&store, "Ghost Rule")]);
    assert_eq!(taken.held.len(), 1);
    assert_eq!(status_of(&store, "Roki"), TrackStatus::AwaitingReview);
    assert_eq!(approved_rank(&store, "Roki"), None);
}

#[test]
fn force_takes_a_flagged_best_candidate() {
    let scratch = Scratch::new("picker-take-forced");
    let mut store = queue(&scratch);
    flag_first(&mut store, "Roki", "remix");
    let entries = waiting(&store);

    let taken = picker::take_best(&mut store, &entries, true).expect("taken");

    assert_eq!(taken.approved.len(), 2);
    assert_eq!(approved_rank(&store, "Roki"), Some(1));
}

#[test]
fn the_picker_offers_only_the_candidates_asked_for() {
    let scratch = Scratch::new("picker-shown");
    let mut store = queue(&scratch);
    let mut input = Cursor::new(b"3\n2\n".to_vec());
    let mut transcript: Vec<u8> = Vec::new();

    let tally = picker::walk(&mut store, 2, &mut input, &mut transcript).expect("a walk");
    let transcript = String::from_utf8(transcript).expect("utf-8");

    assert!(transcript.contains(" 2. "), "{transcript}");
    assert!(!transcript.contains(" 3. "), "{transcript}");
    assert!(transcript.contains("Not one of the offered candidates."));
    assert_eq!(tally.approved, 1);
    assert_eq!(approved_rank(&store, "Roki"), Some(2));
}
