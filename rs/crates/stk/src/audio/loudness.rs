//! Measuring how loud a file actually is, in LUFS.
//!
//! ReplayGain needs an absolute, K-weighted, twice-gated figure -- the one EBU
//! R 128 and ITU-R BS.1770 define -- plus a peak, so a player applying positive
//! gain has something to check itself against. `audio/fingerprint.rs` computes
//! a loudness envelope and is no help at all: its last step subtracts the mean,
//! throwing the absolute level away on purpose so two copies of one track match
//! whatever they were mastered at.
//!
//! ffmpeg's `ebur128` filter does the measurement. Reading its summary is a
//! pure function over text, so what the parser accepts is tested directly
//! rather than through a subprocess.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::audio::ffmpeg;
use crate::config::LoudnessSettings;

/// What a scan found in one file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness over the whole file, gated as BS.1770 specifies.
    pub integrated_lufs: f64,
    /// The loudest the signal gets, in dBFS. Above zero on a clipped master.
    pub peak_dbfs: f64,
}

/// One file's measurement, or why it has none.
///
/// A failure is carried rather than returned, so one unreadable file cannot
/// abort a library-wide scan -- the same shape `verify_files` uses.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessResult {
    pub path: PathBuf,
    pub loudness: Option<Loudness>,
    pub detail: String,
}

/// Measure one file.
#[must_use]
pub fn measure_file(path: &Path, settings: &LoudnessSettings) -> LoudnessResult {
    let summary = match ffmpeg::ebur128(path, settings.true_peak) {
        Ok(summary) => summary,
        Err(error) => {
            return LoudnessResult {
                path: path.to_path_buf(),
                loudness: None,
                detail: error.to_string(),
            }
        }
    };

    match parse_summary(&summary, settings.true_peak) {
        Some(loudness) => LoudnessResult {
            path: path.to_path_buf(),
            loudness: Some(loudness),
            detail: String::new(),
        },
        None => LoudnessResult {
            path: path.to_path_buf(),
            loudness: None,
            detail: "no measurable loudness".to_owned(),
        },
    }
}

/// Measure many files, in parallel.
///
/// Every one is a full decode spent waiting on ffmpeg rather than computing, so
/// this is the same trade `verify_files` makes and the only one that matters
/// for how long a library-wide scan takes. Results come back in the order the
/// paths were given, so a plan is reproducible.
#[must_use]
pub fn measure_files(paths: &[PathBuf], settings: &LoudnessSettings) -> Vec<LoudnessResult> {
    paths
        .par_iter()
        .map(|path| measure_file(path, settings))
        .collect()
}

/// Read what the `ebur128` filter reported.
///
/// The summary looks like this, and both peak blocks are present when both peak
/// modes are enabled, so the peak has to be read under its own heading rather
/// than by finding a `Peak:` line:
///
/// ```text
///   Integrated loudness:
///     I:         -21.1 LUFS
///     Threshold: -31.1 LUFS
///   ...
///   Sample peak:
///     Peak:      -18.1 dBFS
///
///   True peak:
///     Peak:      -18.1 dBFS
/// ```
///
/// A file of digital silence reports `-inf` for both and has no gain anyone can
/// compute, so it reads back as `None` rather than as an enormous boost.
#[must_use]
pub fn parse_summary(text: &str, true_peak: bool) -> Option<Loudness> {
    let wanted_peak = if true_peak {
        "True peak:"
    } else {
        "Sample peak:"
    };

    let mut integrated = None;
    let mut peak = None;
    let mut under_wanted_peak = false;

    for line in text.lines() {
        let line = line.trim();
        if is_heading(line) {
            under_wanted_peak = line == wanted_peak;
            continue;
        }
        if let Some(value) = line.strip_prefix("I:") {
            integrated = number_before_unit(value);
        } else if under_wanted_peak {
            if let Some(value) = line.strip_prefix("Peak:") {
                peak = number_before_unit(value);
            }
        }
    }

    Some(Loudness {
        integrated_lufs: integrated?,
        peak_dbfs: peak?,
    })
}

/// One of the summary's section headings, rather than a figure inside one.
fn is_heading(line: &str) -> bool {
    matches!(
        line,
        "Integrated loudness:" | "Loudness range:" | "Sample peak:" | "True peak:"
    )
}

/// The number in `-21.1 LUFS`, or `None` for the `-inf` a silent file reports.
///
/// `-inf` parses perfectly well as a float, and would then become a gain asking
/// a player to raise a silent file by an infinite amount. A file whose peak is
/// `-inf` holds no signal at all, which is the case ffmpeg reports as an
/// integrated loudness of exactly -70 -- the absolute gate -- rather than as
/// `-inf` of its own.
fn number_before_unit(value: &str) -> Option<f64> {
    value
        .split_whitespace()
        .next()?
        .parse()
        .ok()
        .filter(|number: &f64| number.is_finite())
}
