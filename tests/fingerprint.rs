//! Chromaprint fingerprints: comparison on literal values, computation on real files.

mod support;

use stk::audio::fingerprint::{fingerprint, similarity, Fingerprint};
use support::{committed, encode, Encoded, Scratch};

fn print(values: Vec<u32>) -> Fingerprint {
    Fingerprint {
        values,
        duration_seconds: None,
    }
}

/// Not a shift of another sequence, which the alignment search would rightly match.
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
fn the_same_track_shifted_in_time_still_matches() {
    let values: Vec<u32> = (1000..1060).collect();
    let shifted = values[3..].to_vec();

    assert!(similarity(&print(values), &print(shifted)) > 0.95);
}

#[test]
fn unrelated_fingerprints_do_not_match() {
    assert!(similarity(&unrelated(1, 60), &unrelated(2, 60)) < 0.9);
}

#[test]
fn the_same_audio_in_two_containers_fingerprints_alike() {
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
fn a_container_with_no_decoder_says_so_rather_than_scoring_zero() {
    // An empty print for an undecodable file would silently read as "not a duplicate".
    let scratch = Scratch::new("fp-undecodable");
    let track = encode(&scratch.join("a.opus"), &Encoded::default());

    let error = fingerprint(&track).expect_err("no decoder");
    assert!(error.to_string().contains("no decoder"), "{error}");
}

#[test]
fn the_compressed_form_names_the_algorithm_and_the_length() {
    // AcoustID reads the algorithm and length from the header, so it must match.
    let values: Vec<u32> = (0..300_u32)
        .map(|value| value.wrapping_mul(2_654_435_761))
        .collect();
    let encoded = print(values).compressed();

    let header: Vec<u8> = decode_base64url(&encoded).into_iter().take(4).collect();
    assert_eq!(header, [1, 0, 1, 44]);
}

/// Its own decoder, so the test doesn't trust the encoder it checks.
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
