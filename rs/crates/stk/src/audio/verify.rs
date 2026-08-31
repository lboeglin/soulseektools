//! Running the spectral check against files on disk.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::audio::{ffmpeg, spectral};
use crate::config::VerifySettings;
use crate::models::{Verdict, VerifyResult};

/// Decide whether a lossless container really holds lossless audio.
///
/// Decoding or probing failures are reported as [`Verdict::Error`] rather than
/// returned as an error, so one unreadable file cannot abort a library-wide run.
#[must_use]
pub fn verify_file(path: &Path, settings: &VerifySettings) -> VerifyResult {
    let probed = ffmpeg::probe(path).and_then(|stream| {
        let chunks = ffmpeg::decode_probes(
            path,
            stream.sample_rate,
            stream.duration_seconds,
            settings.probe_seconds,
            settings.probe_count,
        )?;
        Ok((stream, chunks))
    });

    let (stream, chunks) = match probed {
        Ok(probed) => probed,
        Err(error) => {
            return VerifyResult {
                path: path.to_path_buf(),
                verdict: Verdict::Error,
                detail: error.to_string(),
                sample_rate: None,
                bit_depth: None,
                codec: String::new(),
                cutoff_hz: None,
                nyquist_hz: None,
                steepness_db_per_khz: None,
            };
        }
    };

    let Some(analysis) = spectral::analyse(&chunks, stream.sample_rate, &settings.tuning()) else {
        return VerifyResult {
            path: path.to_path_buf(),
            verdict: Verdict::Error,
            detail: "no audible content to analyse".to_owned(),
            sample_rate: Some(stream.sample_rate),
            bit_depth: stream.bit_depth,
            codec: stream.codec,
            cutoff_hz: None,
            nyquist_hz: None,
            steepness_db_per_khz: None,
        };
    };

    let nyquist = stream.nyquist_hz();
    VerifyResult {
        path: path.to_path_buf(),
        verdict: analysis.verdict,
        detail: analysis.detail,
        sample_rate: Some(stream.sample_rate),
        bit_depth: stream.bit_depth,
        codec: stream.codec,
        cutoff_hz: Some(round_half_even(analysis.cutoff_hz)),
        nyquist_hz: Some(round_half_even(nyquist)),
        steepness_db_per_khz: Some(round_half_even_to(analysis.steepness_db_per_khz, 1)),
    }
}

/// Python's `round()`, which breaks ties to even where Rust's rounds away from
/// zero.
///
/// The report carries `round(cutoff_hz)` and `round(steepness, 1)`, so a tie
/// rounded the other way is a row that differs from `oracle/verify.csv`.
/// Formatting and reparsing gets there exactly: both languages format floats
/// with correct decimal rounding, ties to even.
fn round_half_even(value: f64) -> i64 {
    format!("{value:.0}").parse().unwrap_or(0)
}

fn round_half_even_to(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// Verify many files, in parallel.
///
/// `verify` is the most expensive command in the tool -- five subprocesses per
/// file, so roughly 1,300 for a 266-file library -- and every one of them is
/// spent waiting on ffmpeg rather than computing. The Python runs them one after
/// another; fanning out over the files is the single largest win available here,
/// and it is deliberate work rather than something the language gives for free.
///
/// Results come back in the order the paths were given, so a report is
/// reproducible.
#[must_use]
pub fn verify_files(paths: &[PathBuf], settings: &VerifySettings) -> Vec<VerifyResult> {
    paths
        .par_iter()
        .map(|path| verify_file(path, settings))
        .collect()
}
