//! Ported from `py/tests/unit/test_fingerprint.py`.
//!
//! The Python monkeypatches `shutil.which` and `subprocess.run` to exercise
//! backend selection and fpcalc parsing without the binary. There is no backend
//! to select any more and no binary to stand in for: the fingerprint is
//! computed in-process, so the comparison is tested on values written here and
//! the computation on files decoded for real.

mod support;

use stk::audio::fingerprint::{fingerprint, similarity, Fingerprint};
use support::{committed, encode, Encoded, Scratch};

fn print(values: Vec<u32>) -> Fingerprint {
    Fingerprint {
        values,
        duration_seconds: None,
    }
}

/// Words unique to `seed`, standing in for an unrelated track.
///
/// Deliberately not a shift of another sequence: the alignment search would
/// rightly call a shifted fingerprint the same audio.
fn unrelated(seed: u64, length: usize) -> Fingerprint {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    print(
        (0..length)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (state >> 32) as u32
            })
            .collect(),
    )
}

#[test]
fn a_fingerprint_matches_itself() {
    let values = unrelated(7, 40);
    assert!((similarity(&values, &values) - 1.0).abs() < 1e-9);
}

#[test]
fn the_same_track_shifted_in_time_still_matches() {
    // Two rips of one track routinely differ by a second of leading silence,
    // which shifts every word along without changing any of them.
    let values: Vec<u32> = (1000..1060).collect();
    let shifted = values[3..].to_vec();

    assert!(similarity(&print(values), &print(shifted)) > 0.95);
}

#[test]
fn unrelated_fingerprints_do_not_match() {
    assert!(similarity(&unrelated(1, 60), &unrelated(2, 60)) < 0.9);
}

#[test]
fn an_empty_fingerprint_matches_nothing() {
    assert!(similarity(&print(Vec::new()), &print(vec![1, 2, 3])) < f64::EPSILON);
}

#[test]
fn too_few_frames_to_compare_scores_zero() {
    let short = print(vec![1, 2, 3]);
    assert!(similarity(&short, &short) < f64::EPSILON);
}

#[test]
fn fingerprints_differing_in_every_bit_do_not_match() {
    let values = print(vec![0; 30]);
    let inverted = print(vec![0xFFFF_FFFF; 30]);

    assert!(similarity(&values, &inverted) < 0.1);
}

#[test]
fn a_fingerprint_with_a_few_altered_bits_still_matches() {
    // A re-encode of the same recording changes a bit here and there; that is
    // the whole reason the comparison is a bit error rate and not equality.
    let values: Vec<u32> = (2000..2040).collect();
    let altered: Vec<u32> = values.iter().map(|value| value ^ 0b1).collect();

    assert!(similarity(&print(values), &print(altered)) > 0.95);
}

#[test]
fn the_same_audio_in_two_containers_fingerprints_alike() {
    // The pairing the duplicate scan exists for: one track, ripped twice, as
    // lossless and as lossy, sharing no metadata and differing in file size.
    // Both are committed, because only a real encoder can produce the second.
    let left = fingerprint(&committed("pair.flac")).expect("a fingerprint");
    let right = fingerprint(&committed("pair.mp3")).expect("a fingerprint");

    assert!(similarity(&left, &right) > 0.9);
}

#[test]
fn two_different_tracks_do_not_fingerprint_alike() {
    let scratch = Scratch::new("fp-different");
    let first = encode(
        &scratch.join("a.flac"),
        &Encoded {
            seconds: 8.0,
            frequency: 300,
            modulation_hz: 0.7,
            ..Encoded::default()
        },
    );
    let second = encode(
        &scratch.join("b.flac"),
        &Encoded {
            seconds: 8.0,
            frequency: 415,
            modulation_hz: 2.9,
            ..Encoded::default()
        },
    );

    let left = fingerprint(&first).expect("a fingerprint");
    let right = fingerprint(&second).expect("a fingerprint");

    assert!(similarity(&left, &right) < 0.9);
}

#[test]
fn a_fingerprint_records_the_duration() {
    let scratch = Scratch::new("fp-duration");
    let track = encode(
        &scratch.join("a.flac"),
        &Encoded {
            seconds: 5.0,
            ..Encoded::default()
        },
    );

    let result = fingerprint(&track).expect("a fingerprint");
    assert!((result.duration_seconds.expect("a duration") - 5.0).abs() < 0.2);
}

#[test]
fn a_container_with_no_decoder_says_so_rather_than_scoring_zero() {
    // Opus is taggable and undecodable here, and a duplicate scan that silently
    // returned an empty fingerprint for it would report "not a duplicate".
    let scratch = Scratch::new("fp-undecodable");
    let track = encode(&scratch.join("a.opus"), &Encoded::default());

    let error = fingerprint(&track).expect_err("no decoder");
    assert!(error.to_string().contains("no decoder"), "{error}");
}
