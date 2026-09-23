//! Reading audio out of real files.
//!
//! `spectral.rs` covers the classifier on synthesised spectra and `verify.rs`
//! covers the verdicts; this covers the decoder underneath both, which is the
//! part that used to be two subprocesses.
#![allow(clippy::cast_precision_loss)]

mod support;

use stk::audio::decode::{probe_windows, Audio};
use support::{committed, encode, hi_res_flac, noise_source, Depth, Encoded, Scratch};

fn source(scratch: &Scratch) -> std::path::PathBuf {
    noise_source(&scratch.join("source.flac"), 8, 44100)
}

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

    // The spellings are ffprobe's, because `oracle/verify.csv` carries them.
    assert_eq!(stream.codec, "flac");
    assert_eq!(stream.sample_rate, 44100);
    assert_eq!(stream.channels, 2);
    assert_eq!(stream.bit_depth, Some(16));
    assert!((stream.nyquist_hz() - 22050.0).abs() < 1e-9);
    assert!((stream.duration_seconds.expect("a duration") - 2.0).abs() < 0.2);
}

#[test]
fn opening_a_file_that_is_not_audio_is_an_explicit_error() {
    let scratch = Scratch::new("decode-notes");
    let path = scratch.join("notes.txt");
    std::fs::write(&path, "hello").expect("a written file");

    assert!(Audio::open(&path).is_err());
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
fn a_window_is_mono_at_the_files_own_rate() {
    let scratch = Scratch::new("decode-window");
    let mut audio = Audio::open(&source(&scratch)).expect("an opened file");
    let samples = audio.window(0.0, 1.0).expect("decoded samples");

    assert_eq!(samples.len(), 44100);
    assert!(samples.iter().all(|sample| (-1.0..=1.0).contains(sample)));
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
fn frames_arrive_at_the_files_own_channel_count() {
    let scratch = Scratch::new("decode-frames");
    let mut audio = Audio::open(&source(&scratch)).expect("an opened file");
    let channels = audio.stream().channels as usize;

    let mut samples = 0_usize;
    audio
        .frames(&mut |frames: &[f32]| {
            samples += frames.len();
            Ok(())
        })
        .expect("a full decode");

    let expected = 8.0 * 44100.0 * channels as f64;
    assert!(
        (samples as f64 - expected).abs() / expected < 0.05,
        "{samples} samples"
    );
}

#[test]
fn a_long_track_is_sampled_at_several_points() {
    let windows = probe_windows(Some(300.0), 20.0, 4);

    assert_eq!(windows.len(), 4);
    assert!(windows[0].0 > 0.0);
    assert!(windows.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[test]
fn a_track_too_short_to_spread_probes_over_is_read_once() {
    let windows = probe_windows(Some(8.0), 20.0, 4);

    assert_eq!(windows.len(), 1);
    assert!((windows[0].0 - 0.0).abs() < f64::EPSILON);
}

#[test]
fn a_track_of_unknown_length_is_read_once_from_the_start() {
    assert_eq!(probe_windows(None, 20.0, 4), vec![(0.0, 20.0)]);
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
fn bits_used_says_nothing_about_a_16_bit_or_lossy_stream() {
    let scratch = Scratch::new("decode-bits-none");
    let mut cd =
        Audio::open(&noise_source(&scratch.join("cd.flac"), 2, 44100)).expect("an open file");
    cd.window(0.0, 1.0).expect("decoded samples");
    let mut mp3 = Audio::open(&committed("noise-320k.mp3")).expect("an open file");
    mp3.window(0.0, 1.0).expect("decoded samples");

    assert_eq!(cd.bits_used(), None);
    assert_eq!(mp3.bits_used(), None);
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
