//! Turning a loudness measurement into the tags a player acts on (#7).
//!
//! Two figures, and they are not interchangeable. **Track gain** flattens every
//! file to the same loudness, which is what shuffle wants. **Album gain**
//! applies one figure across a record, so a quiet interlude stays quiet
//! relative to the track after it. Players choose per context, so both are
//! written.
//!
//! An album is defined by its tags -- `(album artist or artist, album)` -- and
//! nothing else. That is exactly how a player groups when it applies album
//! gain, so measuring any other way would produce a figure it never uses. A
//! file with no album tag has no album to belong to and gets a track gain only;
//! falling back to the containing folder would give a folder of unrelated
//! singles one shared figure, which is the wrong answer for that folder.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::audio::loudness::Loudness;
use crate::config::LoudnessSettings;
use crate::models::{ReplayGainTags, TrackTags};
use crate::text::normalize::flatten;

/// The offset BS.1770 applies between energy and loudness.
const LOUDNESS_OFFSET_DB: f64 = -0.691;

/// What the Opus header measures against, fixed by the format.
const R128_REFERENCE_LUFS: f64 = -23.0;

/// A file a scan is about to look at.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub tags: TrackTags,
    pub duration_seconds: Option<f64>,
    pub current: ReplayGainTags,
    /// Whether this container keeps an `R128_*` field as well.
    pub r128: bool,
}

/// A target with whatever the scan found in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub target: Target,
    pub loudness: Option<Loudness>,
}

/// What one file would gain, and what it carries now.
#[derive(Debug, Clone, PartialEq)]
pub struct GainChange {
    pub path: PathBuf,
    pub current: ReplayGainTags,
    pub proposed: ReplayGainTags,
}

impl GainChange {
    #[must_use]
    pub fn is_change(&self) -> bool {
        self.current != self.proposed
    }
}

/// The files a run will measure, and the ones an existing scan already covers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub wanted: Vec<Target>,
    pub skipped: Vec<Target>,
}

/// The album a file belongs to, or `None` when its tags name none.
///
/// Compared through [`flatten`] rather than `normalize`: `normalize` drops a
/// bracketed suffix, which would file "Ghost Rule (Deluxe Edition)" and
/// "Ghost Rule" as one record when they are two, mastered separately.
#[must_use]
pub fn album_key(tags: &TrackTags) -> Option<(String, String)> {
    let album = flatten(&tags.album);
    if album.is_empty() {
        return None;
    }
    let artist = if tags.album_artist.is_empty() {
        &tags.artist
    } else {
        &tags.album_artist
    };
    Some((flatten(artist), album))
}

/// Decide what to measure, never overwriting a scan somebody else already ran.
///
/// The decision is made per album rather than per file. An album gain is a
/// figure over a whole record, so measuring six tracks of a twelve-track album
/// and leaving the rest would write an album gain derived from half of it. A
/// record every one of whose tracks is already scanned is left alone; one with
/// any gap is measured entire.
#[must_use]
pub fn select(targets: Vec<Target>, album_gain: bool, force: bool) -> Selection {
    if force {
        return Selection {
            wanted: targets,
            skipped: Vec::new(),
        };
    }

    let mut covered: HashMap<(String, String), bool> = HashMap::new();
    for target in &targets {
        let scanned = target.current.is_scanned();
        covered
            .entry(grouping(target, album_gain))
            .and_modify(|all| *all = *all && scanned)
            .or_insert(scanned);
    }

    let mut selection = Selection::default();
    for target in targets {
        if covered
            .get(&grouping(&target, album_gain))
            .copied()
            .unwrap_or(false)
        {
            selection.skipped.push(target);
        } else {
            selection.wanted.push(target);
        }
    }
    selection
}

/// The group a file is decided and measured with: its album, or itself alone.
fn grouping(target: &Target, album_gain: bool) -> (String, String) {
    album_gain
        .then(|| album_key(&target.tags))
        .flatten()
        .unwrap_or_else(|| (String::new(), target.path.display().to_string()))
}

/// Work out the tags every measured file would carry.
///
/// A file the scan could not measure yields nothing: there is no figure to
/// write, and the caller reports why separately.
#[must_use]
pub fn plan(
    measured: &[Measured],
    settings: &LoudnessSettings,
    album_gain: bool,
) -> Vec<GainChange> {
    let albums = album_figures(measured, settings, album_gain);

    measured
        .iter()
        .filter_map(|entry| {
            let loudness = entry.loudness?;
            let track_gain = gain_db(loudness.integrated_lufs, settings.reference_lufs);
            let album = album_key(&entry.target.tags)
                .and_then(|key| albums.get(&key))
                .copied();

            let mut proposed = ReplayGainTags {
                track_gain: Some(format_gain(track_gain)),
                track_peak: Some(format_peak(peak_amplitude(loudness.peak_dbfs))),
                album_gain: album.map(|(gain, _)| format_gain(gain)),
                album_peak: album.map(|(_, peak)| format_peak(peak)),
                r128_track_gain: None,
                r128_album_gain: None,
            };
            if entry.target.r128 {
                proposed.r128_track_gain = Some(format_r128(track_gain, settings.reference_lufs));
                proposed.r128_album_gain =
                    album.map(|(gain, _)| format_r128(gain, settings.reference_lufs));
            }

            Some(GainChange {
                path: entry.target.path.clone(),
                current: entry.target.current.clone(),
                proposed,
            })
        })
        .collect()
}

/// One gain and one peak per album, from the tracks this run measured.
fn album_figures(
    measured: &[Measured],
    settings: &LoudnessSettings,
    album_gain: bool,
) -> HashMap<(String, String), (f64, f64)> {
    if !album_gain {
        return HashMap::new();
    }

    let mut grouped: HashMap<(String, String), Vec<(Loudness, f64)>> = HashMap::new();
    for entry in measured {
        let (Some(loudness), Some(key)) = (entry.loudness, album_key(&entry.target.tags)) else {
            continue;
        };
        grouped
            .entry(key)
            .or_default()
            .push((loudness, entry.target.duration_seconds.unwrap_or(0.0)));
    }

    grouped
        .into_iter()
        .filter_map(|(key, tracks)| {
            let loudness = album_loudness(&tracks)?;
            let peak = tracks
                .iter()
                .map(|(track, _)| peak_amplitude(track.peak_dbfs))
                .fold(0.0_f64, f64::max);
            Some((key, (gain_db(loudness, settings.reference_lufs), peak)))
        })
        .collect()
}

/// The loudness of a whole record, from the loudness of its tracks.
///
/// Energy summed across the album and weighted by how long each track is, which
/// is what BS.1770 means by the loudness of a programme made of several parts.
///
/// It is an approximation, knowingly. A true album measurement re-gates every
/// 400 ms block against the *album's* relative threshold, where these figures
/// were each gated against their own track's; combining them cannot reproduce
/// that. The deviation is a few tenths of an LU on an ordinary record, well
/// under the decibel anyone can hear, and the alternative -- a second ffmpeg
/// pass over each album's tracks concatenated -- doubles the decode cost of the
/// slowest thing this tool does and fails outright on an album whose tracks do
/// not share a sample rate and channel layout.
#[must_use]
pub fn album_loudness(tracks: &[(Loudness, f64)]) -> Option<f64> {
    if tracks.is_empty() {
        return None;
    }

    // A track of unknown length still belongs to the record, so where no length
    // is known at all the weights fall back to one apiece rather than to zero.
    let timed = tracks.iter().any(|(_, seconds)| *seconds > 0.0);
    let weight = |seconds: f64| if timed { seconds } else { 1.0 };

    let total: f64 = tracks.iter().map(|(_, seconds)| weight(*seconds)).sum();
    if total <= 0.0 {
        return None;
    }

    let energy: f64 = tracks
        .iter()
        .map(|(track, seconds)| {
            weight(*seconds) * 10.0_f64.powf((track.integrated_lufs - LOUDNESS_OFFSET_DB) / 10.0)
        })
        .sum();
    Some(LOUDNESS_OFFSET_DB + 10.0 * (energy / total).log10())
}

/// How far a player has to move a file to reach the reference level.
#[must_use]
pub fn gain_db(integrated_lufs: f64, reference_lufs: f64) -> f64 {
    reference_lufs - integrated_lufs
}

/// A peak in dBFS as the linear amplitude the tags carry: 1.0 is full scale.
#[must_use]
pub fn peak_amplitude(dbfs: f64) -> f64 {
    10.0_f64.powf(dbfs / 20.0)
}

/// A gain the way every other scanner spells it.
#[must_use]
pub fn format_gain(db: f64) -> String {
    format!("{db:.2} dB")
}

#[must_use]
pub fn format_peak(amplitude: f64) -> String {
    format!("{amplitude:.6}")
}

/// The Opus header's own spelling of a gain: Q7.8 fixed point against
/// -23 LUFS, whatever reference the `REPLAYGAIN_*` figures were measured to.
///
/// The field is a signed 16 bit integer, so it saturates rather than wrapping --
/// a wrapped value would be a loud file asking to be made louder still.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn format_r128(gain_db: f64, reference_lufs: f64) -> String {
    let against_r128 = gain_db + (R128_REFERENCE_LUFS - reference_lufs);
    let steps = (against_r128 * 256.0).round();
    let clamped = steps.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
    clamped.to_string()
}
