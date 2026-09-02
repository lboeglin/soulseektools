//! The EBU R 128 loudness scan behind `stk loudness` (#7).
//!
//! The analysis is a pure function over samples, so most of what a figure means
//! is established here without decoding anything -- which is also what #9 wants
//! of it, since nothing in this file needs a tool installed to run.

mod support;

use std::f64::consts::TAU;

use stk::audio::loudness::{measure_album, measure_file, measure_samples, Loudness};
use stk::config::LoudnessSettings;
use support::{encode, silence, Encoded, Scratch};

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

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

#[test]
fn a_quieter_signal_measures_quieter_by_exactly_as_much() {
    let loud = loudness_of(&tone(0.5, 3.0), 2).expect("a measurement");
    let quiet = loudness_of(&tone(0.25, 3.0), 2).expect("a measurement");

    assert!(
        (loud.integrated_lufs - quiet.integrated_lufs - 6.0206).abs() < 0.01,
        "{loud:?} against {quiet:?}"
    );
}

#[test]
fn a_peak_comes_back_as_a_linear_amplitude() {
    let measured = loudness_of(&tone(0.5, 1.0), 2).expect("a measurement");

    assert!((measured.peak - 0.5).abs() < 0.01, "{measured:?}");
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

/// Digital silence gates away to nothing. Read as a number it would be a gain
/// asking a player to raise silence infinitely.
#[test]
fn digital_silence_has_no_measurable_loudness() {
    assert_eq!(loudness_of(&vec![0.0; RATE as usize * 2 * 3], 2), None);
}

#[test]
fn a_measurement_reads_back_from_a_real_file() {
    needs_ffmpeg!();
    let scratch = Scratch::new("loudness-real");
    let path = encode(
        &scratch.join("tone.flac"),
        &Encoded {
            seconds: 2.0,
            ..Encoded::default()
        },
    );

    let result = measure_file(&path, &LoudnessSettings::default());

    let loudness = result.loudness.expect("a measurement");
    assert!(
        loudness.integrated_lufs < 0.0 && loudness.integrated_lufs > -70.0,
        "{loudness:?}"
    );
    assert!(loudness.peak > 0.0 && loudness.peak <= 1.0, "{loudness:?}");
}

#[test]
fn a_file_six_db_quieter_is_measured_six_db_quieter() {
    needs_ffmpeg!();
    let scratch = Scratch::new("loudness-relative");
    let loud = encode(
        &scratch.join("loud.flac"),
        &Encoded {
            seconds: 2.0,
            modulation_hz: 0.5,
            ..Encoded::default()
        },
    );
    let quiet = scratch.join("quiet.flac");
    support::run_ffmpeg(&[
        "-i",
        &loud.display().to_string(),
        "-filter:a",
        "volume=-6dB",
        "-c:a",
        "flac",
        &quiet.display().to_string(),
    ]);

    let settings = LoudnessSettings::default();
    let first = measure_file(&loud, &settings)
        .loudness
        .expect("the loud one");
    let second = measure_file(&quiet, &settings)
        .loudness
        .expect("the quiet one");

    assert!(
        (first.integrated_lufs - second.integrated_lufs - 6.0).abs() < 0.1,
        "{first:?} against {second:?}"
    );
}

#[test]
fn a_file_of_digital_silence_is_reported_rather_than_measured() {
    needs_ffmpeg!();
    let scratch = Scratch::new("loudness-silence");
    let path = silence(&scratch.join("quiet.flac"), 2, 44100);

    let result = measure_file(&path, &LoudnessSettings::default());

    assert_eq!(result.loudness, None);
    assert!(!result.detail.is_empty());
}

/// One unreadable file must not abort a library-wide scan.
#[test]
fn a_file_that_cannot_be_read_carries_its_reason_rather_than_failing() {
    let scratch = Scratch::new("loudness-broken");
    let path = scratch.join("broken.flac");
    std::fs::write(&path, b"not audio").expect("a written file");

    let result = measure_file(&path, &LoudnessSettings::default());

    assert_eq!(result.loudness, None);
    assert!(!result.detail.is_empty());
}

/// An album's gate is the album's, not each track's, so a record is scanned as
/// one programme rather than averaged from the figures its tracks got.
#[test]
fn an_album_is_measured_across_its_tracks_at_once() {
    needs_ffmpeg!();
    let scratch = Scratch::new("loudness-album");
    let loud = encode(
        &scratch.join("loud.flac"),
        &Encoded {
            seconds: 2.0,
            modulation_hz: 0.5,
            ..Encoded::default()
        },
    );
    let quiet = scratch.join("quiet.flac");
    support::run_ffmpeg(&[
        "-i",
        &loud.display().to_string(),
        "-filter:a",
        "volume=-12dB",
        "-c:a",
        "flac",
        &quiet.display().to_string(),
    ]);

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

#[test]
fn tracks_come_back_in_the_order_the_paths_were_given() {
    needs_ffmpeg!();
    let scratch = Scratch::new("loudness-order");
    let paths: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|name| {
            encode(
                &scratch.join(&format!("{name}.flac")),
                &Encoded {
                    seconds: 1.0,
                    ..Encoded::default()
                },
            )
        })
        .collect();

    let scan = measure_album(&paths, &LoudnessSettings::default());

    assert_eq!(
        scan.tracks
            .iter()
            .map(|result| result.path.clone())
            .collect::<Vec<_>>(),
        paths
    );
}

/// A record nothing in which could be read has no figure to give its tracks.
#[test]
fn an_album_of_unreadable_files_reports_no_album_loudness() {
    let scratch = Scratch::new("loudness-album-broken");
    let path = scratch.join("broken.flac");
    std::fs::write(&path, b"not audio").expect("a written file");

    let scan = measure_album(&[path], &LoudnessSettings::default());

    assert_eq!(scan.album, None);
    assert_eq!(scan.tracks.len(), 1);
}
