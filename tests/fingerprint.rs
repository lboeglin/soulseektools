//! Chromaprint fingerprints: the comparison on literal values, the
//! computation on real decoded files.

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
fn too_few_frames_to_compare_scores_zero() {
    let short = print(vec![1, 2, 3]);
    assert!(similarity(&short, &short) < f64::EPSILON);
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

#[test]
fn the_compressed_form_names_the_algorithm_and_the_length() {
    // AcoustID reads the first byte as the algorithm and the next three as the
    // number of values, so a print encoded under any other preset than the one
    // computed with would be looked up against the wrong index.
    let values: Vec<u32> = (0..300_u32)
        .map(|value| value.wrapping_mul(2_654_435_761))
        .collect();
    let encoded = print(values).compressed();

    let header: Vec<u8> = decode_base64url(&encoded).into_iter().take(4).collect();
    assert_eq!(header, [1, 0, 1, 44]);
}

#[test]
fn the_compressed_form_is_url_safe_base64_without_padding() {
    // fpcalc's spelling, and the only one AcoustID accepts in a query string.
    for length in [1, 2, 3, 40, 41] {
        let encoded = print((0..length).map(|value| value * 7).collect()).compressed();
        assert!(
            encoded
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character)),
            "{encoded}"
        );
    }
}

/// A decoder small enough to live here, so the test does not trust the
/// encoder it is checking.
fn decode_base64url(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bits: u32 = 0;
    let mut count = 0;
    let mut bytes = Vec::new();
    for character in text.bytes() {
        let value = u32::try_from(
            ALPHABET
                .iter()
                .position(|candidate| *candidate == character)
                .expect("a base64url character"),
        )
        .expect("an index below 64");
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push(u8::try_from((bits >> count) & 0xFF).expect("a masked byte"));
        }
    }
    bytes
}
