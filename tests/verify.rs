//! The transcode detector end to end, on real encoded files.

mod support;

use stk::audio::verify::verify_file;
use stk::config::VerifySettings;
use stk::library::scan::claims_lossless;
use stk::models::Verdict;
use support::{committed, hi_res_flac, noise_source, silence, transcode_to_flac, Depth, Scratch};

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

/// ffmpeg's default resampler fades through 22.05 kHz, which the ordinary floor calls a roll-off.
#[test]
fn a_hi_res_file_made_by_ffmpegs_default_resampler_is_caught() {
    let result = verify_file(
        &committed("noise-44k-as-96k.flac"),
        &VerifySettings::default(),
    );

    assert_eq!(result.verdict, Verdict::Upsampled, "{}", result.detail);
    assert!(result.detail.contains("44100Hz"), "{}", result.detail);
}

#[test]
fn a_cd_rate_file_padded_to_24_bits_is_caught_by_its_depth_alone() {
    let scratch = Scratch::new("verify-padded");
    let padded = hi_res_flac(&scratch.join("padded.flac"), 44100, None, Depth::PaddedTo24);

    let result = verify_file(&padded, &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Upsampled, "{}", result.detail);
    assert_eq!(
        result.detail,
        "24-bit container holding 16-bit audio -- padded, not hi-res"
    );
}

#[test]
fn genuine_hi_res_audio_passes() {
    let scratch = Scratch::new("verify-hi-res-genuine");
    let genuine = hi_res_flac(
        &scratch.join("genuine.flac"),
        96000,
        None,
        Depth::TwentyFour,
    );

    let result = verify_file(&genuine, &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Ok, "{}", result.detail);
    assert_eq!(result.bit_depth, Some(24));
}

#[test]
fn a_transcode_padded_to_24_bits_stays_a_transcode() {
    let scratch = Scratch::new("verify-padded-transcode");
    let fake = hi_res_flac(
        &scratch.join("fake.flac"),
        44100,
        Some(16000.0),
        Depth::PaddedTo24,
    );

    let result = verify_file(&fake, &VerifySettings::default());

    assert_eq!(result.verdict, Verdict::Fake, "{}", result.detail);
    assert!(result.detail.contains("transcoded"), "{}", result.detail);
}

#[test]
fn an_m4a_claims_lossless_only_when_it_holds_alac() {
    assert!(claims_lossless(&committed("tone-alac.m4a")));
    assert!(!claims_lossless(&committed("tone.m4a")));
    assert!(claims_lossless(&committed("pair.flac")));
    assert!(!claims_lossless(&committed("tone.mp3")));
}
