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
//!
//! The grouping is also what a scan is decided and run in. `audio/loudness.rs`
//! measures a record as one programme, so the figures here are arithmetic over
//! what it already worked out rather than an average of the tracks.

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
/// One record, or one file that names none. A group is the unit throughout: an
/// album gain describes a whole record, so measuring part of one would produce
/// a figure derived from part of one.
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

/// Gather files into the records they belong to, keeping the order they came in.
///
/// `album_gain` off makes every file its own group, which is what
/// `--track-only` means: nothing is measured against anything else.
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
/// An album gain describes a whole record, so a run naming one track of a
/// twelve-track album has no album figure to write: one measured from that
/// track alone would tell a player the record is as loud as that one song. The
/// tracks these groups were built from decide it, against what the library
/// knows the record to hold.
///
/// A record the index has never heard of is taken at face value. That covers
/// both the run over the whole library, where a group is the record by
/// construction, and the folder nothing has scanned yet -- and an index that
/// disagrees with the disk is a reason to scan, not to refuse a figure.
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
/// A record every one of whose tracks is already scanned is left alone; one
/// with any gap is measured entire, because an album gain derived from half a
/// record is not an album gain.
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
/// A file the scan could not measure yields nothing: there is no figure to
/// write, and the caller reports why separately.
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
