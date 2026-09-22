//! Ranking tests, built around the wrong downloads the original scorer allowed.
//!
//! Each case in the folder group is a shape the old filename-only scorer could
//! not distinguish: two results whose filenames are identical, differing only in
//! the remote folder they sit in.
//!
//! Ported from `py/tests/unit/test_ranking.py`.

use stk::config::{RankingSettings, UploaderSettings};
use stk::models::{Candidate, ScoredCandidate, Track};
use stk::soulseek::ranking::{is_audio, prefer_one_uploader, rank, score_folder, PREFERRED_FLAG};

fn track() -> Track {
    Track {
        artist: "DECO*27".to_owned(),
        title: "Roki".to_owned(),
        duration_seconds: Some(215.0),
        ..Track::default()
    }
}

fn candidate(path: &str) -> Candidate {
    Candidate {
        username: "peer".to_owned(),
        remote_path: path.to_owned(),
        size: 30_000_000,
        bit_depth: Some(16),
        sample_rate: Some(44100),
        free_upload_slots: true,
        queue_length: 0,
        ..Candidate::default()
    }
}

fn studio() -> Candidate {
    candidate("@@peer\\Music\\DECO27 - Ghost\\03 - Roki.flac")
}

fn ranked(track: &Track, candidates: &[Candidate]) -> Vec<ScoredCandidate> {
    rank(track, candidates, &RankingSettings::default(), None)
}

#[test]
fn a_suspicious_folder_ranks_below_an_identical_studio_file() {
    for folder in [
        "Symphony Concert 2019",
        "Tokyo Orchestra Live",
        "DECO27 Live at Budokan",
        "Vocaloid Medley Collection",
    ] {
        // Identical filenames. The old scorer saw no difference between these.
        let suspicious = candidate(&format!("@@peer\\Music\\{folder}\\03 - Roki.flac"));
        let results = ranked(&track(), &[suspicious.clone(), studio()]);

        assert_ne!(results[0].candidate, suspicious, "{folder}");
        assert_eq!(results[results.len() - 1].candidate, suspicious, "{folder}");
        assert!(!results[results.len() - 1].flags.is_empty(), "{folder}");
    }
}

#[test]
fn the_flag_names_what_was_found_in_the_path() {
    let results = ranked(
        &track(),
        &[candidate("@@peer\\Symphony Concert\\03 - Roki.flac")],
    );
    assert!(results[0].flags.contains(&"symphony".to_owned()));
}

#[test]
fn a_rejected_term_in_the_folder_drops_a_result_to_the_bottom() {
    let karaoke = candidate("@@peer\\Karaoke Versions\\03 - Roki.flac");
    let results = ranked(&track(), &[karaoke.clone(), studio()]);
    let last = &results[results.len() - 1];

    assert_eq!(last.candidate, karaoke);
    assert!(last.rejected());
    assert_eq!(last.flags, vec!["reject:karaoke".to_owned()]);
}

#[test]
fn a_term_in_the_filename_is_caught_too() {
    let results = ranked(
        &track(),
        &[candidate("@@peer\\Music\\03 - Roki (Live).flac")],
    );
    assert!(results[0].flags.contains(&"live".to_owned()));
}

#[test]
fn several_warnings_compound() {
    let one = candidate("@@peer\\Live Symphony\\03 - Roki.flac");
    let few = candidate("@@peer\\Live\\03 - Roki.flac");
    let results = ranked(&track(), &[one, few.clone()]);

    assert_eq!(results[0].candidate, few);
    assert!(results[1].flags.len() > results[0].flags.len());
}

#[test]
fn a_term_the_track_itself_contains_is_not_held_against_it() {
    // Someone wanting "Live and Learn" is not asking to reject every result
    // whose path says "live".
    let wanted = Track::new("Crush 40", "Live and Learn");
    let result = candidate("@@peer\\Sonic Adventure 2\\01 Live and Learn.flac");

    assert!(ranked(&wanted, &[result])[0].flags.is_empty());
}

#[test]
fn lossless_outranks_lossy_whatever_the_bitrate() {
    let lossy = Candidate {
        bit_depth: None,
        bitrate: Some(320),
        ..candidate("@@peer\\Music\\03 - Roki.mp3")
    };
    let results = ranked(&track(), &[lossy, studio()]);
    assert_eq!(results[0].candidate.extension(), "flac");
}

#[test]
fn a_higher_bitrate_wins_between_lossy_results() {
    let low = Candidate {
        bit_depth: None,
        bitrate: Some(128),
        ..candidate("@@a\\Music\\03 - Roki.mp3")
    };
    let high = Candidate {
        bit_depth: None,
        bitrate: Some(320),
        ..candidate("@@b\\Music\\03 - Roki.mp3")
    };

    assert_eq!(ranked(&track(), &[low, high.clone()])[0].candidate, high);
}

#[test]
fn hi_res_outranks_cd_quality() {
    let hires = Candidate {
        sample_rate: Some(96000),
        bit_depth: Some(24),
        ..candidate("@@b\\Music\\03 - Roki.flac")
    };

    assert_eq!(
        ranked(&track(), &[studio(), hires.clone()])[0].candidate,
        hires
    );
}

#[test]
fn a_free_slot_breaks_a_tie() {
    let busy = Candidate {
        free_upload_slots: false,
        ..candidate("@@a\\Music\\03 - Roki.flac")
    };
    let free = candidate("@@b\\Music\\03 - Roki.flac");

    assert_eq!(ranked(&track(), &[busy, free.clone()])[0].candidate, free);
}

#[test]
fn a_long_queue_costs_a_result() {
    let short = candidate("@@a\\Music\\03 - Roki.flac");
    let long = Candidate {
        queue_length: 400,
        ..candidate("@@b\\Music\\03 - Roki.flac")
    };

    assert_eq!(ranked(&track(), &[long, short.clone()])[0].candidate, short);
}

#[test]
fn a_result_with_no_quality_information_still_ranks() {
    let unknown = Candidate {
        bit_depth: None,
        sample_rate: None,
        ..candidate("@@a\\Music\\03 - Roki.mp3")
    };

    assert!(!ranked(&track(), &[unknown]).is_empty());
}

#[test]
fn non_audio_files_are_dropped() {
    assert!(ranked(&track(), &[candidate("@@peer\\Music\\Roki.cue")]).is_empty());
    assert!(ranked(&track(), &[candidate("@@peer\\Music\\cover.jpg")]).is_empty());
}

#[test]
fn a_file_named_after_another_song_is_dropped() {
    assert!(ranked(
        &track(),
        &[candidate("@@peer\\Music\\Night of Knights.flac")]
    )
    .is_empty());
}

#[test]
fn a_file_named_after_the_artist_alone_is_kept() {
    // Peers file untitled rips under the artist's name; a human can still judge it.
    assert!(!ranked(&track(), &[candidate("@@peer\\Music\\DECO 27.flac")]).is_empty());
}

#[test]
fn punctuation_differences_do_not_lose_a_match() {
    assert!(!ranked(&track(), &[candidate("@@peer\\Music\\DECO_27 - Roki.flac")]).is_empty());
}

#[test]
fn a_track_number_prefix_does_not_lose_a_match() {
    assert!(!ranked(&track(), &[candidate("@@peer\\Music\\03. Roki.flac")]).is_empty());
}

#[test]
fn the_minimum_ratio_is_configurable() {
    let loose = candidate("@@peer\\Music\\Rokii.flac");
    let strict = RankingSettings {
        min_title_ratio: 0.99,
        ..RankingSettings::default()
    };
    let lenient = RankingSettings {
        min_title_ratio: 0.3,
        ..RankingSettings::default()
    };

    assert!(rank(&track(), std::slice::from_ref(&loose), &strict, None).is_empty());
    assert!(!rank(&track(), &[loose], &lenient, None).is_empty());
}

#[test]
fn a_result_of_the_wrong_length_is_penalised() {
    let right = Candidate {
        duration_seconds: Some(215.0),
        ..candidate("@@a\\Music\\03 - Roki.flac")
    };
    let wrong = Candidate {
        duration_seconds: Some(480.0),
        ..candidate("@@b\\Music\\03 - Roki.flac")
    };

    let results = ranked(&track(), &[wrong, right.clone()]);
    assert_eq!(results[0].candidate, right);
    assert!(results[1].flags.contains(&"duration".to_owned()));
}

#[test]
fn a_small_difference_is_tolerated() {
    let close = Candidate {
        duration_seconds: Some(218.0),
        ..candidate("@@a\\Music\\03 - Roki.flac")
    };

    assert!(ranked(&track(), &[close])[0].flags.is_empty());
}

#[test]
fn nothing_is_penalised_when_a_length_is_unknown() {
    let unknown = candidate("@@a\\Music\\03 - Roki.flac");
    let wanted = Track::new("DECO*27", "Roki");

    assert!(ranked(&wanted, std::slice::from_ref(&unknown))[0]
        .flags
        .is_empty());
    assert!(ranked(&track(), &[unknown])[0].flags.is_empty());
}

#[test]
fn results_come_back_best_first() {
    let lossy = Candidate {
        bit_depth: None,
        bitrate: Some(128),
        ..candidate("@@b\\Music\\03 - Roki.mp3")
    };
    let results = ranked(
        &track(),
        &[candidate("@@a\\Live\\03 - Roki.flac"), lossy, studio()],
    );

    let scores: Vec<i64> = results.iter().map(|result| result.score).collect();
    let mut sorted = scores.clone();
    sorted.sort_unstable_by(|left, right| right.cmp(left));
    assert_eq!(scores, sorted);
}

#[test]
fn the_number_of_results_can_be_capped() {
    let many: Vec<Candidate> = (0..10)
        .map(|index| candidate(&format!("@@peer{index}\\Music\\03 - Roki.flac")))
        .collect();

    assert_eq!(
        rank(&track(), &many, &RankingSettings::default(), Some(3)).len(),
        3
    );
}

#[test]
fn no_candidates_yields_no_results() {
    assert!(ranked(&track(), &[]).is_empty());
}

/// A batch is a list per track; these are built by hand so the pass is tested
/// without a client, a store or a search.
fn scored(username: &str, score: i64) -> ScoredCandidate {
    ScoredCandidate {
        candidate: Candidate {
            username: username.to_owned(),
            remote_path: format!(r"@@{username}\Music\track.flac"),
            ..Candidate::default()
        },
        score,
        title_ratio: 1.0,
        flags: Vec::new(),
    }
}

fn uploader() -> UploaderSettings {
    UploaderSettings::default()
}

fn first_peers(batch: &[Vec<ScoredCandidate>]) -> Vec<&str> {
    batch
        .iter()
        .map(|list| {
            list.first()
                .map_or("", |scored| scored.candidate.username.as_str())
        })
        .collect()
}

/// Three tracks, each won by a different peer, with `bulk` a close second on
/// every one of them. The shape the issue describes: twelve tracks from twelve
/// strangers, or all of them from the one peer who has the lot.
fn close_batch() -> Vec<Vec<ScoredCandidate>> {
    (0..3)
        .map(|index| {
            vec![
                scored(&format!("solo{index}"), 37_000_000),
                scored("bulk", 36_999_500),
            ]
        })
        .collect()
}

#[test]
fn a_peer_covering_more_of_the_batch_wins_a_close_race() {
    let mut batch = close_batch();

    let preferred = prefer_one_uploader(&mut batch, &uploader()).expect("a preferred peer");

    assert_eq!(preferred.username, "bulk");
    assert_eq!(preferred.covered, 3);
    assert_eq!(first_peers(&batch), vec!["bulk", "bulk", "bulk"]);
}

#[test]
fn a_promoted_candidate_says_why_it_moved() {
    // A silent reordering of a list a human reads is worse than none.
    let mut batch = close_batch();
    prefer_one_uploader(&mut batch, &uploader()).expect("a preferred peer");

    assert!(batch[0][0].flags.iter().any(|flag| flag == PREFERRED_FLAG));
    assert!(
        batch[0][1].flags.is_empty(),
        "the one it passed is untouched"
    );
}

#[test]
fn a_peer_covering_fewer_tracks_than_the_minimum_changes_nothing() {
    let mut batch: Vec<Vec<ScoredCandidate>> = (0..2)
        .map(|index| {
            vec![
                scored(&format!("solo{index}"), 37_000_000),
                scored("bulk", 36_999_500),
            ]
        })
        .collect();

    assert_eq!(prefer_one_uploader(&mut batch, &uploader()), None);
    assert_eq!(first_peers(&batch), vec!["solo0", "solo1"]);
}

#[test]
fn a_batch_of_one_track_is_left_alone() {
    let mut batch = vec![vec![
        scored("solo0", 37_000_000),
        scored("bulk", 36_999_500),
    ]];

    assert_eq!(prefer_one_uploader(&mut batch, &uploader()), None);
}

#[test]
fn preferring_one_uploader_never_swaps_a_lossless_file_for_a_lossy_one() {
    // A quality tier is 10,000,000 apart; the window is 2,000.
    let mut batch: Vec<Vec<ScoredCandidate>> = (0..5)
        .map(|index| {
            vec![
                scored(&format!("solo{index}"), 37_000_000),
                scored("bulk", 27_000_000),
            ]
        })
        .collect();

    assert_eq!(prefer_one_uploader(&mut batch, &uploader()), None);
    assert_eq!(first_peers(&batch)[0], "solo0");
}

#[test]
fn a_penalised_candidate_is_never_promoted_to_keep_one_uploader() {
    // One penalise term costs 2,000,000 -- a live recording is not a close call.
    let mut batch: Vec<Vec<ScoredCandidate>> = (0..5)
        .map(|index| {
            vec![
                scored(&format!("solo{index}"), 37_000_000),
                scored("bulk", 35_000_000),
            ]
        })
        .collect();

    assert_eq!(prefer_one_uploader(&mut batch, &uploader()), None);
}

#[test]
fn a_rejected_candidate_is_never_promoted() {
    let mut batch: Vec<Vec<ScoredCandidate>> = (0..4)
        .map(|index| {
            vec![
                scored(&format!("solo{index}"), 37_000_000),
                scored("bulk", -1),
            ]
        })
        .collect();

    assert_eq!(prefer_one_uploader(&mut batch, &uploader()), None);
}

#[test]
fn the_batch_pass_keeps_the_rest_of_a_list_in_score_order() {
    let mut batch: Vec<Vec<ScoredCandidate>> = (0..3)
        .map(|index| {
            vec![
                scored(&format!("solo{index}"), 37_000_000),
                scored(&format!("other{index}"), 36_999_800),
                scored("bulk", 36_999_500),
            ]
        })
        .collect();

    prefer_one_uploader(&mut batch, &uploader()).expect("a preferred peer");

    assert_eq!(
        batch[0]
            .iter()
            .map(|scored| scored.candidate.username.as_str())
            .collect::<Vec<_>>(),
        vec!["bulk", "solo0", "other0"]
    );
}

#[test]
fn the_preferred_peer_is_the_same_whatever_order_the_tracks_arrive_in() {
    // Coverage is counted against the un-reordered lists for exactly this.
    let mut forwards = close_batch();
    let mut backwards = close_batch();
    backwards.reverse();

    assert_eq!(
        prefer_one_uploader(&mut forwards, &uploader()),
        prefer_one_uploader(&mut backwards, &uploader())
    );
}

#[test]
fn a_tie_on_coverage_is_settled_by_what_it_costs() {
    let mut batch: Vec<Vec<ScoredCandidate>> = (0..3)
        .map(|index| {
            vec![
                scored(&format!("solo{index}"), 37_000_000),
                scored("cheap", 36_999_900),
                scored("dear", 36_999_100),
            ]
        })
        .collect();

    let preferred = prefer_one_uploader(&mut batch, &uploader()).expect("a preferred peer");

    assert_eq!(preferred.username, "cheap");
    assert_eq!(preferred.sacrificed, 300);
}

#[test]
fn preferring_one_uploader_can_be_widened_or_narrowed() {
    let narrow = UploaderSettings {
        score_window: 100,
        ..uploader()
    };
    let mut batch = close_batch();

    assert_eq!(prefer_one_uploader(&mut batch, &narrow), None);
    assert_eq!(first_peers(&batch)[0], "solo0");
}

#[test]
fn an_empty_batch_settles_on_nobody() {
    assert_eq!(prefer_one_uploader(&mut [], &uploader()), None);
    assert_eq!(prefer_one_uploader(&mut [Vec::new()], &uploader()), None);
}

/// Scoring a folder rather than a file (#13).
///
/// `stk get --album` starts from an album name, so there is no tracklist and no
/// title to hold a filename against. What is left is the half of the scoring
/// that reads the whole remote path, and it has to read the folder's files
/// together: a folder is queued whole or not at all.
fn lossy(path: &str, bitrate: u32) -> Candidate {
    Candidate {
        bit_depth: None,
        sample_rate: None,
        bitrate: Some(bitrate),
        ..candidate(path)
    }
}

fn album(folder: &str, names: &[&str]) -> Vec<Candidate> {
    names
        .iter()
        .map(|name| candidate(&format!("{folder}\\{name}")))
        .collect()
}

fn ghost_rule() -> Vec<Candidate> {
    album(
        "@@peer\\Music\\DECO27 - Ghost Rule EP",
        &["01 - Roki.flac", "02 - Ghost Rule.flac", "03 - Aimai.flac"],
    )
}

#[test]
fn a_folder_of_audio_is_worth_scoring() {
    let scored = score_folder(
        &ghost_rule(),
        &RankingSettings::default(),
        "deco 27 ghost rule",
    )
    .expect("a scored folder");

    assert!(scored.score > 0);
    assert!(scored.flags.is_empty());
    assert_eq!(scored.below_quality, 0);
}

#[test]
fn a_folder_with_no_audio_in_it_is_not_a_folder_worth_offering() {
    let paperwork = album(
        "@@peer\\Music\\Ghost Rule EP",
        &["folder.jpg", "ghost.cue", "ghost.log"],
    );

    assert_eq!(
        score_folder(&paperwork, &RankingSettings::default(), "ghost rule"),
        None
    );
}

/// A file is scored on its own path; a folder is taken whole, so one karaoke
/// track inside it is evidence about every track inside it.
#[test]
fn a_rejected_term_on_any_file_rejects_the_whole_folder() {
    let mixed = album(
        "@@peer\\Music\\Ghost Rule EP",
        &["01 - Roki.flac", "02 - Ghost Rule (Karaoke).flac"],
    );
    let scored =
        score_folder(&mixed, &RankingSettings::default(), "ghost rule").expect("a scored folder");

    assert!(scored.score < 0);
    assert_eq!(scored.flags, vec!["reject:karaoke".to_owned()]);
}

#[test]
fn a_penalised_term_in_the_folder_name_costs_the_whole_folder() {
    let options = RankingSettings::default();
    let live = album(
        "@@peer\\Music\\DECO27 Live at Budokan",
        &["01 - Roki.flac", "02 - Ghost Rule.flac"],
    );

    let studio = score_folder(&ghost_rule(), &options, "deco 27 ghost rule").expect("a folder");
    let concert = score_folder(&live, &options, "deco 27 ghost rule").expect("a folder");

    assert!(concert.flags.contains(&"live".to_owned()));
    assert_eq!(studio.score - concert.score, options.penalty_per_term);
}

/// The carve-out `terms_in` makes for a single file has to hold for a folder:
/// nobody asking for *Live and Learn* is asking to reject every path saying
/// "live".
#[test]
fn a_term_the_album_itself_contains_is_not_held_against_it() {
    let asked_for = album(
        "@@peer\\Music\\Live and Learn",
        &["01 - Open Your Heart.flac", "02 - Live and Learn.flac"],
    );
    let scored =
        score_folder(&asked_for, &RankingSettings::default(), "live and learn").expect("a folder");

    assert!(scored.flags.is_empty());
    assert!(scored.score > 0);
}

/// The mean, not the sum. A folder padded out with bonus tracks is not a better
/// rip of the record than a folder holding the record.
#[test]
fn a_longer_folder_does_not_outscore_a_better_one() {
    let padded: Vec<Candidate> = (1..=20)
        .map(|number| lossy(&format!("@@a\\Music\\Best Of\\{number:02} - Roki.mp3"), 320))
        .collect();
    let options = RankingSettings::default();

    let short = score_folder(&ghost_rule(), &options, "ghost rule").expect("a folder");
    let long = score_folder(&padded, &options, "best of").expect("a folder");

    assert!(short.score > long.score);
}

#[test]
fn a_peers_free_slot_and_queue_move_the_whole_folder() {
    let options = RankingSettings::default();
    let busy: Vec<Candidate> = ghost_rule()
        .into_iter()
        .map(|file| Candidate {
            free_upload_slots: false,
            queue_length: 40,
            ..file
        })
        .collect();

    let free = score_folder(&ghost_rule(), &options, "ghost rule").expect("a folder");
    let waiting = score_folder(&busy, &options, "ghost rule").expect("a folder");

    assert_eq!(free.score - waiting.score, 1000 + 40);
}

/// A folder is taken whole, so the one MP3 among the FLACs is a warning. Hiding
/// it would describe a folder that is not the one on the peer's disk, and the
/// one on the peer's disk is what would arrive.
#[test]
fn a_folder_that_is_only_partly_lossless_is_warned_about_rather_than_dropped() {
    let mut mixed = ghost_rule();
    mixed.push(lossy(
        "@@peer\\Music\\DECO27 - Ghost Rule EP\\04 - Bonus.mp3",
        192,
    ));
    let options = RankingSettings {
        require_lossless: true,
        ..RankingSettings::default()
    };

    let scored = score_folder(&mixed, &options, "ghost rule").expect("a folder");

    assert_eq!(scored.below_quality, 1);
    assert!(scored.score > 0);
}

#[test]
fn a_folder_nothing_in_which_clears_the_floor_is_gone() {
    let all_lossy = vec![
        lossy("@@peer\\Music\\Ghost Rule EP\\01 - Roki.mp3", 192),
        lossy("@@peer\\Music\\Ghost Rule EP\\02 - Ghost Rule.mp3", 192),
    ];
    let options = RankingSettings {
        min_bitrate: Some(320),
        ..RankingSettings::default()
    };

    assert_eq!(score_folder(&all_lossy, &options, "ghost rule"), None);
}

#[test]
fn only_audio_counts_as_audio() {
    for (path, audio) in [
        ("@@peer\\Music\\01 - Roki.flac", true),
        ("@@peer\\Music\\01 - Roki.mp3", true),
        ("@@peer\\Music\\01 - Roki.opus", true),
        ("@@peer\\Music\\ghost.cue", false),
        ("@@peer\\Music\\folder.jpg", false),
        ("@@peer\\Music\\readme", false),
    ] {
        assert_eq!(is_audio(&candidate(path)), audio, "is_audio({path:?})");
    }
}
