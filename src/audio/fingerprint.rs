//! Chromaprint fingerprints via `rusty-chromaprint`, compared by bit error rate.
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

/// Stored with each cached print; an unknown kind (from `fpcalc`, say) is a cache miss.
pub const KIND: &str = "chromaprint-rs";

/// `fpcalc`'s default; releases of one recording mostly differ near the end.
const LIMIT_SECONDS: u64 = 120;

/// Rips of one track often differ by a second or two of leading silence.
pub const MAX_ALIGNMENT_FRAMES: i64 = 8;

const MIN_COMPARABLE_FRAMES: usize = 8;

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

    /// The form AcoustID takes: delta-coded, bit-packed, unpadded URL-safe base64.
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
