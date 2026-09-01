//! Grouping the review queue by the folder a peer offers it from (#6).
//!
//! The offers are built from a real `ReviewStore` rather than from hand-made
//! `ReviewTrack` values, because the rank numbering the grouping depends on is
//! `set_candidates`' business: a fixture that numbered its own ranks could agree
//! with itself while disagreeing with the database.

mod support;

use stk::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use stk::review::albums::{offers, AlbumOffer};
use stk::review::store::ReviewStore;
use support::Scratch;

fn store(scratch: &Scratch) -> ReviewStore {
    ReviewStore::open(&scratch.join("review.db")).expect("a store")
}

fn scored(username: &str, remote_path: &str, score: i64) -> ScoredCandidate {
    ScoredCandidate {
        candidate: Candidate {
            username: username.to_owned(),
            remote_path: remote_path.to_owned(),
            size: 30_000_000,
            sample_rate: Some(44100),
            bit_depth: Some(16),
            free_upload_slots: true,
            ..Candidate::default()
        },
        score,
        title_ratio: 0.99,
        flags: Vec::new(),
    }
}

/// A track with candidates found and nothing chosen yet.
fn reviewable(store: &mut ReviewStore, title: &str, candidates: &[ScoredCandidate]) -> i64 {
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
        .set_candidates(track_id, candidates)
        .expect("recorded candidates");
    track_id
}

/// The offers the CLI would show: over tracks awaiting review, and no others.
fn waiting(store: &ReviewStore, minimum: usize) -> Vec<AlbumOffer> {
    let tracks = store
        .tracks(&[TrackStatus::AwaitingReview], None)
        .expect("the waiting tracks");
    offers(&tracks, minimum)
}

#[test]
fn a_folder_one_peer_holds_several_tracks_in_is_one_offer() {
    let scratch = Scratch::new("albums-one-offer");
    let mut store = store(&scratch);
    reviewable(
        &mut store,
        "Roki",
        &[scored(
            "bulk",
            r"@@bulk\Music\Ghost Rule EP\01 Roki.flac",
            100,
        )],
    );
    reviewable(
        &mut store,
        "Ghost Rule",
        &[scored(
            "bulk",
            r"@@bulk\Music\Ghost Rule EP\02 Ghost Rule.flac",
            100,
        )],
    );

    let offers = waiting(&store, 2);

    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].username, "bulk");
    assert_eq!(offers[0].name(), "Ghost Rule EP");
    assert_eq!(offers[0].covered(), 2);
    assert_eq!(offers[0].quality(), "2 flac");
}

#[test]
fn a_mixed_folder_says_so_in_its_quality() {
    let scratch = Scratch::new("albums-mixed");
    let mut store = store(&scratch);
    reviewable(
        &mut store,
        "Roki",
        &[scored("bulk", r"@@bulk\Music\Best Of\01 Roki.flac", 100)],
    );
    reviewable(
        &mut store,
        "Ghost Rule",
        &[scored(
            "bulk",
            r"@@bulk\Music\Best Of\02 Ghost Rule.mp3",
            100,
        )],
    );

    assert_eq!(waiting(&store, 2)[0].quality(), "1 flac, 1 mp3");
}

#[test]
fn two_peers_offering_the_same_folder_name_are_separate_offers() {
    let scratch = Scratch::new("albums-two-peers");
    let mut store = store(&scratch);
    for title in ["Roki", "Ghost Rule"] {
        reviewable(
            &mut store,
            title,
            &[
                scored("peerA", &format!(r"Music\Ghost Rule EP\{title}.flac"), 100),
                scored("peerB", &format!(r"Music\Ghost Rule EP\{title}.flac"), 100),
            ],
        );
    }

    let offers = waiting(&store, 2);

    assert_eq!(offers.len(), 2);
    assert_eq!(offers[0].folder, offers[1].folder);
    assert_ne!(offers[0].username, offers[1].username);
}

/// `Candidate::split_path` returns the whole path as *both* halves when there is
/// no separator, so a bare filename would group with itself and be offered as an
/// album named after a track.
#[test]
fn a_candidate_with_no_folder_is_not_an_album_of_one() {
    let scratch = Scratch::new("albums-no-folder");
    let mut store = store(&scratch);
    reviewable(&mut store, "Roki", &[scored("peerA", "Roki.flac", 100)]);
    reviewable(
        &mut store,
        "Ghost Rule",
        &[scored("peerA", "Roki.flac", 100)],
    );

    assert!(waiting(&store, 2).is_empty());
}

#[test]
fn a_rejected_candidate_is_never_part_of_an_offer() {
    let scratch = Scratch::new("albums-rejected");
    let mut store = store(&scratch);
    for title in ["Roki", "Ghost Rule"] {
        reviewable(
            &mut store,
            title,
            &[
                scored("peerA", &format!(r"Music\Studio\{title}.flac"), 100),
                scored("peerA", &format!(r"Music\Live 2019\{title}.flac"), -1),
            ],
        );
    }

    let offers = waiting(&store, 2);

    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].name(), "Studio");
}

#[test]
fn an_offer_below_the_minimum_is_not_listed() {
    let scratch = Scratch::new("albums-minimum");
    let mut store = store(&scratch);
    for title in ["Roki", "Ghost Rule"] {
        reviewable(
            &mut store,
            title,
            &[scored("bulk", &format!(r"Music\Best Of\{title}.flac"), 100)],
        );
    }

    assert_eq!(waiting(&store, 2).len(), 1);
    assert!(waiting(&store, 3).is_empty());
}

#[test]
fn the_best_rank_wins_when_one_folder_holds_two_files_for_a_track() {
    let scratch = Scratch::new("albums-two-files");
    let mut store = store(&scratch);
    let first = reviewable(
        &mut store,
        "Roki",
        &[
            scored("bulk", r"Music\Best Of\01 Roki.flac", 100),
            scored("bulk", r"Music\Best Of\01 Roki (alt).flac", 90),
        ],
    );
    reviewable(
        &mut store,
        "Ghost Rule",
        &[scored("bulk", r"Music\Best Of\02 Ghost Rule.flac", 100)],
    );

    let offers = waiting(&store, 2);

    assert_eq!(offers[0].covered(), 2);
    assert_eq!(offers[0].picks()[0], (first, 1));
}

#[test]
fn offers_are_ordered_by_coverage_then_by_what_they_cost() {
    let scratch = Scratch::new("albums-order");
    let mut store = store(&scratch);
    for title in ["Roki", "Ghost Rule", "Sanbika"] {
        reviewable(
            &mut store,
            title,
            &[scored("big", &format!(r"Music\Complete\{title}.flac"), 100)],
        );
    }
    for title in ["Andromeda", "Vampire"] {
        reviewable(
            &mut store,
            title,
            &[
                scored("cheap", &format!(r"Music\Singles\{title}.flac"), 100),
                scored("dear", &format!(r"Music\Rips\{title}.flac"), 80),
            ],
        );
    }

    let offers = waiting(&store, 2);

    let order: Vec<&str> = offers.iter().map(|offer| offer.username.as_str()).collect();
    assert_eq!(order, ["big", "cheap", "dear"]);
    assert_eq!(offers[1].sacrificed, 0);
    assert_eq!(offers[2].sacrificed, 40);
}

#[test]
fn a_track_already_decided_leaves_the_offer_it_was_part_of() {
    let scratch = Scratch::new("albums-decided");
    let mut store = store(&scratch);
    let first = reviewable(
        &mut store,
        "Roki",
        &[scored("bulk", r"Music\Best Of\01 Roki.flac", 100)],
    );
    reviewable(
        &mut store,
        "Ghost Rule",
        &[scored("bulk", r"Music\Best Of\02 Ghost Rule.flac", 100)],
    );
    assert_eq!(waiting(&store, 2).len(), 1);

    store.approve(first, 1).expect("an approval");

    assert!(waiting(&store, 2).is_empty());
}

#[test]
fn approving_an_album_approves_every_track_it_covers() {
    let scratch = Scratch::new("albums-approve");
    let mut store = store(&scratch);
    for title in ["Roki", "Ghost Rule"] {
        reviewable(
            &mut store,
            title,
            &[
                scored("solo", &format!(r"Music\Solo\{title}.flac"), 100),
                scored("bulk", &format!(r"Music\Best Of\{title}.flac"), 90),
            ],
        );
    }
    let offer = waiting(&store, 2)
        .into_iter()
        .find(|offer| offer.username == "bulk")
        .expect("the album offer");

    store.approve_all(&offer.picks()).expect("an approval");

    for (track_id, rank) in offer.picks() {
        let stored = store.track(track_id).expect("a lookup").expect("the track");
        assert_eq!(stored.status, TrackStatus::Approved);
        assert_eq!(stored.approved_rank, Some(rank));
    }
}

/// The constraint #6 lists: `ReviewTrack::approved` resolves the choice
/// positionally by `rank - 1` and `next_candidate` assumes dense `1..n` ranks,
/// so an album approval must not disturb either or the stall retry breaks too.
#[test]
fn approving_an_album_leaves_every_rank_resolvable() {
    let scratch = Scratch::new("albums-ranks");
    let mut store = store(&scratch);
    for title in ["Roki", "Ghost Rule"] {
        reviewable(
            &mut store,
            title,
            &[
                scored("solo", &format!(r"Music\Solo\{title}.flac"), 100),
                scored("bulk", &format!(r"Music\Best Of\{title}.flac"), 90),
            ],
        );
    }
    let offer = waiting(&store, 2)
        .into_iter()
        .find(|offer| offer.username == "bulk")
        .expect("the album offer");
    store.approve_all(&offer.picks()).expect("an approval");

    for track in &offer.tracks {
        let stored = store
            .track(track.track_id)
            .expect("a lookup")
            .expect("the track");
        assert_eq!(
            stored
                .approved()
                .expect("the approved candidate")
                .candidate
                .remote_path,
            track.scored.candidate.remote_path
        );
        // Nothing left after the album's own pick, so the retry gives up rather
        // than promoting a rank the renumbering invented.
        assert_eq!(store.next_candidate(track.track_id).expect("a retry"), None);
    }
}
