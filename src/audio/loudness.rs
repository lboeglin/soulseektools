//! Integrated loudness (EBU R 128) and peak. An album is one programme: its gate spans all of it.

use std::path::{Path, PathBuf};

use ebur128::{EbuR128, Mode};
use rayon::prelude::*;

use crate::audio::decode::Audio;
use crate::config::LoudnessSettings;
use crate::errors::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    pub integrated_lufs: f64,
    /// Linear amplitude: 1.0 is full scale.
    pub peak: f64,
}

/// A failure is carried, not returned, so one file can't abort a scan.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessResult {
    pub path: PathBuf,
    pub loudness: Option<Loudness>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumScan {
    pub tracks: Vec<LoudnessResult>,
    /// Absent unless every track was measured.
    pub album: Option<Loudness>,
}

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

/// Analyser state lives only for this call, so memory is bounded by one album.
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
