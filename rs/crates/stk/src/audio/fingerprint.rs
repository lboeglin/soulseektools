//! Acoustic fingerprints, for finding duplicates that share no metadata.
//!
//! Grouping by duration and file size -- what the original script did -- produces
//! coincidental matches between unrelated tracks of similar length and bitrate.
//! A fingerprint compares the audio itself.
//!
//! Two backends, chosen at runtime:
//!
//! - **chromaprint** (`fpcalc`), the same fingerprint AcoustID uses. Compared
//!   locally by bit error rate, so no web service and no API key are involved.
//! - **loudness envelope**, the fallback when `fpcalc` is absent: the track's
//!   per-frame loudness curve, compared by correlation. It survives re-encoding
//!   and differing bitrates, which a byte hash does not, while needing only
//!   ffmpeg.
//!
//! Fingerprints of different kinds are never compared.
// Sample counts, band indices and frame offsets are cast between `usize`, `f64`
// and the integer widths throughout. Every one of them is bounded by an FFT size
// or a track length -- thousands, not billions -- so the pedantic cast lints
// only ask for `try_from` unwraps around values that cannot overflow.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;

use serde::Deserialize;

use crate::audio::ffmpeg;
use crate::errors::{Error, Result};

pub const ENVELOPE_SAMPLE_RATE: u32 = 8000;
pub const ENVELOPE_FRAME_SECONDS: f64 = 0.25;

/// How far two fingerprints may be shifted against each other when comparing.
///
/// Rips of the same track routinely differ by a second or two of leading
/// silence.
pub const MAX_ALIGNMENT_FRAMES: i64 = 8;

const FPCALC_TIMEOUT: Duration = Duration::from_secs(60);
const MIN_COMPARABLE_FRAMES: usize = 8;
const FLAT_ENVELOPE_DB: f64 = 1.0;
const NO_EVIDENCE: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FingerprintKind {
    Chromaprint,
    Envelope,
}

impl FingerprintKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chromaprint => "chromaprint",
            Self::Envelope => "envelope",
        }
    }
}

impl fmt::Display for FingerprintKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for FingerprintKind {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        match text {
            "chromaprint" => Ok(Self::Chromaprint),
            "envelope" => Ok(Self::Envelope),
            other => Err(format!("unknown fingerprint kind: {other}")),
        }
    }
}

/// A comparable summary of a track's audio.
///
/// The two backends carry different value types -- chromaprint's 32-bit words
/// and the envelope's dB curve -- and are never compared against each other.
#[derive(Debug, Clone, PartialEq)]
pub enum Values {
    Chromaprint(Vec<u32>),
    Envelope(Vec<f64>),
}

impl Values {
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Chromaprint(values) => values.len(),
            Self::Envelope(values) => values.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn kind(&self) -> FingerprintKind {
        match self {
            Self::Chromaprint(_) => FingerprintKind::Chromaprint,
            Self::Envelope(_) => FingerprintKind::Envelope,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint {
    pub values: Values,
    pub duration_seconds: Option<f64>,
}

impl Fingerprint {
    #[must_use]
    pub fn kind(&self) -> FingerprintKind {
        self.values.kind()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[must_use]
pub fn fpcalc_path() -> Option<PathBuf> {
    which::which("fpcalc").ok()
}

/// Which backend this machine will use.
#[must_use]
pub fn available_kind() -> FingerprintKind {
    if fpcalc_path().is_some() {
        FingerprintKind::Chromaprint
    } else {
        FingerprintKind::Envelope
    }
}

/// Compute a fingerprint using the requested backend, or the best available.
pub fn fingerprint(path: &Path, kind: Option<FingerprintKind>) -> Result<Fingerprint> {
    match kind.unwrap_or_else(available_kind) {
        FingerprintKind::Chromaprint => chromaprint(path),
        FingerprintKind::Envelope => envelope(path),
    }
}

/// How alike two fingerprints are, in `[0, 1]`.
///
/// Comparing across backends is a programming error, not a low score, so it is
/// an error rather than a number.
pub fn similarity(left: &Fingerprint, right: &Fingerprint) -> Result<f64> {
    match (&left.values, &right.values) {
        _ if left.kind() != right.kind() => Err(Error::ExternalTool(format!(
            "cannot compare {} with {}",
            left.kind(),
            right.kind()
        ))),
        _ if left.is_empty() || right.is_empty() => Ok(0.0),
        (Values::Chromaprint(first), Values::Chromaprint(second)) => {
            Ok(chromaprint_similarity(first, second))
        }
        (Values::Envelope(first), Values::Envelope(second)) => {
            Ok(envelope_similarity(first, second))
        }
        _ => unreachable!("kinds were compared above"),
    }
}

#[derive(Deserialize)]
struct FpcalcOutput {
    #[serde(default)]
    fingerprint: Vec<u32>,
    #[serde(default)]
    duration: Option<f64>,
}

fn chromaprint(path: &Path) -> Result<Fingerprint> {
    let binary = fpcalc_path().ok_or_else(|| {
        Error::ExternalTool("fpcalc is not on PATH; install chromaprint to use it".to_owned())
    })?;

    let mut child = Command::new(binary)
        .args(["-raw", "-json"])
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| Error::ExternalTool(format!("fpcalc could not be started: {error}")))?;

    // Drained on their own threads for the same reason as in `ffmpeg::run`: a
    // raw fingerprint easily fills a pipe buffer.
    let mut stdout = child.stdout.take().expect("a piped stdout");
    let mut stderr = child.stderr.take().expect("a piped stderr");
    let reading_stdout = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let reading_stderr = std::thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        buffer
    });

    let deadline = std::time::Instant::now() + FPCALC_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                return Err(Error::ExternalTool("fpcalc timed out".to_owned()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(Error::ExternalTool(format!("fpcalc failed: {error}"))),
        }
    };

    let stdout = reading_stdout.join().unwrap_or_default();
    let stderr = reading_stderr.join().unwrap_or_default();

    if !status.success() {
        return Err(Error::ExternalTool(stderr.trim().to_owned()));
    }

    parse_fpcalc_output(&stdout)
}

/// Read what `fpcalc -raw -json` printed.
///
/// Separate from spawning it so the parsing is covered without a fake
/// subprocess, which is what the Python tests monkeypatch `subprocess.run` for.
pub fn parse_fpcalc_output(stdout: &[u8]) -> Result<Fingerprint> {
    let payload: FpcalcOutput = serde_json::from_slice(stdout)
        .map_err(|error| Error::ExternalTool(format!("fpcalc returned no usable JSON: {error}")))?;

    Ok(Fingerprint {
        values: Values::Chromaprint(payload.fingerprint),
        duration_seconds: payload.duration.filter(|seconds| *seconds != 0.0),
    })
}

fn envelope(path: &Path) -> Result<Fingerprint> {
    let stream = ffmpeg::probe(path)?;
    let samples = ffmpeg::decode(path, ENVELOPE_SAMPLE_RATE, 0.0, None)?;
    Ok(Fingerprint {
        values: Values::Envelope(envelope_of(&samples, ENVELOPE_SAMPLE_RATE)),
        duration_seconds: stream.duration_seconds,
    })
}

/// The per-frame loudness curve, in dB and mean-centred.
///
/// Centring removes the overall level, so the same track ripped at two different
/// volumes still produces the same curve.
#[must_use]
pub fn envelope_of(samples: &[f32], sample_rate: u32) -> Vec<f64> {
    let frame = ((f64::from(sample_rate) * ENVELOPE_FRAME_SECONDS) as usize).max(1);
    let usable = samples.len() - samples.len() % frame;
    if usable < frame {
        return Vec::new();
    }

    let db: Vec<f64> = samples[..usable]
        .chunks_exact(frame)
        .map(|window| {
            let mean_square = window
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>()
                / frame as f64;
            20.0 * (mean_square.sqrt() + 1e-9).log10()
        })
        .collect();

    let mean = db.iter().sum::<f64>() / db.len() as f64;
    db.iter().map(|value| value - mean).collect()
}

fn chromaprint_similarity(first: &[u32], second: &[u32]) -> f64 {
    let mut best = 0.0_f64;
    for offset in -MAX_ALIGNMENT_FRAMES..=MAX_ALIGNMENT_FRAMES {
        let (left, right) = align(first, second, offset);
        if left.len() < MIN_COMPARABLE_FRAMES {
            continue;
        }
        let differing: u32 = left
            .iter()
            .zip(right)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum();
        best = best.max(1.0 - f64::from(differing) / (left.len() * 32) as f64);
    }
    best
}

fn envelope_similarity(first: &[f64], second: &[f64]) -> f64 {
    // Audio with no dynamics -- a sustained tone, a drone -- has a flat curve
    // carrying no shape to correlate, and correlating the residual numerical
    // noise would invent a confident answer. Say "no evidence" instead, a score
    // deliberately too low to group files by and too high to rule them out.
    let flat = [first, second].map(|curve| standard_deviation(curve) < FLAT_ENVELOPE_DB);
    if flat.iter().all(|value| *value) {
        return NO_EVIDENCE;
    }
    if flat.iter().any(|value| *value) {
        return 0.0;
    }

    let mut best = 0.0_f64;
    for offset in -MAX_ALIGNMENT_FRAMES..=MAX_ALIGNMENT_FRAMES {
        let (left, right) = align(first, second, offset);
        if left.len() < MIN_COMPARABLE_FRAMES {
            continue;
        }
        let (left_deviation, right_deviation) =
            (standard_deviation(left), standard_deviation(right));
        if left_deviation == 0.0 || right_deviation == 0.0 {
            continue;
        }

        let (left_mean, right_mean) = (mean(left), mean(right));
        let covariance = left
            .iter()
            .zip(right)
            .map(|(a, b)| (a - left_mean) * (b - right_mean))
            .sum::<f64>()
            / left.len() as f64;
        let correlation = covariance / (left_deviation * right_deviation);
        best = best.max((correlation + 1.0) / 2.0);
    }
    best
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// The population standard deviation, which is what numpy's `std` gives.
fn standard_deviation(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mean = mean(values);
    (values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64)
        .sqrt()
}

fn align<'a, T>(left: &'a [T], right: &'a [T], offset: i64) -> (&'a [T], &'a [T]) {
    let (shifted, other) = if offset >= 0 {
        (
            &left[usize::try_from(offset).unwrap_or(0).min(left.len())..],
            right,
        )
    } else {
        (
            left,
            &right[usize::try_from(-offset).unwrap_or(0).min(right.len())..],
        )
    };
    let length = shifted.len().min(other.len());
    (&shifted[..length], &other[..length])
}
