//! Reading ffmpeg's `ebur128` summary, and measuring real files with it (#7).

mod support;

use stk::audio::loudness::{measure_file, measure_files, parse_summary};
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

/// A summary as ffmpeg 8.1 prints it, both peak modes enabled.
const SUMMARY: &str = "\
[Parsed_ebur128_0 @ 0000] t: 2.999977  TARGET:-23 LUFS  M: -21.1 S: -21.1  I: -21.1 LUFS  \
LRA: 20.0 LU  SPK: -18.1 dBFS  FTPK: -18.1 dBFS  TPK: -18.1 dBFS
[Parsed_ebur128_0 @ 0000] Summary:

  Integrated loudness:
    I:         -21.1 LUFS
    Threshold: -31.1 LUFS

  Loudness range:
    LRA:        20.0 LU
    Threshold: -41.1 LUFS
    LRA low:   -41.1 LUFS
    LRA high:  -21.1 LUFS

  Sample peak:
    Peak:      -18.1 dBFS

  True peak:
    Peak:       -3.4 dBFS
";

#[test]
fn a_summary_yields_the_integrated_loudness_and_the_peak() {
    let read = parse_summary(SUMMARY, true).expect("a measurement");

    assert!((read.integrated_lufs - -21.1).abs() < 1e-9);
    assert!((read.peak_dbfs - -3.4).abs() < 1e-9);
}

/// Both peak blocks are present when both modes are on, so the figure has to be
/// read under its own heading rather than by finding a `Peak:` line.
#[test]
fn a_summary_with_both_peaks_reports_the_one_that_was_asked_for() {
    let sample = parse_summary(SUMMARY, false).expect("a measurement");

    assert!((sample.peak_dbfs - -18.1).abs() < 1e-9);
}

/// A silent file reports `-inf` and has no gain anyone can compute; reading it
/// as a number would ask a player for an enormous boost.
#[test]
fn a_silent_file_has_no_measurable_loudness() {
    let summary = "\
  Integrated loudness:
    I:          -inf LUFS

  True peak:
    Peak:       -inf dBFS
";

    assert_eq!(parse_summary(summary, true), None);
}

#[test]
fn output_that_holds_no_summary_measures_nothing() {
    assert_eq!(parse_summary("", true), None);
    assert_eq!(parse_summary("ffmpeg: no such file\n", true), None);
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
    assert!(loudness.peak_dbfs <= 0.0, "{loudness:?}");
}

/// The property that does not depend on an ffmpeg version's exact figure: half
/// the amplitude is 6 dB quieter, and the scan has to say so.
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
    let measured = measure_files(&[loud, quiet], &settings);

    let first = measured[0].loudness.expect("the loud measurement");
    let second = measured[1].loudness.expect("the quiet measurement");
    assert!(
        (first.integrated_lufs - second.integrated_lufs - 6.0).abs() < 0.3,
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
    needs_ffmpeg!();
    let scratch = Scratch::new("loudness-broken");
    let path = scratch.join("broken.flac");
    std::fs::write(&path, b"not audio").expect("a written file");

    let result = measure_file(&path, &LoudnessSettings::default());

    assert_eq!(result.loudness, None);
    assert!(!result.detail.is_empty());
}

#[test]
fn results_come_back_in_the_order_the_paths_were_given() {
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

    let measured = measure_files(&paths, &LoudnessSettings::default());

    assert_eq!(
        measured
            .iter()
            .map(|result| result.path.clone())
            .collect::<Vec<_>>(),
        paths
    );
}
