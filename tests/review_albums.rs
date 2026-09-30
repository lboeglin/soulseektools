//! Grouping by folder, on a real `ReviewStore` so the rank numbering is the database's.

mod support;

use stk::config::RankingSettings;
use stk::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use stk::review::albums::{album_rows, folders, offers, queue_folder, AlbumOffer, FolderOffer};
use stk::review::store::{Retry, ReviewStore};
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

/// Regression: measured against rank 1, the cost went negative after a batch promotion.
#[test]
fn what_a_folder_costs_is_measured_against_the_best_score_not_the_first_rank() {
    let scratch = Scratch::new("albums-promoted-head");
    let mut store = store(&scratch);
    for title in ["Roki", "Ghost Rule"] {
        reviewable(
            &mut store,
            title,
            &[
                // Promoted, and scoring lower than the candidate behind it.
                scored("bulk", &format!(r"Music\Best Of\{title}.flac"), 90),
                scored("solo", &format!(r"Music\Solo\{title}.flac"), 100),
            ],
        );
    }

    let offers = waiting(&store, 2);

    assert!(
        offers.iter().all(|offer| offer.sacrificed >= 0),
        "{offers:?}"
    );
    let by_peer = |username: &str| {
        offers
            .iter()
            .find(|offer| offer.username == username)
            .expect("the offer")
            .sacrificed
    };
    assert_eq!(by_peer("solo"), 0);
    assert_eq!(by_peer("bulk"), 20);
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

/// Approval resolves by `rank - 1` and the retry assumes dense ranks.
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
    }
}

/// Regression: the retry promoted only `rank + 1`, so rank 1 was never tried.
#[test]
fn a_failed_album_pick_falls_back_to_the_track_it_was_preferred_over() {
    let scratch = Scratch::new("albums-fallback");
    let mut store = store(&scratch);
    let track_id = reviewable(
        &mut store,
        "Roki",
        &[
            scored("solo", r"Music\Solo\Roki.flac", 100),
            scored("bulk", r"Music\Best Of\Roki.flac", 90),
        ],
    );
    reviewable(
        &mut store,
        "Ghost Rule",
        &[
            scored("solo", r"Music\Solo\Ghost Rule.flac", 100),
            scored("bulk", r"Music\Best Of\Ghost Rule.flac", 90),
        ],
    );
    let offer = waiting(&store, 2)
        .into_iter()
        .find(|offer| offer.username == "bulk")
        .expect("the album offer");
    store.approve_all(&offer.picks()).expect("an approval");
    assert_eq!(
        store
            .track(track_id)
            .expect("a lookup")
            .expect("the track")
            .approved_rank,
        Some(2)
    );

    assert_eq!(
        store.next_candidate(track_id).expect("a retry"),
        Retry::Approved(1),
        "the candidate the album was preferred over is still worth trying"
    );
    assert_eq!(
        store.next_candidate(track_id).expect("a retry"),
        Retry::Exhausted
    );
    assert_eq!(
        store
            .track(track_id)
            .expect("a lookup")
            .expect("the track")
            .status,
        TrackStatus::Failed
    );
}

#[test]
fn a_file_shared_at_a_peers_root_is_not_an_album() {
    let scratch = Scratch::new("albums-share-root");
    let mut store = store(&scratch);
    reviewable(
        &mut store,
        "Roki",
        &[scored("bulk", r"@@bulk\Roki.flac", 100)],
    );
    reviewable(
        &mut store,
        "Ghost Rule",
        &[scored("bulk", r"@@bulk\Ghost Rule.flac", 100)],
    );

    assert!(waiting(&store, 2).is_empty());
}

fn found(username: &str, remote_path: &str) -> Candidate {
    Candidate {
        username: username.to_owned(),
        remote_path: remote_path.to_owned(),
        size: 30_000_000,
        sample_rate: Some(44100),
        bit_depth: Some(16),
        free_upload_slots: true,
        ..Candidate::default()
    }
}

fn listing(username: &str, folder: &str, names: &[&str]) -> Vec<Candidate> {
    names
        .iter()
        .map(|name| found(username, &format!("{folder}\\{name}")))
        .collect()
}

fn ghost_rule(username: &str) -> Vec<Candidate> {
    listing(
        username,
        "@@peer\\Music\\DECO27 - Ghost Rule EP",
        &[
            "01 - Roki.flac",
            "02 - Ghost Rule.flac",
            "03 - Aimai Elegy.flac",
        ],
    )
}

fn offered(candidates: &[Candidate], minimum: usize) -> Vec<FolderOffer> {
    folders(
        candidates,
        &RankingSettings::default(),
        "deco 27 ghost rule ep",
        minimum,
    )
}

#[test]
fn a_search_becomes_one_offer_per_peer_and_folder() {
    let mut results = ghost_rule("bulk");
    results.extend(listing(
        "peerC",
        "@@peer\\Shared\\Ghost Rule",
        &["Roki.flac", "Ghost Rule.flac"],
    ));

    let offers = offered(&results, 2);

    assert_eq!(offers.len(), 2);
    assert_eq!(offers[0].username, "bulk");
    assert_eq!(offers[0].name(), "DECO27 - Ghost Rule EP");
    assert_eq!(offers[0].count(), 3);
    assert_eq!(offers[1].username, "peerC");
    assert_eq!(offers[1].count(), 2);
}

#[test]
fn a_folders_paperwork_is_counted_and_never_queued() {
    let mut results = ghost_rule("bulk");
    results.extend(listing(
        "bulk",
        "@@peer\\Music\\DECO27 - Ghost Rule EP",
        &["ghost.cue", "ghost.log", "folder.jpg"],
    ));

    let offers = offered(&results, 2);

    assert_eq!(offers[0].count(), 3);
    assert_eq!(offers[0].extras, 3);
    assert!(offers[0]
        .files
        .iter()
        .all(|file| file.candidate.extension() == "flac"));
}

#[test]
fn a_folder_becomes_one_row_per_file_with_the_album_and_the_number_on_it() {
    let offers = offered(&ghost_rule("bulk"), 2);
    let rows = album_rows(&offers[0], "DECO*27", "Ghost Rule EP");

    assert_eq!(rows.len(), 3);
    let titles: Vec<&str> = rows.iter().map(|(track, _)| track.title.as_str()).collect();
    assert_eq!(titles, vec!["Roki", "Ghost Rule", "Aimai Elegy"]);

    for (track, _) in &rows {
        assert_eq!(track.artist, "DECO*27");
        assert_eq!(track.album, "Ghost Rule EP");
        assert!(track.title_guessed);
    }
    assert_eq!(
        rows[2].0.source_value(&["track number"]),
        Some("3"),
        "the filename's own number"
    );
}

/// The queue's key is `artist - title`, so the second would take the first's row.
#[test]
fn two_files_giving_the_same_title_yield_one_row() {
    let twice = listing(
        "bulk",
        "@@peer\\Music\\Ghost Rule EP",
        &["01 - Roki.flac", "01 - Roki.mp3", "02 - Ghost Rule.flac"],
    );
    let offers = offered(&twice, 2);
    let rows = album_rows(&offers[0], "DECO*27", "Ghost Rule EP");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1.candidate.extension(), "flac");
}

#[test]
fn a_track_already_on_its_way_is_left_alone() {
    let scratch = Scratch::new("albums-queue-skips");
    let mut store = store(&scratch);
    let offers = offered(&ghost_rule("bulk"), 2);
    let rows = album_rows(&offers[0], "DECO*27", "Ghost Rule EP");

    let waiting = reviewable(
        &mut store,
        "Roki",
        &[scored("someone", "@@x\\Roki.flac", 10)],
    );
    store
        .mark(waiting, TrackStatus::Downloaded, "already here")
        .expect("a downloaded track");

    let outcome = queue_folder(&mut store, &rows).expect("a queued folder");

    assert_eq!(outcome.queued, 2);
    assert_eq!(outcome.skipped, vec!["DECO*27 - Roki".to_owned()]);
    let entry = store.track(waiting).expect("the track").expect("a row");
    assert_eq!(entry.status, TrackStatus::Downloaded);
    assert_eq!(entry.candidates.len(), 1);
    assert_eq!(entry.candidates[0].candidate.username, "someone");
}

#[test]
fn a_folder_must_carry_the_numbers_of_the_album_asked_for() {
    use stk::review::albums::names_its_numbers;

    assert!(!names_its_numbers(
        r"Music\Kikuo\Kikuo Miku 7",
        "Kikuo Miku 3"
    ));
    assert!(names_its_numbers(
        r"Music\Kikuo\Kikuo Miku 3\CD1",
        "Kikuo Miku 3"
    ));
    assert!(names_its_numbers(
        r"Music\YOASOBI\THE BOOK 03",
        "THE BOOK 3"
    ));
    assert!(names_its_numbers(r"Music\DECO_27\GHOST", "GHOST"));
}

#[test]
fn a_filename_naming_the_album_too_yields_just_the_title() {
    let named = listing(
        "bulk",
        r"@@peer\Music\Kikuo Miku 7",
        &[
            "Kikuo - Kikuo Miku 7 - 01 Twins at the carousel.flac",
            "Kikuo - Kikuo Miku 7 - 11 Astral Travel.flac",
        ],
    );
    let offers = offered(&named, 2);
    let rows = album_rows(&offers[0], "Kikuo", "Kikuo Miku 7");

    let titles: Vec<&str> = rows.iter().map(|(track, _)| track.title.as_str()).collect();
    assert_eq!(titles, vec!["Twins at the carousel", "Astral Travel"]);
    assert_eq!(rows[1].0.source_value(&["track number"]), Some("11"));
}
