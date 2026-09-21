//! The write log: what `--apply` changed, and how to put it back.
//!
//! The project's headline invariant is that nothing is written without
//! `--apply`. That said nothing about what happened once you did pass it: a
//! `genre apply` over two hundred files with a bad map entry, or an `art fetch`
//! that embedded the wrong artist's cover, left no record and no way back (#2).
//!
//! Every mutation path writes a row here, and `stk changes undo` reads it back.
//! A half-covered log would be worse than none, since it implies a completeness
//! it does not have, so the rule is that a command which writes to a user's
//! files logs what it wrote.
//!
//! Text values are stored inline. Artwork is not: a library-wide `art fetch`
//! replaces covers of 100 KB to 1 MB each, so the row keeps a SHA-256 and the
//! bytes go to `<index dir>/changes/<sha256>.bin`. Content addressing is what
//! makes that affordable -- an album's twelve tracks usually share one replaced
//! cover, and therefore one sidecar.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sha2::{Digest, Sha256};

use crate::errors::{Error, Result};
use crate::library::index::LibraryIndex;
use crate::library::tags::{self, TagField};
use crate::models::{Artwork, ReplayGainTags, TrackTags};

/// What a logged row changed about a file.
///
/// One variant per field rather than a single "tags" row carrying all of them.
/// [`plan_undo`] decides whether a file moved since it was written by comparing
/// that field's current value against what was logged, and a row covering every
/// field would read a genre edited yesterday as a reason to refuse to put a
/// title back -- then, under `--force`, revert the genre as well. An undo must
/// never write wider than the row it is undoing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeField {
    Genre,
    Artwork,
    /// Every loudness figure at once, unlike the tag fields below.
    ///
    /// The four ReplayGain values are one measurement, not four decisions. A
    /// row apiece would let an undo put a gain back without the peak that
    /// bounds it, leaving a player applying positive gain nothing to check
    /// itself against -- the opposite of what splitting genre from title
    /// protects (#7).
    ReplayGain,
    Artist,
    AlbumArtist,
    Album,
    Title,
    TrackNumber,
    /// Where the file is, rather than what it says.
    Move,
}

impl ChangeField {
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
        }
    }

    /// How a logged tag field is written back.
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
        [
            Self::Genre,
            Self::Artwork,
            Self::ReplayGain,
            Self::Artist,
            Self::AlbumArtist,
            Self::Album,
            Self::Title,
            Self::TrackNumber,
            Self::Move,
        ]
        .into_iter()
        .find(|field| field.as_str() == text)
        .ok_or_else(|| format!("unknown change field: {text}"))
    }
}

/// One logged write.
///
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

/// A row on its way into the log.
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

/// Which rows `stk changes list` wants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeFilter {
    pub limit: Option<usize>,
    pub command: Option<String>,
    pub run: Option<i64>,
}

/// One `--apply` invocation's worth of rows, sharing a run so an undo can take
/// the whole command back rather than one file of it.
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

    /// Log one tag field of one file, as `tags apply` writes them.
    ///
    /// Genres keep their own encoding whichever command wrote them, so a row
    /// logged by `genre apply` and one logged here read back the same way.
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
            // Always present, empty meaning the file carried nothing there, so
            // the comparison `plan_undo` makes stays total.
            old_value: Some(field.value(old)),
            new_value: Some(field.value(new)),
        })
    }

    /// Log a file move, keyed on where the file has ended up.
    ///
    /// `path` is the destination, not the source. [`plan_undo`] opens by asking
    /// whether the file it is about to put back is still where the log left it,
    /// and a row keyed on the source would be blocked as gone the moment the
    /// move succeeded. It is also the column `stk changes list` prints, which
    /// should say where the file actually is.
    ///
    /// Written *before* the rename, not after. A row for a rename that then
    /// failed is refused by `plan_undo` as a file that is gone -- detectable and
    /// inert. A rename with no row is the gap in the record the log exists to
    /// close.
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

    /// Log a whole loudness measurement, as `stk loudness` writes them.
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
            // Always present, an empty set meaning the file had never been
            // scanned, so the comparison `plan_undo` makes stays total.
            old_value: Some(encode_replay_gain(old)),
            new_value: Some(encode_replay_gain(new)),
        })
    }

    /// `new` is absent when the cover was removed rather than replaced, which
    /// is what undoing a first-time embed comes to.
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

/// Why a logged change cannot be put back as things stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    FileGone,
    FileChanged,
    SidecarGone(PathBuf),
    UnknownField(String),
    /// Something else now sits where a moved file came from (#3).
    SourceOccupied(PathBuf),
}

impl Blocked {
    /// Whether `--force` may go ahead anyway.
    ///
    /// A file edited since the row was written is a judgement call, and the
    /// user's to make. The rest are missing pieces no flag can conjure back --
    /// and `SourceOccupied` risks destroying a *different* file, which is not
    /// something a flag should buy.
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

/// What undoing one row would do, and what stands in the way.
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

    /// Whether this can be carried out, given whether `--force` was passed.
    #[must_use]
    pub fn allowed(&self, force: bool) -> bool {
        match &self.blocked {
            None => true,
            Some(blocked) => force && blocked.forceable(),
        }
    }
}

/// Work out what putting one change back would mean.
///
/// A file edited since the row was written is reported rather than overwritten:
/// restoring a two-month-old genre over a tag someone has since fixed by hand is
/// exactly the kind of surprise an undo exists to prevent.
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
    };

    let blocked = if current == change.new_value {
        // A move's current value is its own location, so the comparison above
        // can never catch anything: the hazard is that the place it came from
        // is no longer free.
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

/// Put one change back, and log the undo as a change of its own.
///
/// The log stays a complete account of what this tool wrote, which is the whole
/// reason to keep one -- an undo that left no trace would be the same gap in the
/// record that #2 opened this against.
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
            // Keyed on the spelling the filesystem used, not the one the log
            // asked for: `create_dir_all("zun")` reuses an existing `ZUN` on
            // Windows, so the index row may be filed under a different case and
            // a delete on the requested path would miss it.
            index.relocate(
                &crate::organise::spelled_on_disk(&change.path),
                &back,
                roots,
            )?;
            return Ok(());
        }
    }

    index.reindex(&change.path, roots)?;
    Ok(())
}

/// What undoing one tag field would read and what it would say.
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

/// Put one tag field back, leaving the file's other fields where they are.
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

/// The command name an undo logs itself under.
pub const UNDO_COMMAND: &str = "changes undo";

/// Store bytes beside the index and return the SHA-256 naming them.
pub fn store_blob(index: &LibraryIndex, bytes: &[u8]) -> Result<String> {
    let sha = digest(bytes);
    let directory = sidecar_directory(index);
    std::fs::create_dir_all(&directory)
        .map_err(|error| Error::Config(format!("{}: {error}", directory.display())))?;

    let destination = directory.join(format!("{sha}.bin"));
    // Content addressing means an existing file already holds these bytes.
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

/// How a genre list is stored, so a value round-trips through the log.
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

/// How a measurement is stored, so every figure round-trips through the log.
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

/// Whether something else has taken the place a moved file came from.
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
