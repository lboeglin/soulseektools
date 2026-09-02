//! The interactive picker (#8): what each answer decides, and what it leaves
//! behind in the review queue.
//!
//! The walk takes its input and its transcript as arguments, so a whole session
//! is a string of answers and the result is read back out of the store -- no
//! terminal, and no subprocess.

mod support;

use std::io::Cursor;

use stk::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use stk::review::picker::{self, Decision, Tally};
use stk::review::store::ReviewStore;
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

/// Two tracks awaiting review, three candidates each.
///
/// The first candidate carries `preferred-uploader`, the flag the batch pass
/// leaves when it moves a result to the front because one peer covers more of
/// the batch -- which is exactly what someone deciding wants to see.
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

/// Answer the prompts with `script`, and hand back the tally and the transcript.
fn walk(store: &mut ReviewStore, script: &str) -> (Tally, String) {
    let mut input = Cursor::new(script.as_bytes().to_vec());
    let mut transcript: Vec<u8> = Vec::new();
    let tally = picker::walk(store, &mut input, &mut transcript).expect("a completed walk");
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
fn a_number_takes_that_candidate() {
    let scratch = Scratch::new("picker-number");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "2\n3\n");

    assert_eq!(tally.approved, 2);
    assert_eq!(approved_rank(&store, "Roki"), Some(2));
    assert_eq!(approved_rank(&store, "Ghost Rule"), Some(3));
}

#[test]
fn a_rank_not_offered_is_asked_again_rather_than_recorded() {
    let scratch = Scratch::new("picker-out-of-range");
    let mut store = queue(&scratch);

    let (tally, transcript) = walk(&mut store, "9\n2\nq\n");

    assert!(
        transcript.contains("Not one of the offered candidates."),
        "{transcript}"
    );
    assert_eq!(tally.approved, 1);
    assert_eq!(approved_rank(&store, "Roki"), Some(2));
    assert_eq!(status_of(&store, "Ghost Rule"), TrackStatus::AwaitingReview);
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
fn s_leaves_the_track_awaiting_review() {
    let scratch = Scratch::new("picker-skip");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "s\nq\n");

    assert_eq!(tally.skipped, 1);
    assert_eq!(tally.approved, 0);
    assert_eq!(status_of(&store, "Roki"), TrackStatus::AwaitingReview);
}

#[test]
fn u_takes_back_the_previous_decision_and_offers_it_again() {
    let scratch = Scratch::new("picker-undo");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "1\nu\n2\nq\n");

    assert_eq!(tally.approved, 1);
    assert_eq!(approved_rank(&store, "Roki"), Some(2));
    // The candidate taken back must not stay marked as tried, or #2's retry
    // would fall past it to the third and the best copy would never be fetched.
    let id = id_of(&store, "Roki");
    assert_eq!(store.next_candidate(id).expect("a retry"), Some(1));
}

#[test]
fn undoing_a_rejection_returns_the_track_to_the_queue() {
    let scratch = Scratch::new("picker-undo-reject");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "r\nu\ns\nq\n");

    assert_eq!(tally.rejected, 0);
    assert_eq!(tally.skipped, 1);
    assert_eq!(status_of(&store, "Roki"), TrackStatus::AwaitingReview);
}

#[test]
fn undo_before_anything_is_decided_says_so() {
    let scratch = Scratch::new("picker-undo-empty");
    let mut store = queue(&scratch);

    let (tally, transcript) = walk(&mut store, "u\nq\n");

    assert!(transcript.contains("Nothing to undo yet."), "{transcript}");
    assert_eq!(
        tally,
        Tally {
            waiting: 2,
            ..Tally::default()
        }
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
    // An empty answer takes the first candidate, and a pipe at its end reads as
    // an empty line unless the difference is checked for. It was not in the
    // prompt this loop grew out of, so `stk get "..." < /dev/null` downloaded a
    // candidate nobody had chosen; here it would have approved the whole queue.
    let scratch = Scratch::new("picker-eof");
    let mut store = queue(&scratch);

    let (tally, _) = walk(&mut store, "");

    assert_eq!(tally.approved, 0);
    for title in TITLES {
        assert_eq!(status_of(&store, title), TrackStatus::AwaitingReview);
    }
}

#[test]
fn the_listing_shows_each_candidates_path_and_flags() {
    let scratch = Scratch::new("picker-listing");
    let mut store = queue(&scratch);

    let (_, transcript) = walk(&mut store, "q\n");

    assert!(
        transcript.contains("@@peer1\\Music\\Ghost Rule EP\\Roki.flac"),
        "{transcript}"
    );
    assert!(transcript.contains("preferred-uploader"), "{transcript}");
    assert!(transcript.contains("1/2  DECO*27 - Roki"), "{transcript}");
}

#[test]
fn only_tracks_awaiting_review_are_offered() {
    let scratch = Scratch::new("picker-awaiting-only");
    let mut store = queue(&scratch);
    let rejected = id_of(&store, "Roki");
    store.reject(rejected, "live").expect("a rejection");

    let (tally, transcript) = walk(&mut store, "1\n");

    assert_eq!(tally.waiting, 1);
    assert!(!transcript.contains("Roki.flac"), "{transcript}");
    assert_eq!(approved_rank(&store, "Ghost Rule"), Some(1));
}

#[test]
fn every_answer_the_prompt_offers_is_understood() {
    assert_eq!(picker::parse("", 3), Some(Decision::Approve(1)));
    assert_eq!(picker::parse(" 2 \n", 3), Some(Decision::Approve(2)));
    assert_eq!(picker::parse("R\n", 3), Some(Decision::Reject));
    assert_eq!(picker::parse("skip", 3), Some(Decision::Skip));
    assert_eq!(picker::parse("u", 3), Some(Decision::Undo));
    assert_eq!(picker::parse("q", 3), Some(Decision::Quit));
    assert_eq!(picker::parse("none", 3), Some(Decision::Quit));
    assert_eq!(picker::parse("4", 3), None);
    assert_eq!(picker::parse("0", 3), None);
    assert_eq!(picker::parse("maybe", 3), None);
}
