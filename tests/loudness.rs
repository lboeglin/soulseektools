//! The EBU R 128 loudness scan.

mod support;

use std::f64::consts::TAU;
use std::path::Path;

use stk::audio::loudness::{measure_album, measure_samples, Loudness, LoudnessResult};

use stk::config::LoudnessSettings;
use support::{encode, silence, Encoded, Scratch};

fn measure_file(path: &Path, settings: &LoudnessSettings) -> LoudnessResult {
    measure_album(&[path.to_path_buf()], settings)
        .tracks
        .remove(0)
}

const RATE: u32 = 48_000;

/// A stereo 1 kHz tone at a known amplitude, interleaved.
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

/// Each channel's mean square is 0.5, summed to 1.0, so the figure is the -0.691 offset.
#[test]
fn a_known_tone_measures_the_loudness_the_standard_gives_it() {
    let measured = loudness_of(&tone(1.0, 3.0), 2).expect("a measurement");

    assert!(measured.integrated_lufs.abs() < 0.5, "{measured:?}");
}

/// BS.1770 sums channels, so the scan must not be handed a mono downmix.
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

/// The gate is relative to the whole album, so it can't be averaged from tracks.
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
    assert!((album.peak - tracks[0].peak).abs() < 1e-9);
}

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

/// A decode read landing mid-sample would shift every frame after it.
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

    let long = measure("long.flac", 12.0);
    let short = measure("short.flac", 2.0);

    assert!(
        (long.integrated_lufs - short.integrated_lufs).abs() < 0.05,
        "{long:?} against {short:?}"
    );
    assert!((long.peak - short.peak).abs() < 1e-6, "{long:?}");
}
