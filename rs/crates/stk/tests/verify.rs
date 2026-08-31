//! End-to-end checks of the transcode detector against real encoded files.
//!
//! `spectral.rs` covers the classifier on synthesised spectra; this covers the
//! path through ffprobe and ffmpeg, including a genuine MP3 re-encoded as FLAC
//! -- the exact deception the check exists to catch.
//!
//! Ported from `py/tests/unit/test_verify.py`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    // Several assertions pin an exact sentinel a short-circuit returns -- 0.0 for
    // an empty fingerprint, 0.5 for no evidence -- rather than a computed score.
    clippy::float_cmp
)]

mod support;

use stk::audio::ffmpeg;
use stk::audio::verify::verify_file;
use stk::config::VerifySettings;
use stk::models::Verdict;
use support::{noise_source, silence, transcode_to_flac, Scratch};

/// Rust has no runtime skip; a machine without ffmpeg says so and moves on,
/// while CI installs it so the coverage is not quietly lost.
macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

fn source(scratch: &Scratch) -> std::path::PathBuf {
    noise_source(&scratch.join("source.flac"), 8, 44100)
}

#[test]
fn genuine_lossless_audio_passes() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-genuine");
    let result = verify_file(&source(&scratch), &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Ok, "{}", result.detail);
    assert_eq!(result.codec, "flac");
    assert_eq!(result.sample_rate, Some(44100));
}

#[test]
fn an_mp3_re_encoded_as_flac_is_caught() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-transcode");
    let source = source(&scratch);

    for bitrate in ["128k", "192k", "320k"] {
        let faked = transcode_to_flac(&source, &scratch, &format!("fake{bitrate}"), bitrate);
        let result = verify_file(&faked, &VerifySettings::default());

        assert_eq!(
            result.verdict,
            Verdict::Fake,
            "{bitrate}: {}",
            result.detail
        );
        assert!(result.cutoff_hz.is_some(), "{bitrate}");
        assert!(result.cutoff_hz < result.nyquist_hz, "{bitrate}");
    }
}

#[test]
fn the_reported_cutoff_rises_with_the_source_bitrate() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-cutoff-rises");
    let source = source(&scratch);

    let low = verify_file(
        &transcode_to_flac(&source, &scratch, "low", "128k"),
        &VerifySettings::default(),
    );
    let high = verify_file(
        &transcode_to_flac(&source, &scratch, "high", "320k"),
        &VerifySettings::default(),
    );

    assert!(low.cutoff_hz < high.cutoff_hz);
}

#[test]
fn the_verdict_explains_itself() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-explains");
    let source = source(&scratch);
    let result = verify_file(
        &transcode_to_flac(&source, &scratch, "explained", "128k"),
        &VerifySettings::default(),
    );

    assert!(result.detail.contains("transcoded"), "{}", result.detail);
    assert!(result.steepness_db_per_khz.is_some());
}

#[test]
fn a_silent_file_is_reported_as_unanalysable_not_as_lossless() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-silent");
    let silent = silence(&scratch.join("silent.flac"), 5, 44100);

    let result = verify_file(&silent, &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Error);
    assert!(
        result.detail.contains("no audible content"),
        "{}",
        result.detail
    );
}

#[test]
fn an_unreadable_file_is_reported_rather_than_raised() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-broken");
    let broken = scratch.join("broken.flac");
    std::fs::write(&broken, b"not audio at all").expect("a written file");

    let result = verify_file(&broken, &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Error);
    assert_eq!(result.path, broken);
}

#[test]
fn a_stricter_threshold_can_flag_more() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-strict");
    let strict = verify_file(
        &source(&scratch),
        &VerifySettings {
            steep_db_per_khz: 0.1,
            ..VerifySettings::default()
        },
    );

    // Content still reaches Nyquist, so the near-Nyquist branch wins first.
    assert_eq!(strict.verdict, Verdict::Ok, "{}", strict.detail);
}

#[test]
fn probe_settings_are_honoured() {
    needs_ffmpeg!();
    let scratch = Scratch::new("verify-probes");
    let result = verify_file(
        &source(&scratch),
        &VerifySettings {
            probe_seconds: 2.0,
            probe_count: 1,
            ..VerifySettings::default()
        },
    );

    assert_eq!(result.verdict, Verdict::Ok, "{}", result.detail);
}

#[test]
fn probe_reports_the_stream_properties() {
    needs_ffmpeg!();
    let scratch = Scratch::new("ffmpeg-probe");
    let stream = ffmpeg::probe(&source(&scratch)).expect("a probed stream");

    assert_eq!(stream.sample_rate, 44100);
    assert_eq!(stream.codec, "flac");
    assert!((stream.nyquist_hz() - 22050.0).abs() < 1e-9);
    assert!((stream.duration_seconds.expect("a duration") - 8.0).abs() < 0.2);
}

#[test]
fn probing_a_non_audio_file_is_an_explicit_error() {
    needs_ffmpeg!();
    let scratch = Scratch::new("ffmpeg-notes");
    let path = scratch.join("notes.txt");
    std::fs::write(&path, "hello").expect("a written file");

    assert!(ffmpeg::probe(&path).is_err());
}

#[test]
fn decode_returns_mono_float_samples() {
    needs_ffmpeg!();
    let scratch = Scratch::new("ffmpeg-decode");
    let samples = ffmpeg::decode(&source(&scratch), 8000, 0.0, Some(1.0)).expect("decoded samples");

    let expected = 8000.0;
    assert!(
        (samples.len() as f64 - expected).abs() / expected < 0.05,
        "{} samples",
        samples.len()
    );
    assert!(samples.iter().all(|sample| (-1.0..=1.0).contains(sample)));
}

#[test]
fn decode_probes_samples_several_windows_of_a_long_track() {
    needs_ffmpeg!();
    let scratch = Scratch::new("ffmpeg-windows");
    let chunks =
        ffmpeg::decode_probes(&source(&scratch), 8000, Some(8.0), 1.0, 4).expect("decoded windows");

    assert_eq!(chunks.len(), 4);
}

#[test]
fn decode_probes_reads_a_short_track_in_one_window() {
    needs_ffmpeg!();
    let scratch = Scratch::new("ffmpeg-one-window");
    let chunks =
        ffmpeg::decode_probes(&source(&scratch), 8000, Some(8.0), 20.0, 4).expect("decoded window");

    assert_eq!(chunks.len(), 1);
}

#[test]
fn requiring_a_missing_tool_explains_what_to_install() {
    let error = ffmpeg::require("stk-no-such-tool").expect_err("a missing tool");
    assert!(error.to_string().contains("install ffmpeg"), "{error}");
}

#[test]
fn render_spectrogram_writes_an_image() {
    needs_ffmpeg!();
    let scratch = Scratch::new("ffmpeg-spectrogram");
    let destination = ffmpeg::render_spectrogram(&source(&scratch), &scratch.join("spectrum.png"))
        .expect("a rendered spectrogram");

    assert!(destination.is_file());
    assert!(destination.metadata().expect("metadata").len() > 0);
}
