//! The EBU R 128 loudness scan behind `stk loudness`.

mod support;

use std::f64::consts::TAU;

use stk::audio::loudness::{measure_album, measure_file, measure_samples, Loudness};
use stk::config::LoudnessSettings;
use support::{encode, silence, Encoded, Scratch};

const RATE: u32 = 48_000;

/// A stereo 1 kHz tone at a known amplitude, interleaved as the analyser wants.
///
/// Every cast here is bounded by a few seconds of audio at a known rate, not by
/// an address space, and the samples are `f32` because that is what the
/// analyser takes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn tone(amplitude: f64, seconds: f64) -> Vec<f32> {
    let frames = (f64::from(RATE) * seconds) as usize;
    (0..frames)
        .flat_map(|frame| {
            let value = (amplitude * (TAU * 1000.0 * frame as f64 / f64::from(RATE)).sin()) as f32;
            [value, value]
        })
        .collect()
}

fn loudness_of(samples: &[f32], channels: u32) -> Option<Loudness> {
    let state = measure_samples(samples, RATE, channels, true).expect("an analyser");
    let integrated = state.loudness_global().expect("a global loudness");
    integrated.is_finite().then(|| Loudness {
        integrated_lufs: integrated,
        peak: (0..channels)
            .filter_map(|channel| state.true_peak(channel).ok())
            .fold(0.0_f64, f64::max),
    })
}

/// A full-scale stereo 1 kHz tone measures about 0 LUFS. Each channel's mean
/// square is 0.5 and BS.1770 sums them under unit weights, so the energy is
/// 1.0 and the whole figure is the -0.691 offset against the K-weighting's
/// small lift near 1 kHz.
#[test]
fn a_known_tone_measures_the_loudness_the_standard_gives_it() {
    let measured = loudness_of(&tone(1.0, 3.0), 2).expect("a measurement");

    assert!(measured.integrated_lufs.abs() < 0.5, "{measured:?}");
}

/// The whole reason the scan is not handed a mono downmix: BS.1770 sums the
/// channels under fixed weights, so the same signal in two channels is louder
/// than in one.
#[test]
fn the_same_signal_in_two_channels_is_louder_than_in_one() {
    let stereo = loudness_of(&tone(0.5, 3.0), 2).expect("a measurement");
    let mono: Vec<f32> = tone(0.5, 3.0).iter().step_by(2).copied().collect();
    let mono = loudness_of(&mono, 1).expect("a measurement");

    assert!(
        (stereo.integrated_lufs - mono.integrated_lufs - 3.01).abs() < 0.01,
        "{stereo:?} against {mono:?}"
    );
}

#[test]
fn a_file_of_digital_silence_is_reported_rather_than_measured() {
    let scratch = Scratch::new("loudness-silence");
    let path = silence(&scratch.join("quiet.flac"), 2, 44100);

    let result = measure_file(&path, &LoudnessSettings::default());

    assert_eq!(result.loudness, None);
    assert!(!result.detail.is_empty());
}

/// An album's gate is the album's, not each track's, so a record is scanned as
/// one programme rather than averaged from the figures its tracks got.
#[test]
fn an_album_is_measured_across_its_tracks_at_once() {
    let scratch = Scratch::new("loudness-album");
    let loud = encode(
        &scratch.join("loud.flac"),
        &Encoded {
            seconds: 2.0,
            modulation_hz: 0.5,
            ..Encoded::default()
        },
    );
    let quiet = encode(
        &scratch.join("quiet.flac"),
        &Encoded {
            seconds: 2.0,
            modulation_hz: 0.5,
            // -12dB, as an amplitude.
            gain: 0.251_188_65,
            ..Encoded::default()
        },
    );

    let scan = measure_album(&[loud, quiet], &LoudnessSettings::default());

    let album = scan.album.expect("an album loudness");
    let tracks: Vec<Loudness> = scan
        .tracks
        .iter()
        .map(|result| result.loudness.expect("a track measurement"))
        .collect();
    assert!(
        album.integrated_lufs < tracks[0].integrated_lufs
            && album.integrated_lufs > tracks[1].integrated_lufs,
        "{album:?} against {tracks:?}"
    );
    // The peak belongs to whichever track holds it.
    assert!((album.peak - tracks[0].peak).abs() < 1e-9);
}

/// A gate is relative to the programme it is applied to, so a figure taken from
/// nine tracks of a twelve-track record describes nine tracks. Writing it to
/// those nine as their album gain would say otherwise.
#[test]
fn a_record_with_a_track_it_could_not_read_has_no_album_figure() {
    let scratch = Scratch::new("loudness-album-partial");
    let readable = encode(
        &scratch.join("a.flac"),
        &Encoded {
            seconds: 1.0,
            modulation_hz: 0.5,
            ..Encoded::default()
        },
    );
    let broken = scratch.join("b.flac");
    std::fs::write(&broken, b"not audio").expect("a written file");

    let scan = measure_album(&[readable, broken], &LoudnessSettings::default());

    assert_eq!(scan.album, None);
    assert!(scan.tracks[0].loudness.is_some());
    assert!(scan.tracks[1].loudness.is_none());
}

/// The decode is handed to the analyser a block at a time, and a 64 KiB read
/// lands mid-sample far more often than not. A boundary mishandled would shift
/// every frame after it, so a steady tone long enough to cross dozens of them
/// has to measure exactly what a short one of the same tone does.
#[test]
fn a_long_track_measures_what_a_short_one_of_the_same_tone_does() {
    let scratch = Scratch::new("loudness-blocks");
    let settings = LoudnessSettings::default();
    let measure = |name: &str, seconds: f64| {
        let path = encode(
            &scratch.join(name),
            &Encoded {
                seconds,
                sample_rate: RATE,
                ..Encoded::default()
            },
        );
        measure_file(&path, &settings)
            .loudness
            .expect("a measurement")
    };

    // 12 s of 48 kHz stereo `f32` is 4.6 MB, so about seventy blocks.
    let long = measure("long.flac", 12.0);
    let short = measure("short.flac", 2.0);

    assert!(
        (long.integrated_lufs - short.integrated_lufs).abs() < 0.05,
        "{long:?} against {short:?}"
    );
    assert!((long.peak - short.peak).abs() < 1e-6, "{long:?}");
}
