//! Turning a loudness measurement into ReplayGain track and album tags.
//!
//! An album is `(album artist or artist, album)` from the tags, as players
//! group it. A file with no album tag gets a track gain only.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::path::PathBuf;

use crate::audio::loudness::Loudness;
use crate::config::LoudnessSettings;
use crate::models::{LibraryEntry, ReplayGainTags, TrackTags};
use crate::text::normalize::flatten;

/// What the Opus header measures against, fixed by the format.
const R128_REFERENCE_LUFS: f64 = -23.0;

/// A file a scan is about to look at.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub tags: TrackTags,
    pub current: ReplayGainTags,
    /// Whether this container keeps an `R128_*` field as well.
    pub r128: bool,
}

/// The files a scan decides on, measures and levels together.
///
/// One record, or one file that names none.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// Absent when these files name no album, and so get a track gain only.
    pub album: Option<(String, String)>,
    pub targets: Vec<Target>,
}

/// A group with what the scan found in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Scanned {
    pub album: Option<Loudness>,
    pub tracks: Vec<(Target, Option<Loudness>)>,
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

/// The records a run will measure, and the ones an existing scan already
/// covers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub wanted: Vec<Group>,
    pub skipped: Vec<Target>,
}

/// The album a file belongs to, or `None` when its tags name none.
///
/// Compared through [`flatten`], not `normalize`, which would merge "Ghost
/// Rule (Deluxe Edition)" into "Ghost Rule".
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

/// Gather files into the records they belong to, keeping the order they came in.
///
/// With `album_gain` off (`--track-only`), every file is its own group.
#[must_use]
pub fn group(targets: Vec<Target>, album_gain: bool) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut seen: HashMap<(String, String), usize> = HashMap::new();

    for target in targets {
        let album = album_gain.then(|| album_key(&target.tags)).flatten();
        if let Some(&at) = album.as_ref().and_then(|key| seen.get(key)) {
            groups[at].targets.push(target);
            continue;
        }
        if let Some(key) = album.clone() {
            seen.insert(key, groups.len());
        }
        groups.push(Group {
            album,
            targets: vec![target],
        });
    }
    groups
}

/// How many files the library holds for each record it knows about.
#[must_use]
pub fn album_counts(entries: &[LibraryEntry]) -> HashMap<(String, String), usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        if let Some(key) = album_key(&entry.tags) {
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    counts
}

/// The records this run holds only part of.
///
/// These get no album gain, which would describe only the tracks present.
/// A record the index doesn't know is taken as complete.
#[must_use]
pub fn partial_records<S: BuildHasher>(
    groups: &[Group],
    known: &HashMap<(String, String), usize, S>,
) -> HashSet<(String, String)> {
    groups
        .iter()
        .filter_map(|group| {
            let key = group.album.as_ref()?;
            let held = known.get(key).copied().unwrap_or(0);
            (group.targets.len() < held).then(|| key.clone())
        })
        .collect()
}

/// Decide what to measure, never overwriting a scan somebody else already ran.
///
/// A fully scanned record is left alone; one with any gap is measured entire.
#[must_use]
pub fn select(groups: Vec<Group>, force: bool) -> Selection {
    let mut selection = Selection::default();
    for group in groups {
        if force
            || !group
                .targets
                .iter()
                .all(|target| target.current.is_scanned())
        {
            selection.wanted.push(group);
        } else {
            selection.skipped.extend(group.targets);
        }
    }
    selection
}

/// Work out the tags every measured file would carry.
///
/// A file the scan could not measure yields nothing.
#[must_use]
pub fn plan(scanned: &[Scanned], settings: &LoudnessSettings) -> Vec<GainChange> {
    scanned
        .iter()
        .flat_map(|group| {
            group.tracks.iter().filter_map(|(target, loudness)| {
                let track_gain = gain_db((*loudness)?.integrated_lufs, settings.reference_lufs);
                let album = group.album.map(|album| {
                    (
                        gain_db(album.integrated_lufs, settings.reference_lufs),
                        album,
                    )
                });

                let mut proposed = ReplayGainTags {
                    track_gain: Some(format_gain(track_gain)),
                    track_peak: Some(format_peak((*loudness)?.peak)),
                    album_gain: album.map(|(gain, _)| format_gain(gain)),
                    album_peak: album.map(|(_, album)| format_peak(album.peak)),
                    r128_track_gain: None,
                    r128_album_gain: None,
                };
                if target.r128 {
                    proposed.r128_track_gain =
                        Some(format_r128(track_gain, settings.reference_lufs));
                    proposed.r128_album_gain =
                        album.map(|(gain, _)| format_r128(gain, settings.reference_lufs));
                }

                Some(GainChange {
                    path: target.path.clone(),
                    current: target.current.clone(),
                    proposed,
                })
            })
        })
        .collect()
}

/// How far a player has to move a file to reach the reference level.
#[must_use]
pub fn gain_db(integrated_lufs: f64, reference_lufs: f64) -> f64 {
    reference_lufs - integrated_lufs
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
/// Saturates rather than wrapping, which would flip the sign of the gain.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn format_r128(gain_db: f64, reference_lufs: f64) -> String {
    let against_r128 = gain_db + (R128_REFERENCE_LUFS - reference_lufs);
    let steps = (against_r128 * 256.0).round();
    let clamped = steps.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
    clamped.to_string()
}
