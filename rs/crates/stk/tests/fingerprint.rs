//! Ported from `py/tests/unit/test_fingerprint.py`.
//!
//! The Python monkeypatches `shutil.which` and `subprocess.run` to exercise
//! backend selection and fpcalc parsing without the binary. Rust has no
//! equivalent, so `parse_fpcalc_output` is exposed and tested directly, and the
//! selection tests assert against what this machine actually has.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    // Several assertions pin an exact sentinel a short-circuit returns -- 0.0 for
    // an empty fingerprint, 0.5 for no evidence -- rather than a computed score.
    clippy::float_cmp
)]

mod support;

use stk::audio::fingerprint::{
    available_kind, envelope_of, fingerprint, fpcalc_path, parse_fpcalc_output, similarity,
    Fingerprint, FingerprintKind, Values, ENVELOPE_SAMPLE_RATE,
};
use support::{encode, Encoded, Scratch};

const DECIBELS: f64 = 10.0;

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

fn envelope(values: Vec<f64>) -> Fingerprint {
    Fingerprint {
        values: Values::Envelope(values),
        duration_seconds: None,
    }
}

fn chromaprint(values: Vec<u32>) -> Fingerprint {
    Fingerprint {
        values: Values::Chromaprint(values),
        duration_seconds: None,
    }
}

fn sine(count: usize, span: f64) -> Vec<f64> {
    (0..count)
        .map(|index| DECIBELS * (span * index as f64 / (count - 1) as f64).sin())
        .collect()
}

fn score(left: &Fingerprint, right: &Fingerprint) -> f64 {
    similarity(left, right).expect("comparable fingerprints")
}

#[test]
fn a_steady_tone_has_a_flat_centred_curve() {
    let samples = vec![0.5_f32; ENVELOPE_SAMPLE_RATE as usize * 4];
    let curve = envelope_of(&samples, ENVELOPE_SAMPLE_RATE);

    assert_eq!(curve.len(), 16);
    assert!(curve.iter().all(|value| value.abs() < 1e-9));
}

#[test]
fn the_curve_follows_the_loudness_over_time() {
    let mut samples = vec![0.01_f32; ENVELOPE_SAMPLE_RATE as usize];
    samples.extend(vec![0.9_f32; ENVELOPE_SAMPLE_RATE as usize]);

    let curve = envelope_of(&samples, ENVELOPE_SAMPLE_RATE);

    assert!(curve[0] < 0.0);
    assert!(curve[curve.len() - 1] > 0.0);
}

#[test]
fn overall_volume_does_not_change_the_curve() {
    // The same rip at two mastering levels must fingerprint identically.
    let mut pattern = vec![0.2_f32; ENVELOPE_SAMPLE_RATE as usize];
    pattern.extend(vec![0.8_f32; ENVELOPE_SAMPLE_RATE as usize]);
    let quieter: Vec<f32> = pattern.iter().map(|sample| sample * 0.25).collect();

    let left = envelope_of(&pattern, ENVELOPE_SAMPLE_RATE);
    let right = envelope_of(&quieter, ENVELOPE_SAMPLE_RATE);

    assert_eq!(left.len(), right.len());
    assert!(left.iter().zip(&right).all(|(a, b)| (a - b).abs() < 1e-6));
}

#[test]
fn audio_shorter_than_one_frame_yields_nothing() {
    assert!(envelope_of(&[0.0; 10], ENVELOPE_SAMPLE_RATE).is_empty());
}

#[test]
fn a_fingerprint_matches_itself() {
    let curve = envelope(sine(40, 12.0));
    assert!((score(&curve, &curve) - 1.0).abs() < 1e-9);
}

#[test]
fn the_same_track_shifted_in_time_still_matches() {
    let curve = sine(60, 12.0);
    let shifted = curve[3..].to_vec();

    assert!(score(&envelope(curve), &envelope(shifted)) > 0.95);
}

#[test]
fn unrelated_curves_do_not_match() {
    // Two deterministic but unrelated shapes, standing in for the Python's
    // seeded normal draws; what matters is that they share no structure.
    let left: Vec<f64> = (0..60)
        .map(|index| DECIBELS * f64::from(index).sin())
        .collect();
    let right: Vec<f64> = (0..60)
        .map(|index| DECIBELS * (f64::from(index) * 2.7 + 1.3).cos())
        .collect();

    assert!(score(&envelope(left), &envelope(right)) < 0.9);
}

#[test]
fn an_empty_fingerprint_matches_nothing() {
    assert_eq!(
        score(&envelope(Vec::new()), &envelope(vec![1.0, 2.0, 3.0])),
        0.0
    );
}

#[test]
fn fingerprints_of_different_kinds_are_never_compared() {
    let error = similarity(&envelope(vec![1.0; 20]), &chromaprint(vec![1; 20]))
        .expect_err("mismatched kinds");
    assert!(error.to_string().contains("cannot compare"), "{error}");
}

#[test]
fn a_featureless_curve_yields_no_evidence_either_way() {
    let flat = envelope(vec![0.0; 40]);
    let varied = envelope(sine(40, 12.0));

    assert!((score(&flat, &flat) - 0.5).abs() < 1e-9);
    assert_eq!(score(&flat, &varied), 0.0);
}

#[test]
fn too_few_frames_to_compare_scores_zero() {
    let short = envelope(vec![0.0, DECIBELS, -DECIBELS]);
    assert_eq!(score(&short, &short), 0.0);
}

#[test]
fn identical_chromaprints_match() {
    let values: Vec<u32> = (1000..1030).collect();
    let fingerprint = chromaprint(values);

    assert!((score(&fingerprint, &fingerprint) - 1.0).abs() < 1e-9);
}

#[test]
fn chromaprints_differing_in_every_bit_do_not_match() {
    let values = chromaprint(vec![0; 30]);
    let inverted = chromaprint(vec![0xFFFF_FFFF; 30]);

    assert!(score(&values, &inverted) < 0.1);
}

#[test]
fn a_chromaprint_with_a_few_altered_bits_still_matches() {
    let values: Vec<u32> = (2000..2040).collect();
    let altered: Vec<u32> = values.iter().map(|value| value ^ 0b1).collect();

    assert!(score(&chromaprint(values), &chromaprint(altered)) > 0.95);
}

#[test]
fn the_available_backend_follows_whether_fpcalc_is_installed() {
    let expected = if fpcalc_path().is_some() {
        FingerprintKind::Chromaprint
    } else {
        FingerprintKind::Envelope
    };
    assert_eq!(available_kind(), expected);
}

#[test]
fn asking_for_chromaprint_without_fpcalc_is_an_explicit_error() {
    if fpcalc_path().is_some() {
        eprintln!("skipping: fpcalc is installed, so there is no absence to report");
        return;
    }

    let error = fingerprint(
        std::path::Path::new("track.flac"),
        Some(FingerprintKind::Chromaprint),
    )
    .expect_err("no fpcalc");
    assert!(error.to_string().contains("fpcalc"), "{error}");
}

#[test]
fn the_chromaprint_backend_parses_fpcalc_output() {
    let payload = br#"{"duration": 212.5, "fingerprint": [1, 2, 3]}"#;
    let result = parse_fpcalc_output(payload).expect("parsed output");

    assert_eq!(result.kind(), FingerprintKind::Chromaprint);
    assert_eq!(result.values, Values::Chromaprint(vec![1, 2, 3]));
    assert!((result.duration_seconds.expect("a duration") - 212.5).abs() < 1e-9);
}

#[test]
fn unparseable_fpcalc_output_is_reported_as_a_tool_error() {
    let error = parse_fpcalc_output(b"bad file").expect_err("unparseable output");
    assert!(error.to_string().contains("no usable JSON"), "{error}");
}

#[test]
fn the_same_audio_in_two_containers_fingerprints_alike() {
    needs_ffmpeg!();
    // The pairing the duplicate scan exists for: one track, ripped twice, as
    // lossless and as lossy, sharing no metadata and differing in file size.
    let scratch = Scratch::new("fp-containers");
    let options = Encoded {
        seconds: 8.0,
        frequency: 300,
        modulation_hz: 0.7,
        ..Encoded::default()
    };
    let lossless = encode(&scratch.join("a.flac"), &options);
    let lossy = encode(&scratch.join("a.mp3"), &options);

    let left = fingerprint(&lossless, Some(FingerprintKind::Envelope)).expect("a fingerprint");
    let right = fingerprint(&lossy, Some(FingerprintKind::Envelope)).expect("a fingerprint");

    assert!(score(&left, &right) > 0.9);
}

#[test]
fn two_different_tracks_do_not_fingerprint_alike() {
    needs_ffmpeg!();
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
            frequency: 300,
            modulation_hz: 2.9,
            ..Encoded::default()
        },
    );

    let left = fingerprint(&first, Some(FingerprintKind::Envelope)).expect("a fingerprint");
    let right = fingerprint(&second, Some(FingerprintKind::Envelope)).expect("a fingerprint");

    assert!(score(&left, &right) < 0.9);
}

#[test]
fn featureless_audio_reports_no_evidence_either_way() {
    needs_ffmpeg!();
    // A sustained tone has no loudness shape to correlate. The score must be too
    // low to group files by and too high to claim they differ.
    let scratch = Scratch::new("fp-featureless");
    let tone = encode(
        &scratch.join("tone.flac"),
        &Encoded {
            seconds: 8.0,
            ..Encoded::default()
        },
    );
    let music = encode(
        &scratch.join("music.flac"),
        &Encoded {
            seconds: 8.0,
            modulation_hz: 0.7,
            ..Encoded::default()
        },
    );

    let steady = fingerprint(&tone, Some(FingerprintKind::Envelope)).expect("a fingerprint");
    let dynamic = fingerprint(&music, Some(FingerprintKind::Envelope)).expect("a fingerprint");

    assert!((score(&steady, &steady) - 0.5).abs() < 1e-9);
    assert_eq!(score(&steady, &dynamic), 0.0);
}

#[test]
fn a_fingerprint_records_the_duration() {
    needs_ffmpeg!();
    let scratch = Scratch::new("fp-duration");
    let track = encode(
        &scratch.join("a.flac"),
        &Encoded {
            seconds: 5.0,
            ..Encoded::default()
        },
    );

    let result = fingerprint(&track, Some(FingerprintKind::Envelope)).expect("a fingerprint");
    assert!((result.duration_seconds.expect("a duration") - 5.0).abs() < 0.2);
}
