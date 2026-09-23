//! End-to-end checks of the transcode detector against real encoded files.
//!
//! `spectral.rs` covers the classifier on synthesised spectra and `decode.rs`
//! covers the decoder; this covers the verdict the two produce together,
//! including for a genuine MP3 re-encoded as FLAC -- the exact deception the
//! check exists to catch.
//!
//! Ported from `py/tests/unit/test_verify.py`.

mod support;

use stk::audio::verify::verify_file;
use stk::config::VerifySettings;
use stk::models::Verdict;
use support::{committed, noise_source, silence, transcode_to_flac, Scratch};

fn source(scratch: &Scratch) -> std::path::PathBuf {
    noise_source(&scratch.join("source.flac"), 8, 44100)
}

#[test]
fn genuine_lossless_audio_passes() {
    let scratch = Scratch::new("verify-genuine");
    let result = verify_file(&source(&scratch), &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Ok, "{}", result.detail);
    assert_eq!(result.codec, "flac");
    assert_eq!(result.sample_rate, Some(44100));
}

#[test]
fn an_mp3_re_encoded_as_flac_is_caught() {
    let scratch = Scratch::new("verify-transcode");
    for bitrate in ["128k", "192k", "320k"] {
        let faked = transcode_to_flac(&scratch, &format!("fake{bitrate}"), bitrate);
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
    let scratch = Scratch::new("verify-cutoff-rises");
    let low = verify_file(
        &transcode_to_flac(&scratch, "low", "128k"),
        &VerifySettings::default(),
    );
    let high = verify_file(
        &transcode_to_flac(&scratch, "high", "320k"),
        &VerifySettings::default(),
    );

    assert!(low.cutoff_hz < high.cutoff_hz);
}

#[test]
fn the_verdict_explains_itself() {
    let scratch = Scratch::new("verify-explains");
    let result = verify_file(
        &transcode_to_flac(&scratch, "explained", "128k"),
        &VerifySettings::default(),
    );

    assert!(result.detail.contains("transcoded"), "{}", result.detail);
    assert!(result.steepness_db_per_khz.is_some());
}

#[test]
fn a_silent_file_is_reported_as_unanalysable_not_as_lossless() {
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
    let scratch = Scratch::new("verify-broken");
    let broken = scratch.join("broken.flac");
    std::fs::write(&broken, b"not audio at all").expect("a written file");

    let result = verify_file(&broken, &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Error);
    assert_eq!(result.path, broken);
}

#[test]
fn a_stricter_threshold_can_flag_more() {
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
fn a_genuine_mp3_is_held_to_its_bitrate_and_passes() {
    for name in [
        "noise-128k.mp3",
        "noise-192k.mp3",
        "noise-320k.mp3",
        "noise-v0.mp3",
    ] {
        let result = verify_file(&committed(name), &VerifySettings::default());

        assert_eq!(result.verdict, Verdict::Ok, "{name}: {}", result.detail);
        assert_eq!(result.codec, "mp3");
    }
}

#[test]
fn a_128kbps_mp3_re_encoded_at_320_is_caught() {
    let result = verify_file(
        &committed("noise-128k-as-320k.mp3"),
        &VerifySettings::default(),
    );

    assert_eq!(result.verdict, Verdict::Upsampled, "{}", result.detail);
    assert!(
        result.detail.contains("~128kbps source"),
        "{}",
        result.detail
    );
}

#[test]
fn a_128kbps_mp3_re_encoded_as_vbr_is_caught_despite_the_wider_tolerance() {
    let result = verify_file(
        &committed("noise-128k-as-vbr320.mp3"),
        &VerifySettings::default(),
    );

    assert_eq!(result.verdict, Verdict::Upsampled, "{}", result.detail);
    assert!(result.detail.contains("VBR"), "{}", result.detail);
}

#[test]
fn an_mp3_is_not_judged_as_a_lossless_transcode() {
    // The lossless check would call a 128 kbps wall FAKE; an MP3 claiming
    // 128 kbps is exactly what it says it is.
    let result = verify_file(&committed("noise-128k.mp3"), &VerifySettings::default());

    assert!(
        result.detail.contains("128kbps encode"),
        "{}",
        result.detail
    );
}
