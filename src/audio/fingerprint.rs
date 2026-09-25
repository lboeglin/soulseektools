//! Chromaprint fingerprints (as AcoustID uses), computed by `rusty-chromaprint`
//! and compared locally by bit error rate.
//!
//! They agree with `fpcalc` on about 98.5% of bits, not 100%, so [`KIND`] tags
//! each cached print and one from another backend is recomputed.
// Casts between frame counts and offsets are bounded by a track length.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rusty_chromaprint::{Configuration, FingerprintCompressor, Fingerprinter};

use crate::audio::decode::Audio;
use crate::errors::{Error, Result};

/// What produced a stored fingerprint.
///
/// Stored beside each cached print; an unknown kind is a cache miss.
pub const KIND: &str = "chromaprint-rs";

/// How much of a track is fingerprinted.
///
/// `fpcalc`'s default. Releases of one recording differ at the end far more
/// often than in the first two minutes.
const LIMIT_SECONDS: u64 = 120;

/// How far two fingerprints may be shifted against each other when comparing.
///
/// Rips of the same track routinely differ by a second or two of leading
/// silence.
pub const MAX_ALIGNMENT_FRAMES: i64 = 8;

const MIN_COMPARABLE_FRAMES: usize = 8;

/// A comparable summary of a track's audio.
#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint {
    pub values: Vec<u32>,
    pub duration_seconds: Option<f64>,
}

impl Fingerprint {
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The form AcoustID takes, which is what `fpcalc` prints.
    ///
    /// Delta-coded, bit-packed, URL-safe base64 without padding. The first byte
    /// names the algorithm, so it must match what [`fingerprint`] used.
    #[must_use]
    pub fn compressed(&self) -> String {
        let configuration = preset();
        URL_SAFE_NO_PAD.encode(FingerprintCompressor::from(&configuration).compress(&self.values))
    }
}

/// The preset AcoustID's index is built from, and therefore the only one a
/// lookup can be made with.
fn preset() -> Configuration {
    Configuration::preset_test2()
}

/// Compute a track's fingerprint.
pub fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let mut audio = Audio::open(path)?;
    let stream = audio.stream().clone();

    let mut printer = Fingerprinter::new(&preset());
    printer
        .start(stream.sample_rate, stream.channels)
        .map_err(|error| Error::Audio(format!("{}: {error}", path.display())))?;

    let channels = (stream.channels as usize).max(1);
    let mut wanted = usize::try_from(u64::from(stream.sample_rate) * LIMIT_SECONDS)
        .unwrap_or(usize::MAX)
        .saturating_mul(channels);
    audio.frames(&mut |frames: &[i16]| {
        let take = wanted.min(frames.len());
        printer.consume(&frames[..take]);
        wanted -= take;
        Ok(())
    })?;
    printer.finish();

    Ok(Fingerprint {
        values: printer.fingerprint().to_vec(),
        duration_seconds: stream.duration_seconds,
    })
}

/// How alike two fingerprints are, in `[0, 1]`.
#[must_use]
pub fn similarity(left: &Fingerprint, right: &Fingerprint) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let mut best = 0.0_f64;
    for offset in -MAX_ALIGNMENT_FRAMES..=MAX_ALIGNMENT_FRAMES {
        let (first, second) = align(&left.values, &right.values, offset);
        if first.len() < MIN_COMPARABLE_FRAMES {
            continue;
        }
        let differing: u32 = first
            .iter()
            .zip(second)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum();
        best = best.max(1.0 - f64::from(differing) / (first.len() * 32) as f64);
    }
    best
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
