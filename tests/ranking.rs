//! Search result ranking, per file, per batch and per folder.

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
        // Identical filenames: only the folder tells them apart.
        let suspicious = candidate(&format!("@@peer\\Music\\{folder}\\03 - Roki.flac"));
        let results = ranked(&track(), &[suspicious.clone(), studio()]);

        assert_ne!(results[0].candidate, suspicious, "{folder}");
        assert_eq!(results[results.len() - 1].candidate, suspicious, "{folder}");
        assert!(!results[results.len() - 1].flags.is_empty(), "{folder}");
    }
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
fn a_term_the_track_itself_contains_is_not_held_against_it() {
    let wanted = Track::new("Crush 40", "Live and Learn");
    let result = candidate("@@peer\\Sonic Adventure 2\\01 Live and Learn.flac");

    assert!(ranked(&wanted, &[result])[0].flags.is_empty());
}

#[test]
fn a_wanted_word_that_merely_contains_a_term_keeps_its_penalty() {
    let wanted = Track::new("Daft Punk", "Discovery");
    let result = candidate("@@peer\\Covers\\01 Discovery.flac");

    assert_eq!(
        ranked(&wanted, &[result])[0].flags,
        vec!["cover".to_owned()]
    );
}

#[test]
fn a_term_inside_a_longer_word_is_not_a_match() {
    let result = candidate("@@peer\\Discover Weekly\\03 - Roki.flac");

    assert!(ranked(&track(), &[result])[0].flags.is_empty());
}

#[test]
fn a_term_starting_a_longer_word_still_counts() {
    let result = candidate("@@peer\\DECO27 Remixes\\03 - Roki.flac");

    assert_eq!(
        ranked(&track(), &[result])[0].flags,
        vec!["remix".to_owned()]
    );
}

#[test]
fn a_term_right_after_japanese_text_still_counts() {
    let result = candidate("@@peer\\ロキlive\\03 - Roki.flac");

    assert_eq!(
        ranked(&track(), &[result])[0].flags,
        vec!["live".to_owned()]
    );
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
fn a_file_named_after_another_song_is_dropped() {
    assert!(ranked(
        &track(),
        &[candidate("@@peer\\Music\\Night of Knights.flac")]
    )
    .is_empty());
}

#[test]
fn a_file_named_after_the_artist_alone_is_kept() {
    // Peers file untitled rips under the artist's name.
    assert!(!ranked(&track(), &[candidate("@@peer\\Music\\DECO 27.flac")]).is_empty());
}

#[test]
fn a_respelled_filename_still_matches() {
    for path in [
        r"@@peer\Music\DECO_27 - Roki.flac",
        r"@@peer\Music\03. Roki.flac",
    ] {
        assert!(!ranked(&track(), &[candidate(path)]).is_empty(), "{path}");
    }
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
    let mut batch = close_batch();
    prefer_one_uploader(&mut batch, &uploader()).expect("a preferred peer");

    assert!(batch[0][0].flags.iter().any(|flag| flag == PREFERRED_FLAG));
    assert!(
        batch[0][1].flags.is_empty(),
        "the one it passed is untouched"
    );
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
fn the_preferred_peer_is_the_same_whatever_order_the_tracks_arrive_in() {
    let mut forwards = close_batch();
    let mut backwards = close_batch();
    backwards.reverse();

    assert_eq!(
        prefer_one_uploader(&mut forwards, &uploader()),
        prefer_one_uploader(&mut backwards, &uploader())
    );
}

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
