//! Ranking tests, built around the wrong downloads the original scorer allowed.
//!
//! Each case in the folder group is a shape the old filename-only scorer could
//! not distinguish: two results whose filenames are identical, differing only in
//! the remote folder they sit in.
//!
//! Ported from `py/tests/unit/test_ranking.py`.

use stk::config::{RankingSettings, UploaderSettings};
use stk::models::{Candidate, ScoredCandidate, Track};
use stk::soulseek::ranking::{prefer_one_uploader, rank, PREFERRED_FLAG};

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
