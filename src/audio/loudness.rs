//! Measuring integrated loudness (EBU R 128 / BS.1770) and peak, with the
//! `ebur128` crate.
//!
//! An album is measured as one programme, not averaged from its tracks, because
//! the relative gate depends on the whole programme.

use std::path::{Path, PathBuf};

use ebur128::{EbuR128, Mode};
use rayon::prelude::*;

use crate::audio::decode::Audio;
use crate::config::LoudnessSettings;
use crate::errors::{Error, Result};

/// What a scan found in one file, or in one record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, gated as BS.1770 specifies.
    pub integrated_lufs: f64,
    /// The loudest the signal gets, as a linear amplitude: 1.0 is full scale,
    /// and a clipped master goes above it.
    pub peak: f64,
}

/// One file's measurement, or why it has none.
///
/// A failure is carried, not returned, so one file can't abort a scan.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessResult {
    pub path: PathBuf,
    pub loudness: Option<Loudness>,
    pub detail: String,
}

/// One group of files measured together: a figure each, and one for the record.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumScan {
    pub tracks: Vec<LoudnessResult>,
    /// The record's own figure.
    ///
    /// Absent unless every track was measured.
    pub album: Option<Loudness>,
}

/// Measure one file on its own.
#[must_use]
pub fn measure_file(path: &Path, settings: &LoudnessSettings) -> LoudnessResult {
    match analyse(path, settings) {
        Ok(state) => finish(path, &state),
        Err(error) => LoudnessResult {
            path: path.to_path_buf(),
            loudness: None,
            detail: error.to_string(),
        },
    }
}

/// Measure every file in one record, and the record itself.
///
/// Tracks are measured in parallel; analyser state lives only for this call,
/// so memory is bounded by one album.
#[must_use]
pub fn measure_album(paths: &[PathBuf], settings: &LoudnessSettings) -> AlbumScan {
    let analysed: Vec<(PathBuf, Result<EbuR128>)> = paths
        .par_iter()
        .map(|path| (path.clone(), analyse(path, settings)))
        .collect();

    let states: Option<Vec<&EbuR128>> = analysed
        .iter()
        .map(|(_, state)| state.as_ref().ok())
        .collect();

    let album = states.and_then(|states| {
        EbuR128::loudness_global_multiple(states.iter().copied())
            .ok()
            .filter(|loudness| loudness.is_finite())
            .map(|integrated_lufs| Loudness {
                integrated_lufs,
                peak: states
                    .iter()
                    .map(|state| loudest(state, settings.true_peak))
                    .fold(0.0_f64, f64::max),
            })
    });

    AlbumScan {
        tracks: analysed
            .iter()
            .map(|(path, state)| match state {
                Ok(state) => finish(path, state),
                Err(error) => LoudnessResult {
                    path: path.clone(),
                    loudness: None,
                    detail: error.to_string(),
                },
            })
            .collect(),
        album,
    }
}

/// Run the analyser over a whole file.
fn analyse(path: &Path, settings: &LoudnessSettings) -> Result<EbuR128> {
    let mut audio = Audio::open(path)?;
    let stream = audio.stream();

    // Fed as decoded; the analyser only needs the last 400 ms.
    let mut state = analyser(stream.sample_rate, stream.channels, settings.true_peak)?;
    audio.frames(&mut |frames| {
        state
            .add_frames_f32(frames)
            .map_err(|error| Error::Audio(format!("loudness analyser: {error}")))
    })?;
    Ok(state)
}

/// An analyser set up to answer the questions a gain figure needs.
pub fn analyser(sample_rate: u32, channels: u32, true_peak: bool) -> Result<EbuR128> {
    let mode = Mode::I
        | if true_peak {
            Mode::TRUE_PEAK
        } else {
            Mode::SAMPLE_PEAK
        };
    EbuR128::new(channels, sample_rate, mode)
        .map_err(|error| Error::Audio(format!("loudness analyser: {error}")))
}

/// Measure interleaved samples that are already in memory.
pub fn measure_samples(
    samples: &[f32],
    sample_rate: u32,
    channels: u32,
    true_peak: bool,
) -> Result<EbuR128> {
    let mut state = analyser(sample_rate, channels, true_peak)?;
    state
        .add_frames_f32(samples)
        .map_err(|error| Error::Audio(format!("loudness analyser: {error}")))?;
    Ok(state)
}

/// The figures a finished analyser holds, for one file.
///
/// Digital silence reports `-inf`, which is treated as unmeasurable.
fn finish(path: &Path, state: &EbuR128) -> LoudnessResult {
    let integrated = state
        .loudness_global()
        .ok()
        .filter(|loudness| loudness.is_finite());

    match integrated {
        Some(integrated_lufs) => LoudnessResult {
            path: path.to_path_buf(),
            loudness: Some(Loudness {
                integrated_lufs,
                peak: loudest(state, state.mode().contains(Mode::TRUE_PEAK)),
            }),
            detail: String::new(),
        },
        None => LoudnessResult {
            path: path.to_path_buf(),
            loudness: None,
            detail: "no measurable loudness".to_owned(),
        },
    }
}

/// The loudest any one channel gets.
fn loudest(state: &EbuR128, true_peak: bool) -> f64 {
    (0..state.channels())
        .filter_map(|channel| {
            if true_peak {
                state.true_peak(channel)
            } else {
                state.sample_peak(channel)
            }
            .ok()
        })
        .fold(0.0_f64, f64::max)
}
