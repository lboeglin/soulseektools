//! Reading audio out of real files.
//!
//! `spectral.rs` covers the classifier on synthesised spectra and `verify.rs`
//! covers the verdicts; this covers the decoder underneath both, which is the
//! part that used to be two subprocesses.
#![allow(clippy::cast_precision_loss)]

mod support;

use stk::audio::decode::{probe_windows, Audio};
use support::{encode, hi_res_flac, Depth, Encoded, Scratch};

#[test]
fn a_stream_reports_its_properties() {
    let scratch = Scratch::new("decode-probe");
    let track = encode(
        &scratch.join("tone.flac"),
        &Encoded {
            seconds: 2.0,
            ..Encoded::default()
        },
    );
    let audio = Audio::open(&track).expect("an opened file");
    let stream = audio.stream();

    // Codec names are spelled the way ffprobe spells them.
    assert_eq!(stream.codec, "flac");
    assert_eq!(stream.sample_rate, 44100);
    assert_eq!(stream.channels, 2);
    assert_eq!(stream.bit_depth, Some(16));
    assert!((stream.nyquist_hz() - 22050.0).abs() < 1e-9);
    assert!((stream.duration_seconds.expect("a duration") - 2.0).abs() < 0.2);
}

#[test]
fn a_container_with_no_decoder_is_reported_as_unsupported() {
    // Opus is taggable here and undecodable, and the two facts have to be
    // distinguishable to a caller.
    let scratch = Scratch::new("decode-unsupported");
    let track = encode(&scratch.join("a.opus"), &Encoded::default());

    let error = Audio::open(&track).err().expect("no decoder");
    assert!(error.to_string().contains("no decoder"), "{error}");
}

#[test]
fn a_window_starts_where_it_was_asked_to() {
    // The regression this guards: a seek that silently landed at the start
    // would still return the right number of samples, and every probe of a
    // track would then describe its opening seconds.
    let scratch = Scratch::new("decode-seek");
    let track = encode(
        &scratch.join("swept.flac"),
        &Encoded {
            seconds: 4.0,
            modulation_hz: 0.25,
            ..Encoded::default()
        },
    );
    let mut audio = Audio::open(&track).expect("an opened file");

    let opening = audio.window(0.0, 1.0).expect("the first window");
    let later = audio.window(2.0, 1.0).expect("a later window");
    let again = audio.window(2.0, 1.0).expect("the same window twice");

    assert_eq!(later, again);
    assert!(rms(&opening) > rms(&later) * 2.0);
}

#[test]
fn a_long_track_is_sampled_at_several_points() {
    let windows = probe_windows(Some(300.0), 20.0, 4);

    assert_eq!(windows.len(), 4);
    assert!(windows[0].0 > 0.0);
    assert!(windows.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

fn rms(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt()
}

#[test]
fn a_16_bit_master_padded_to_24_uses_16_bits() {
    let scratch = Scratch::new("decode-bits-padded");
    let padded = hi_res_flac(&scratch.join("padded.flac"), 44100, None, Depth::PaddedTo24);
    let mut audio = Audio::open(&padded).expect("an open file");
    audio.window(0.0, 1.0).expect("decoded samples");

    assert_eq!(audio.stream().bit_depth, Some(24));
    assert_eq!(audio.bits_used(), Some(16));
}

#[test]
fn a_genuine_24_bit_stream_uses_all_24() {
    let scratch = Scratch::new("decode-bits-genuine");
    let genuine = hi_res_flac(
        &scratch.join("genuine.flac"),
        96000,
        None,
        Depth::TwentyFour,
    );
    let mut audio = Audio::open(&genuine).expect("an open file");
    audio.window(0.0, 1.0).expect("decoded samples");

    assert_eq!(audio.bits_used(), Some(24));
}

#[test]
fn bits_used_waits_for_a_sample_that_is_not_silence() {
    let scratch = Scratch::new("decode-bits-unread");
    let genuine = hi_res_flac(
        &scratch.join("genuine.flac"),
        96000,
        None,
        Depth::TwentyFour,
    );

    assert_eq!(
        Audio::open(&genuine).expect("an open file").bits_used(),
        None
    );
}
