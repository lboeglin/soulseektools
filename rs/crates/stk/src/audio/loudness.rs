//! Measuring how loud a file actually is, in LUFS.
//!
//! ReplayGain needs an absolute, K-weighted, twice-gated figure -- the one EBU
//! R 128 and ITU-R BS.1770 define -- plus a peak, so a player applying positive
//! gain has something to check itself against. `audio/fingerprint.rs` computes
//! a loudness envelope and is no help at all: its last step subtracts the mean,
//! throwing the absolute level away on purpose so two copies of one track match
//! whatever they were mastered at.
//!
//! The measurement is the `ebur128` crate, a port of libebur128, rather than
//! ffmpeg's filter of the same name: #9 is removing ffmpeg, and a scan built on
//! a subprocess whose text output had to be parsed would be a line-for-line
//! rewrite later. Only the decode is still ffmpeg's, and that is the one call
//! #9 replaces wholesale.
//!
//! Measuring a record is not averaging its tracks. The gate that decides which
//! 400 ms blocks count is relative to the *programme* being measured, so an
//! album's threshold is the album's, not each track's. `EbuR128` keeps the
//! block energies that make the exact figure reachable, which is why an album
//! is scanned as a unit here rather than combined arithmetically afterwards.

use std::path::{Path, PathBuf};

use ebur128::{EbuR128, Mode};
use rayon::prelude::*;

use crate::audio::ffmpeg;
use crate::config::LoudnessSettings;
use crate::errors::{Error, Result};
use crate::library::tags;

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
/// A failure is carried rather than returned, so one unreadable file cannot
/// abort a library-wide scan -- the same shape `verify_files` uses.
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
    /// Absent unless every track was measured. A gate is relative to the
    /// programme it is applied to, so a figure taken from nine tracks of a
    /// twelve-track record describes nine tracks -- and writing it to those
    /// nine as their album gain would say otherwise.
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
/// The tracks are measured in parallel -- each is a full decode spent waiting
/// on ffmpeg rather than computing -- and their analyser states are kept only
/// as long as this call, which is what bounds the memory a library-wide scan
/// holds: one album's worth at a time rather than every file's.
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
    let stream = tags::read_stream_info(path)?;
    let (Some(sample_rate), Some(channels)) = (stream.sample_rate, stream.channels) else {
        return Err(Error::Tag(format!(
            "{}: the header states no sample rate or channel count",
            path.display()
        )));
    };

    // Fed a block at a time as ffmpeg decodes it: the analyser only ever needs
    // the last 400 ms, so holding a whole four-minute track -- 85 MB of `f32`,
    // times every track of a record decoded at once -- would be memory spent on
    // nothing.
    let mut state = analyser(sample_rate, channels, settings.true_peak)?;
    ffmpeg::decode_interleaved(path, sample_rate, channels, &mut |frames| {
        state
            .add_frames_f32(frames)
            .map_err(|error| Error::Tag(format!("loudness analyser: {error}")))
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
        .map_err(|error| Error::Tag(format!("loudness analyser: {error}")))
}

/// Measure interleaved samples that are already in memory.
///
/// The whole of the analysis, with nothing to decode and nothing to spawn, so
/// what a figure means is established directly.
pub fn measure_samples(
    samples: &[f32],
    sample_rate: u32,
    channels: u32,
    true_peak: bool,
) -> Result<EbuR128> {
    let mut state = analyser(sample_rate, channels, true_peak)?;
    state
        .add_frames_f32(samples)
        .map_err(|error| Error::Tag(format!("loudness analyser: {error}")))?;
    Ok(state)
}

/// The figures a finished analyser holds, for one file.
///
/// A file of digital silence gates away to nothing and reports `-inf`, which is
/// not a loudness and would become a gain asking a player to raise silence
/// infinitely. It is reported as unmeasurable instead.
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
