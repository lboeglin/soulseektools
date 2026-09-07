//! Acoustic fingerprints, for finding duplicates that share no metadata.
//!
//! Grouping by duration and file size -- what the original script did -- produces
//! coincidental matches between unrelated tracks of similar length and bitrate.
//! A fingerprint compares the audio itself.
//!
//! The fingerprint is chromaprint, the same one AcoustID uses, computed here by
//! `rusty-chromaprint` rather than by the `fpcalc` binary. It is compared
//! locally by bit error rate, so no web service and no API key are involved.
//!
//! The port is not bit-identical to `fpcalc`: over a real library the two agree
//! to about 98.5% of bits, which is far above the 0.9 that decides a duplicate
//! but far below the 1.0 two reads of one file should score. That is why
//! [`KIND`] exists -- a fingerprint cached by the old backend is recomputed
//! rather than compared against one from this.
// Frame counts and offsets are cast between `usize`, `u64` and `f64`
// throughout, all bounded by a track length rather than an address space.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::path::Path;

use rusty_chromaprint::{Configuration, Fingerprinter};

use crate::audio::decode::Audio;
use crate::errors::{Error, Result};

/// What produced a stored fingerprint.
///
/// The index keeps this beside the values and treats a name it does not know as
/// a miss, so changing it is how a cache is invalidated: rows written by
/// `fpcalc` are recomputed instead of being compared against a port that
/// disagrees with it in the last percent of bits.
pub const KIND: &str = "chromaprint-rs";

/// How much of a track is fingerprinted.
///
/// `fpcalc`'s own default, and therefore what every cached row was computed
/// under. Two releases of the same recording differ at the end far more often
/// than in the first two minutes, so the cap is also the more reliable
/// comparison.
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
}

/// Compute a track's fingerprint.
pub fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let mut audio = Audio::open(path)?;
    let stream = audio.stream().clone();

    let mut printer = Fingerprinter::new(&Configuration::preset_test2());
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
