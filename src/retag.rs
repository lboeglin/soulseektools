//! What a fetched file is tagged with, and why the peer's block is not trusted.
//!
//! A download arrives carrying whatever its uploader's tagger left: nothing,
//! `Track 01` under `Unknown Artist`, or a full block with a URL in the album
//! field and a `ripped by` comment. The review row it was fetched for carries
//! the artist, title and album that were asked for, which is better than all
//! three -- and without it on the file, `organise`, `genre check`, `dupes` and
//! `art fetch` are blind to the download (#11).
//!
//! The policy is configuration rather than a flag, because it is a statement
//! about every download rather than about one run. Every write goes through
//! the change log, so `stk changes undo` restores the peer's block if the
//! replacement was the wrong call.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::errors::Result;
use crate::library::changes::ChangeLog;
use crate::library::index::LibraryIndex;
use crate::library::scan::owning_root;
use crate::library::tags::{self, TagField};
use crate::models::{Track, TrackTags};
use crate::wantlist::TRACK_NUMBER_COLUMNS;

/// How a download's tags meet the review row it was fetched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagPolicy {
    /// The row wins wherever it has a value, and whatever else the peer
    /// left -- comment, URL, lyrics -- is dropped.
    #[default]
    Replace,
    /// The row fills only what the peer left empty.
    Fill,
    /// The file stays as it arrived.
    Keep,
}

impl TagPolicy {
    pub const ALL: [Self; 3] = [Self::Replace, Self::Fill, Self::Keep];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Fill => "fill",
            Self::Keep => "keep",
        }
    }
}

impl fmt::Display for TagPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TagPolicy {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|policy| policy.as_str() == text)
            .ok_or_else(|| format!("unknown tag policy: {text}"))
    }
}

/// What the review row can say about the file: the fields a wantlist export
/// or an `stk get` query carries. Anything absent here is the peer's to keep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowTags {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub track_number: Option<i64>,
}

impl RowTags {
    /// The track number comes back out of the export's own columns, by the
    /// same header tolerance `wantlist::read_export` reads them with.
    #[must_use]
    pub fn of(track: &Track) -> Self {
        Self {
            artist: track.artist.trim().to_owned(),
            title: track.title.trim().to_owned(),
            album: track.album.trim().to_owned(),
            track_number: track
                .source_value(TRACK_NUMBER_COLUMNS)
                .and_then(tags::leading_number),
        }
    }
}

/// The tags the file should carry, given what it carries now.
///
/// Album artist and genres are never the row's to write: an export names
/// neither, and `genre apply` owns the second. An `stk get` row has no album,
/// and a title-only query no artist, so a blank in the row never blanks the
/// file under either policy.
#[must_use]
pub fn plan(policy: TagPolicy, row: &RowTags, current: &TrackTags) -> TrackTags {
    let mut proposed = current.clone();
    let wins = |proposed_empty: bool| match policy {
        TagPolicy::Replace => true,
        TagPolicy::Fill => proposed_empty,
        TagPolicy::Keep => false,
    };

    if !row.artist.is_empty() && wins(proposed.artist.is_empty()) {
        proposed.artist.clone_from(&row.artist);
    }
    if !row.title.is_empty() && wins(proposed.title.is_empty()) {
        proposed.title.clone_from(&row.title);
    }
    if !row.album.is_empty() && wins(proposed.album.is_empty()) {
        proposed.album.clone_from(&row.album);
    }
    if row.track_number.is_some() && wins(proposed.track_number.is_none()) {
        proposed.track_number = row.track_number;
    }
    proposed
}

/// What tagging one download came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retagged {
    pub policy: TagPolicy,
    pub before: TrackTags,
    pub after: TrackTags,
    /// The fields that changed, in the order a dump lists them.
    pub fields: Vec<TagField>,
    /// How many of the peer's extra items were dropped.
    pub dropped: usize,
    pub error: String,
}

impl Retagged {
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.fields.is_empty() || self.dropped > 0
    }
}

/// Tag one download from its review row, and log what was written.
///
/// A file that cannot be read or written is reported in the outcome rather
/// than raised: one odd container must not stop the rest of a collect. The
/// index is refreshed only for a file under a library root, since a download
/// directory outside every root has no row to bring up to date and must not
/// gain one the next scan will delete.
pub fn apply(
    path: &Path,
    policy: TagPolicy,
    track: &Track,
    log: &ChangeLog,
    index: &mut LibraryIndex,
    roots: &[PathBuf],
) -> Result<Option<Retagged>> {
    if policy == TagPolicy::Keep {
        return Ok(None);
    }

    let mut outcome = Retagged {
        policy,
        before: TrackTags::default(),
        after: TrackTags::default(),
        fields: Vec::new(),
        dropped: 0,
        error: String::new(),
    };
    match retag(path, policy, track, log, index, &mut outcome) {
        Ok(()) => {}
        Err(error) => outcome.error = error.to_string(),
    }
    if outcome.changed() && owning_root(path, roots).is_some() {
        index.reindex(path, roots)?;
    }
    Ok(Some(outcome))
}

fn retag(
    path: &Path,
    policy: TagPolicy,
    track: &Track,
    log: &ChangeLog,
    index: &mut LibraryIndex,
    outcome: &mut Retagged,
) -> Result<()> {
    outcome.before = tags::read_tags(path)?;
    outcome.after = plan(policy, &RowTags::of(track), &outcome.before);
    outcome.fields = TagField::ALL
        .into_iter()
        .filter(|field| field.value(&outcome.before) != field.value(&outcome.after))
        .collect();

    if !outcome.fields.is_empty() {
        tags::write_tags(path, &outcome.after)?;
        for field in &outcome.fields {
            log.tag(index, path, *field, &outcome.before, &outcome.after)?;
        }
    }

    if policy == TagPolicy::Replace {
        let extras = tags::read_extras(path)?;
        if !extras.is_empty() {
            tags::write_extras(path, &[])?;
            log.extras(index, path, &extras, &[])?;
            outcome.dropped = extras.len();
        }
    }
    Ok(())
}
