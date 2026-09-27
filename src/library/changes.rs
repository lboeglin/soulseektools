//! The write log: what `--apply` changed, and how to put it back.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sha2::{Digest, Sha256};

use crate::errors::{Error, Result};
use crate::library::index::LibraryIndex;
use crate::library::tags::{self, Extra, TagField};
use crate::models::{Artwork, ReplayGainTags, TrackTags};

/// One variant per field, so undoing a title never checks or reverts a genre edited since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeField {
    Genre,
    Artwork,
    /// Every loudness figure at once: a gain is never restored without its peak.
    ReplayGain,
    Artist,
    AlbumArtist,
    Album,
    Title,
    TrackNumber,
    Move,
    Extras,
}

impl ChangeField {
    pub const ALL: [Self; 10] = [
        Self::Genre,
        Self::Artwork,
        Self::ReplayGain,
        Self::Artist,
        Self::AlbumArtist,
        Self::Album,
        Self::Title,
        Self::TrackNumber,
        Self::Move,
        Self::Extras,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Genre => "genre",
            Self::Artwork => "artwork",
            Self::ReplayGain => "replay_gain",
            Self::Artist => "artist",
            Self::AlbumArtist => "album_artist",
            Self::Album => "album",
            Self::Title => "title",
            Self::TrackNumber => "track_number",
            Self::Move => "move",
            Self::Extras => "extras",
        }
    }

    #[must_use]
    pub fn of(field: TagField) -> Self {
        match field {
            TagField::Genres => Self::Genre,
            TagField::Artist => Self::Artist,
            TagField::AlbumArtist => Self::AlbumArtist,
            TagField::Album => Self::Album,
            TagField::Title => Self::Title,
            TagField::TrackNumber => Self::TrackNumber,
        }
    }
}

impl fmt::Display for ChangeField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ChangeField {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|field| field.as_str() == text)
            .ok_or_else(|| format!("unknown change field: {text}"))
    }
}

/// `old_value` is absent when the field was: no genre tag, no embedded cover.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub id: i64,
    pub run_id: i64,
    pub applied_at: f64,
    pub command: String,
    pub path: PathBuf,
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewChange {
    pub run_id: i64,
    pub applied_at: f64,
    pub command: String,
    pub path: PathBuf,
    pub field: ChangeField,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeFilter {
    pub limit: Option<usize>,
    pub command: Option<String>,
    pub run: Option<i64>,
}

pub struct ChangeLog {
    run_id: i64,
    command: String,
    applied_at: f64,
}

impl ChangeLog {
    pub fn open(index: &LibraryIndex, command: &str, applied_at: f64) -> Result<Self> {
        Ok(Self {
            run_id: index.next_run_id()?,
            command: command.to_owned(),
            applied_at,
        })
    }

    #[must_use]
    pub fn run_id(&self) -> i64 {
        self.run_id
    }

    pub fn genre(
        &self,
        index: &mut LibraryIndex,
        path: &Path,
        old: &[String],
        new: &[String],
    ) -> Result<i64> {
        index.record_change(&NewChange {
            run_id: self.run_id,
            applied_at: self.applied_at,
            command: self.command.clone(),
            path: path.to_path_buf(),
            field: ChangeField::Genre,
            old_value: Some(encode_genres(old)),
            new_value: Some(encode_genres(new)),
        })
    }

    pub fn tag(
        &self,
        index: &mut LibraryIndex,
        path: &Path,
        field: TagField,
        old: &TrackTags,
        new: &TrackTags,
    ) -> Result<i64> {
        if field == TagField::Genres {
            return self.genre(index, path, &old.genres, &new.genres);
        }
        index.record_change(&NewChange {
            run_id: self.run_id,
            applied_at: self.applied_at,
            command: self.command.clone(),
            path: path.to_path_buf(),
            field: ChangeField::of(field),
            // Always present (empty for nothing), so `plan_undo` can compare.
            old_value: Some(field.value(old)),
            new_value: Some(field.value(new)),
        })
    }

    /// Keyed on the destination and logged before the rename: a row for a failed rename is harmless.
    pub fn moved(&self, index: &mut LibraryIndex, from: &Path, to: &Path) -> Result<i64> {
        index.record_change(&NewChange {
            run_id: self.run_id,
            applied_at: self.applied_at,
            command: self.command.clone(),
            path: to.to_path_buf(),
            field: ChangeField::Move,
            old_value: Some(from.display().to_string()),
            new_value: Some(to.display().to_string()),
        })
    }

    pub fn replay_gain(
        &self,
        index: &mut LibraryIndex,
        path: &Path,
        old: &ReplayGainTags,
        new: &ReplayGainTags,
    ) -> Result<i64> {
        index.record_change(&NewChange {
            run_id: self.run_id,
            applied_at: self.applied_at,
            command: self.command.clone(),
            path: path.to_path_buf(),
            field: ChangeField::ReplayGain,
            // Always present (empty if never scanned), so `plan_undo` can compare.
            old_value: Some(encode_replay_gain(old)),
            new_value: Some(encode_replay_gain(new)),
        })
    }

    pub fn extras(
        &self,
        index: &mut LibraryIndex,
        path: &Path,
        old: &[Extra],
        new: &[Extra],
    ) -> Result<i64> {
        index.record_change(&NewChange {
            run_id: self.run_id,
            applied_at: self.applied_at,
            command: self.command.clone(),
            path: path.to_path_buf(),
            field: ChangeField::Extras,
            old_value: Some(encode_extras(old)),
            new_value: Some(encode_extras(new)),
        })
    }

    /// `new` is `None` when the cover was removed rather than replaced.
    pub fn artwork(
        &self,
        index: &mut LibraryIndex,
        path: &Path,
        old: Option<&Artwork>,
        new: Option<&Artwork>,
    ) -> Result<i64> {
        let old_value = match old {
            Some(artwork) => Some(store_blob(index, &artwork.data)?),
            None => None,
        };
        index.record_change(&NewChange {
            run_id: self.run_id,
            applied_at: self.applied_at,
            command: self.command.clone(),
            path: path.to_path_buf(),
            field: ChangeField::Artwork,
            old_value,
            // The new bytes are in the file; only the old ones need keeping.
            new_value: new.map(|artwork| digest(&artwork.data)),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    FileGone,
    FileChanged,
    SidecarGone(PathBuf),
    UnknownField(String),
    SourceOccupied(PathBuf),
}

impl Blocked {
    /// Whether `--force` may go ahead anyway: only for a file edited since.
    #[must_use]
    pub fn forceable(&self) -> bool {
        matches!(self, Self::FileChanged)
    }
}

impl fmt::Display for Blocked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileGone => formatter.write_str("the file is gone"),
            Self::FileChanged => formatter.write_str("the file has changed since it was logged"),
            Self::SidecarGone(path) => {
                write!(formatter, "the saved cover is gone from {}", path.display())
            }
            Self::UnknownField(field) => write!(formatter, "unknown change field: {field}"),
            Self::SourceOccupied(path) => {
                write!(formatter, "{} is not free any more", path.display())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UndoPlan {
    pub change: Change,
    pub description: String,
    pub blocked: Option<Blocked>,
}

impl UndoPlan {
    #[must_use]
    pub fn ready(&self) -> bool {
        self.blocked.is_none()
    }

    #[must_use]
    pub fn allowed(&self, force: bool) -> bool {
        match &self.blocked {
            None => true,
            Some(blocked) => force && blocked.forceable(),
        }
    }
}

/// A file edited since the row was written is blocked rather than overwritten.
pub fn plan_undo(index: &LibraryIndex, change: Change) -> Result<UndoPlan> {
    let Ok(field) = change.field.parse::<ChangeField>() else {
        let field = change.field.clone();
        return Ok(UndoPlan {
            description: "nothing to do".to_owned(),
            blocked: Some(Blocked::UnknownField(field)),
            change,
        });
    };

    if !change.path.is_file() {
        return Ok(UndoPlan {
            description: "nothing to do".to_owned(),
            blocked: Some(Blocked::FileGone),
            change,
        });
    }

    let (current, description) = match field {
        ChangeField::Genre => {
            let genres = tags::read_tags(&change.path)?.genres;
            let restored = decode_genres(change.old_value.as_deref());
            (
                Some(encode_genres(&genres)),
                if restored.is_empty() {
                    "remove the genre tag".to_owned()
                } else {
                    format!("set the genre back to {}", restored.join(", "))
                },
            )
        }
        ChangeField::Artwork => {
            let current = tags::read_artwork(&change.path)?.map(|art| digest(&art.data));
            (
                current,
                match &change.old_value {
                    Some(_) => "restore the previous cover".to_owned(),
                    None => "remove the embedded cover".to_owned(),
                },
            )
        }
        ChangeField::ReplayGain => {
            let restored = decode_replay_gain(change.old_value.as_deref());
            (
                Some(encode_replay_gain(&tags::read_replay_gain(&change.path)?)),
                if restored.is_empty() {
                    "remove the loudness tags".to_owned()
                } else {
                    format!(
                        "set the track gain back to {}",
                        restored.track_gain.clone().unwrap_or_default()
                    )
                },
            )
        }
        ChangeField::Artist => planned_tag(&change, TagField::Artist)?,
        ChangeField::AlbumArtist => planned_tag(&change, TagField::AlbumArtist)?,
        ChangeField::Album => planned_tag(&change, TagField::Album)?,
        ChangeField::Title => planned_tag(&change, TagField::Title)?,
        ChangeField::TrackNumber => planned_tag(&change, TagField::TrackNumber)?,
        ChangeField::Move => {
            let back = PathBuf::from(change.old_value.clone().unwrap_or_default());
            (
                Some(change.path.display().to_string()),
                format!("move it back to {}", back.display()),
            )
        }
        ChangeField::Extras => {
            let restored = decode_extras(change.old_value.as_deref());
            (
                Some(encode_extras(&tags::read_extras(&change.path)?)),
                if restored.is_empty() {
                    "remove the extra tags".to_owned()
                } else {
                    format!("put back the {} dropped tag(s)", restored.len())
                },
            )
        }
    };

    let blocked = if current == change.new_value {
        // For a move, the hazard is that the original location is taken.
        match field {
            ChangeField::Move => source_occupied(&change),
            _ => missing_sidecar(index, field, change.old_value.as_deref()),
        }
    } else {
        Some(Blocked::FileChanged)
    };

    Ok(UndoPlan {
        change,
        description,
        blocked,
    })
}

pub fn apply_undo(
    index: &mut LibraryIndex,
    plan: &UndoPlan,
    log: &ChangeLog,
    roots: &[PathBuf],
) -> Result<()> {
    let change = &plan.change;
    let field: ChangeField = change
        .field
        .parse()
        .map_err(|reason: String| Error::Config(reason))?;

    match field {
        ChangeField::Genre => {
            let restored = decode_genres(change.old_value.as_deref());
            let current = tags::read_tags(&change.path)?.genres;
            tags::write_genres(&change.path, &restored)?;
            log.genre(index, &change.path, &current, &restored)?;
        }
        ChangeField::Artwork => {
            let current = tags::read_artwork(&change.path)?;
            let restored = match &change.old_value {
                Some(sha) => Some(Artwork {
                    data: load_blob(index, sha)?,
                    mime: "image/jpeg".to_owned(),
                }),
                None => None,
            };
            match &restored {
                Some(artwork) => tags::write_artwork(&change.path, artwork)?,
                None => tags::clear_artwork(&change.path)?,
            }
            log.artwork(index, &change.path, current.as_ref(), restored.as_ref())?;
        }
        ChangeField::ReplayGain => {
            let current = tags::read_replay_gain(&change.path)?;
            let restored = decode_replay_gain(change.old_value.as_deref());
            tags::write_replay_gain(&change.path, &restored)?;
            log.replay_gain(index, &change.path, &current, &restored)?;
        }
        ChangeField::Artist => restore_tag(index, change, log, TagField::Artist)?,
        ChangeField::AlbumArtist => restore_tag(index, change, log, TagField::AlbumArtist)?,
        ChangeField::Album => restore_tag(index, change, log, TagField::Album)?,
        ChangeField::Title => restore_tag(index, change, log, TagField::Title)?,
        ChangeField::TrackNumber => restore_tag(index, change, log, TagField::TrackNumber)?,
        ChangeField::Move => {
            let back = PathBuf::from(change.old_value.clone().unwrap_or_default());
            crate::organise::move_file(&change.path, &back)?;
            log.moved(index, &change.path, &back)?;
            // Keyed on the filesystem's spelling: on Windows `create_dir_all("zun")`
            // reuses an existing `ZUN`.
            index.relocate(
                &crate::organise::spelled_on_disk(&change.path),
                &back,
                roots,
            )?;
            // The folders the command created, or quarantined into, are left empty.
            let quarantine = crate::duplicates::quarantine_root(index);
            let boundary = crate::library::scan::owning_root(&change.path, roots)
                .map(Path::to_path_buf)
                .or_else(|| change.path.starts_with(&quarantine).then_some(quarantine));
            if let Some(boundary) = boundary {
                crate::organise::prune_empty(&change.path, &boundary);
            }
            return Ok(());
        }
        ChangeField::Extras => {
            let current = tags::read_extras(&change.path)?;
            let restored = decode_extras(change.old_value.as_deref());
            tags::write_extras(&change.path, &restored)?;
            log.extras(index, &change.path, &current, &restored)?;
        }
    }

    index.reindex(&change.path, roots)?;
    Ok(())
}

fn planned_tag(change: &Change, field: TagField) -> Result<(Option<String>, String)> {
    let current = field.value(&tags::read_tags(&change.path)?);
    let restored = change.old_value.clone().unwrap_or_default();
    let description = if restored.is_empty() {
        format!("remove the {field} tag")
    } else {
        format!("set the {field} back to {restored}")
    };
    Ok((Some(current), description))
}

fn restore_tag(
    index: &mut LibraryIndex,
    change: &Change,
    log: &ChangeLog,
    field: TagField,
) -> Result<()> {
    let before = tags::read_tags(&change.path)?;
    let mut restored = before.clone();
    field.set(
        &mut restored,
        change.old_value.as_deref().unwrap_or_default(),
    );

    tags::write_tags(&change.path, &restored)?;
    log.tag(index, &change.path, field, &before, &restored)?;
    Ok(())
}

pub const UNDO_COMMAND: &str = "changes undo";

pub fn store_blob(index: &LibraryIndex, bytes: &[u8]) -> Result<String> {
    let sha = digest(bytes);
    let directory = sidecar_directory(index);
    std::fs::create_dir_all(&directory)
        .map_err(|error| Error::Config(format!("{}: {error}", directory.display())))?;

    let destination = directory.join(format!("{sha}.bin"));
    if !destination.exists() {
        std::fs::write(&destination, bytes)
            .map_err(|error| Error::Config(format!("{}: {error}", destination.display())))?;
    }
    Ok(sha)
}

pub fn load_blob(index: &LibraryIndex, sha: &str) -> Result<Vec<u8>> {
    let path = sidecar_directory(index).join(format!("{sha}.bin"));
    std::fs::read(&path).map_err(|error| Error::Config(format!("{}: {error}", path.display())))
}

#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .fold(String::new(), |mut text, byte| {
            use fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[must_use]
pub fn encode_genres(genres: &[String]) -> String {
    serde_json::to_string(genres).unwrap_or_else(|_| "[]".to_owned())
}

#[must_use]
pub fn decode_genres(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default()
}

#[must_use]
pub fn encode_extras(extras: &[Extra]) -> String {
    serde_json::to_string(extras).unwrap_or_else(|_| "[]".to_owned())
}

#[must_use]
pub fn decode_extras(value: Option<&str>) -> Vec<Extra> {
    value
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default()
}

#[must_use]
pub fn encode_replay_gain(gain: &ReplayGainTags) -> String {
    serde_json::to_string(gain).unwrap_or_else(|_| "{}".to_owned())
}

#[must_use]
pub fn decode_replay_gain(value: Option<&str>) -> ReplayGainTags {
    value
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default()
}

fn source_occupied(change: &Change) -> Option<Blocked> {
    let back = PathBuf::from(change.old_value.clone().unwrap_or_default());
    (back.exists() && !crate::organise::same_file(&back, &change.path))
        .then_some(Blocked::SourceOccupied(back))
}

fn missing_sidecar(
    index: &LibraryIndex,
    field: ChangeField,
    old_value: Option<&str>,
) -> Option<Blocked> {
    let (ChangeField::Artwork, Some(sha)) = (field, old_value) else {
        return None;
    };
    let path = sidecar_directory(index).join(format!("{sha}.bin"));
    (!path.is_file()).then_some(Blocked::SidecarGone(path))
}

fn sidecar_directory(index: &LibraryIndex) -> PathBuf {
    index
        .database
        .parent()
        .unwrap_or(Path::new("."))
        .join("changes")
}
