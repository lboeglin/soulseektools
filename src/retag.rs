//! What a fetched file is tagged with: its review row, under the `fetch.tags` policy.

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
use crate::text::normalize::normalize;
use crate::wantlist::TRACK_NUMBER_COLUMNS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagPolicy {
    /// The row wins wherever it has a value; comments, URLs and lyrics are dropped.
    #[default]
    Replace,
    Fill,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowTags {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub track_number: Option<i64>,

    /// Whether `title` was read off the peer's own filename.
    pub title_guessed: bool,
}

impl RowTags {
    /// Last.fm's `track` column is the title, so it is skipped when it holds the title.
    #[must_use]
    pub fn of(track: &Track) -> Self {
        let title = track.title.trim();
        Self {
            artist: track.artist.trim().to_owned(),
            title: title.to_owned(),
            album: track.album.trim().to_owned(),
            track_number: track
                .source_value(TRACK_NUMBER_COLUMNS)
                .filter(|value| *value != title)
                .and_then(tags::leading_number),
            title_guessed: track.title_guessed,
        }
    }
}

/// Album artist and genres are never written, a blank never blanks, a guessed title only fills.
#[must_use]
pub fn plan(policy: TagPolicy, row: &RowTags, current: &TrackTags) -> TrackTags {
    let mut proposed = current.clone();
    let under = |policy, proposed_empty: bool| match policy {
        TagPolicy::Replace => true,
        TagPolicy::Fill => proposed_empty,
        TagPolicy::Keep => false,
    };
    let wins = |proposed_empty: bool| under(policy, proposed_empty);
    let title_policy = match (policy, row.title_guessed) {
        (TagPolicy::Replace, true) => TagPolicy::Fill,
        (policy, _) => policy,
    };

    let differs = |wanted: &str, current: &str| normalize(wanted) != normalize(current);
    if !row.artist.is_empty()
        && differs(&row.artist, &proposed.artist)
        && wins(proposed.artist.is_empty())
    {
        proposed.artist.clone_from(&row.artist);
    }
    if !row.title.is_empty()
        && differs(&row.title, &proposed.title)
        && under(title_policy, proposed.title.is_empty())
    {
        proposed.title.clone_from(&row.title);
    }
    if !row.album.is_empty()
        && differs(&row.album, &proposed.album)
        && wins(proposed.album.is_empty())
    {
        proposed.album.clone_from(&row.album);
    }
    if row.track_number.is_some() && wins(proposed.track_number.is_none()) {
        proposed.track_number = row.track_number;
    }
    proposed
}

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

/// The index is refreshed only for a file under a library root.
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
        let kept: Vec<tags::Extra> = extras
            .iter()
            .filter(|extra| !is_peer_noise(extra))
            .cloned()
            .collect();
        if kept.len() < extras.len() {
            tags::write_extras(path, &kept)?;
            log.extras(index, path, &extras, &kept)?;
            outcome.dropped = extras.len() - kept.len();
        }
    }
    Ok(())
}

/// What a peer leaves that isn't about the music: comments, lyrics and links.
/// Dates, catalogue numbers and MusicBrainz ids stay.
fn is_peer_noise(extra: &tags::Extra) -> bool {
    const NOISE: [&str; 9] = [
        "COMMENT",
        "DESCRIPTION",
        "LYRICS",
        "UNSYNCEDLYRICS",
        "COMM",
        "USLT",
        "WXXX",
        "\u{a9}CMT",
        "\u{a9}LYR",
    ];
    let key = extra.key.to_uppercase();
    extra.locator || NOISE.contains(&key.as_str()) || key.starts_with("WWW") || key.ends_with("URL")
}
