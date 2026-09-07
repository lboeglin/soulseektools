//! Running the spectral check against files on disk.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::audio::decode::{probe_windows, Audio, StreamInfo};
use crate::audio::spectral;
use crate::config::VerifySettings;
use crate::errors::Result;
use crate::models::{Verdict, VerifyResult};

/// Decide whether a lossless container really holds lossless audio.
///
/// Decoding failures are reported as [`Verdict::Error`] rather than returned as
/// an error, so one unreadable file cannot abort a library-wide run.
#[must_use]
pub fn verify_file(path: &Path, settings: &VerifySettings) -> VerifyResult {
    let (stream, chunks) = match sample(path, settings) {
        Ok(sampled) => sampled,
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

/// The stream's properties and a few windows of its audio, from one open.
fn sample(path: &Path, settings: &VerifySettings) -> Result<(StreamInfo, Vec<Vec<f32>>)> {
    let mut audio = Audio::open(path)?;
    let stream = audio.stream().clone();

    let mut chunks = Vec::new();
    for (start, length) in probe_windows(
        stream.duration_seconds,
        settings.probe_seconds,
        settings.probe_count,
    ) {
        let chunk = audio.window(start, length)?;
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
    }
    Ok((stream, chunks))
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
/// `verify` is the most expensive command in the tool: every file is opened,
/// seeked to four places and decoded there, and the transform over what comes
/// back is the work. The Python runs the files one after another; fanning out
/// over them is the single largest win available here, and it is deliberate
/// work rather than something the language gives for free.
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
