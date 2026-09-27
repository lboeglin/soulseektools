//! Running the spectral check against files on disk.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::audio::decode::{probe_windows, Audio, StreamInfo};
use crate::audio::spectral::{self, Claim};
use crate::config::VerifySettings;
use crate::errors::{Error, Result};
use crate::library::tags;
use crate::models::{Verdict, VerifyResult};

/// Decoding failures become [`Verdict::Error`], so one file can't abort a run.
#[must_use]
pub fn verify_file(path: &Path, settings: &VerifySettings) -> VerifyResult {
    let sampled = sample(path, settings).and_then(|(stream, chunks, bits_used)| {
        Ok((claim(path, &stream)?, stream, chunks, bits_used))
    });
    let (claim, stream, chunks, bits_used) = match sampled {
        Ok(sampled) => sampled,
        Err(error) => {
            return VerifyResult {
                path: path.to_path_buf(),
                verdict: Verdict::Error,
                // The result already names the file.
                detail: error
                    .to_string()
                    .replace(&format!("{}: ", path.display()), ""),
                sample_rate: None,
                bit_depth: None,
                codec: String::new(),
                cutoff_hz: None,
                nyquist_hz: None,
                steepness_db_per_khz: None,
            };
        }
    };

    let Some(analysis) = spectral::analyse(&chunks, stream.sample_rate, claim, &settings.tuning())
    else {
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
    let (verdict, detail) = match (stream.bit_depth, bits_used) {
        (Some(claimed), Some(used)) if used <= 16 && claimed > 16 => {
            padded(analysis.verdict, analysis.detail, claimed, used)
        }
        _ => (analysis.verdict, analysis.detail),
    };
    VerifyResult {
        path: path.to_path_buf(),
        verdict,
        detail,
        sample_rate: Some(stream.sample_rate),
        bit_depth: stream.bit_depth,
        codec: stream.codec,
        cutoff_hz: Some(round_half_even(analysis.cutoff_hz)),
        nyquist_hz: Some(round_half_even(nyquist)),
        steepness_db_per_khz: Some(round_half_even_to(analysis.steepness_db_per_khz, 1)),
    }
}

/// Only an MP3 is held to its bitrate: the known cutoffs are LAME's.
fn claim(path: &Path, stream: &StreamInfo) -> Result<Claim> {
    if stream.codec != "mp3" {
        return Ok(Claim::Lossless);
    }
    let bitrate_kbps = tags::read_stream_info(path)?
        .bitrate
        .filter(|bitrate| *bitrate != 0)
        .ok_or_else(|| Error::Audio(format!("{}: no bitrate in the header", path.display())))?;
    Ok(Claim::Lossy {
        bitrate_kbps,
        is_vbr: tags::mp3_is_vbr(path),
    })
}

/// Padding is an upsample, unless the spectrum already found a transcode.
fn padded(verdict: Verdict, detail: String, claimed: u32, used: u32) -> (Verdict, String) {
    let padding = format!("{claimed}-bit container holding {used}-bit audio");
    match verdict {
        Verdict::Ok | Verdict::Suspect => (
            Verdict::Upsampled,
            format!("{padding} -- padded, not hi-res"),
        ),
        Verdict::Upsampled => (Verdict::Upsampled, format!("{detail}; {padding}")),
        Verdict::Fake | Verdict::Error => (verdict, detail),
    }
}

fn sample(
    path: &Path,
    settings: &VerifySettings,
) -> Result<(StreamInfo, Vec<Vec<f32>>, Option<u32>)> {
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
    Ok((stream, chunks, audio.bits_used()))
}

/// Round half to even, which float formatting does and `f64::round` doesn't,
/// so reports stay comparable with those written before.
fn round_half_even(value: f64) -> i64 {
    format!("{value:.0}").parse().unwrap_or(0)
}

fn round_half_even_to(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// Results come back in the order the paths were given.
#[must_use]
pub fn verify_files(paths: &[PathBuf], settings: &VerifySettings) -> Vec<VerifyResult> {
    paths
        .par_iter()
        .map(|path| verify_file(path, settings))
        .collect()
}
