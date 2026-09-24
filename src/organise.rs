//! Laying files out on disk from what their tags say.
//!
//! For a tool whose pitch is "keep it clean", nothing here touched file layout
//! (#3). Downloads land as `<download dir>/<Artist>/<whatever the peer called
//! it>` -- often `01 track.flac` or worse -- and there was no way to get from
//! there to `Artist/Album/01 - Title.flac`.
//!
//! Planning is pure: where a file belongs, and what stands in the way, are
//! decided from indexed entries alone, so the pattern language is tested without
//! moving anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::OrganiseSettings;
use crate::errors::{Error, Result};
use crate::library::changes::ChangeLog;
use crate::library::index::{describe, LibraryIndex};
use crate::library::scan::{iter_audio_files, owning_root, Excludes};
use crate::models::LibraryEntry;
use crate::text::normalize::{sanitize_folder_name, UNKNOWN_ARTIST};

/// A field a pattern can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placeholder {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Track,
}

impl Placeholder {
    const ALL: [(&'static str, Self); 5] = [
        ("artist", Self::Artist),
        ("album_artist", Self::AlbumArtist),
        ("album", Self::Album),
        ("title", Self::Title),
        ("track", Self::Track),
    ];

    fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|(spelling, _)| *spelling == name)
            .map(|(_, placeholder)| placeholder)
    }

    /// The value for one file.
    ///
    /// `artist` and `title` are the resolved forms the index already derived --
    /// tags, then the directory layout, then the filename -- so the fallback
    /// rule is reused rather than restated here. An absent artist becomes
    /// `Unknown Artist`, which is what `fetch` already files one under.
    fn value(self, entry: &LibraryEntry, settings: &OrganiseSettings) -> String {
        match self {
            Self::Artist => artist_of(entry),
            Self::AlbumArtist => {
                if entry.tags.album_artist.is_empty() {
                    artist_of(entry)
                } else {
                    entry.tags.album_artist.clone()
                }
            }
            Self::Album => entry.tags.album.clone(),
            Self::Title => entry.title.raw.clone(),
            Self::Track => entry.tags.track_number.map_or_else(String::new, |number| {
                format!("{number:0width$}", width = settings.track_number_width)
            }),
        }
    }
}

fn artist_of(entry: &LibraryEntry) -> String {
    if entry.artist.raw.is_empty() {
        UNKNOWN_ARTIST.to_owned()
    } else {
        entry.artist.raw.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Field(Placeholder),
}

/// A validated layout: every placeholder is one this knows, and the shape is
/// one that cannot escape the root it is joined to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    components: Vec<Vec<Segment>>,
}

impl std::str::FromStr for Pattern {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        Self::parse(text)
    }
}

impl Pattern {
    /// Parse a pattern, naming whatever in it is not one.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = |detail: &str| Err(Error::Config(format!("organise.pattern: {detail}")));

        if text.trim().is_empty() {
            return bad("must not be empty");
        }
        if text.contains('\\') {
            return bad("separates directories with '/', on every platform");
        }
        if text.starts_with('/') || text.contains("..") || text.contains(':') {
            return bad("must be relative to the library root");
        }

        let mut components = Vec::new();
        for part in text.split('/') {
            if part.trim().is_empty() {
                return bad("has an empty directory in it");
            }
            components.push(parse_component(part)?);
        }

        let named: Vec<Placeholder> = components
            .iter()
            .flatten()
            .filter_map(|segment| match segment {
                Segment::Field(placeholder) => Some(*placeholder),
                Segment::Literal(_) => None,
            })
            .collect();
        if !named.contains(&Placeholder::Title) {
            // Without it every track of an album renders to one name, and the
            // collision rule refuses the lot. Better said now than per file.
            return bad("must contain {title}, or every track of an album collides");
        }
        Ok(Self { components })
    }
}

fn parse_component(text: &str) -> Result<Vec<Segment>> {
    let mut segments = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            segments.push(Segment::Literal(rest[..open].to_owned()));
        }
        let close = rest[open..]
            .find('}')
            .map(|end| open + end)
            .ok_or_else(|| {
                Error::Config("organise.pattern: has a '{' with no matching '}'".to_owned())
            })?;
        let name = &rest[open + 1..close];
        let placeholder = Placeholder::parse(name).ok_or_else(|| {
            Error::Config(format!(
                "organise.pattern: unknown placeholder {{{name}}}; known: {}",
                Placeholder::ALL
                    .iter()
                    .map(|(spelling, _)| format!("{{{spelling}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        segments.push(Segment::Field(placeholder));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Literal(rest.to_owned()));
    }
    Ok(segments)
}

/// Characters left stranded when a placeholder renders empty.
const STRANDED: &[char] = &['-', '_', '.', '(', ')', '[', ']', ' '];

/// Render one component, dropping what an absent field leaves behind.
///
/// An empty placeholder takes the literal next to it with it, so
/// `{track} - {title}` with no track number is `Title` and not ` - Title`. A
/// component whose placeholders were all empty renders empty, and the caller
/// decides what that means for a directory and for a filename.
fn render_component(
    segments: &[Segment],
    entry: &LibraryEntry,
    settings: &OrganiseSettings,
) -> String {
    let mut pieces: Vec<String> = Vec::new();
    let mut last_was_literal = false;
    let mut skip_literal = false;

    for (index, segment) in segments.iter().enumerate() {
        match segment {
            Segment::Literal(text) => {
                if skip_literal {
                    skip_literal = false;
                    continue;
                }
                pieces.push(text.clone());
                last_was_literal = true;
            }
            Segment::Field(placeholder) => {
                let value = placeholder.value(entry, settings);
                if value.is_empty() {
                    // Only a literal can be taken away, and only the one that
                    // actually follows: `index + 1 < len` would leave the flag
                    // set across a second placeholder and eat someone else's
                    // separator two segments later.
                    if matches!(segments.get(index + 1), Some(Segment::Literal(_))) {
                        skip_literal = true;
                    } else if last_was_literal {
                        pieces.pop();
                        last_was_literal = false;
                    }
                } else {
                    pieces.push(value);
                    last_was_literal = false;
                }
            }
        }
    }

    let joined = pieces.concat();
    joined
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|character| STRANDED.contains(&character))
        .to_owned()
}

/// Where a file belongs under `root`, or `None` when it cannot be named.
#[must_use]
pub fn render(
    pattern: &Pattern,
    entry: &LibraryEntry,
    settings: &OrganiseSettings,
) -> Option<PathBuf> {
    let (last, directories) = pattern.components.split_last()?;

    let mut relative = PathBuf::new();
    for segments in directories {
        let rendered = render_component(segments, entry, settings);
        // An empty directory is dropped rather than filled in: inventing
        // "Unknown Album" folders is the mess this command exists to remove.
        if !rendered.is_empty() {
            relative.push(sanitize_folder_name(&rendered, UNKNOWN_ARTIST));
        }
    }

    // Never invent a filename. A file with nothing to call it is left alone.
    let stem = render_component(last, entry, settings);
    if stem.is_empty() {
        return None;
    }
    let stem = sanitize_folder_name(&stem, UNKNOWN_ARTIST);

    let name = match entry.path.extension().and_then(|suffix| suffix.to_str()) {
        Some(suffix) => format!("{stem}.{suffix}"),
        None => stem,
    };
    Some(relative.join(name))
}

/// Why a planned move cannot be carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Obstacle {
    AlreadyInPlace,
    Unnameable,
    NoRoot,
    Occupied(PathBuf),
    /// Claimed by an earlier move in this same run.
    Claimed(PathBuf),
}

impl std::fmt::Display for Obstacle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyInPlace => formatter.write_str("already in place"),
            Self::Unnameable => formatter.write_str("nothing to name it after"),
            Self::NoRoot => formatter.write_str("outside every configured root"),
            Self::Occupied(path) => write!(formatter, "{} already exists", path.display()),
            Self::Claimed(path) => {
                write!(formatter, "{} is claimed by another move", path.display())
            }
        }
    }
}

/// Where one file should sit, and what stands in the way.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedMove {
    pub entry: LibraryEntry,
    pub destination: PathBuf,
    pub blocked: Option<Obstacle>,
}

impl PlannedMove {
    #[must_use]
    pub fn ready(&self) -> bool {
        self.blocked.is_none()
    }
}

/// Work out where every file should go, and which moves collide.
///
/// `destination` consolidates everything into one place. Left unset, each file
/// stays under the root it is already in: with several roots configured, moving
/// a file between them is a decision nobody asked this command to make.
#[must_use]
pub fn plan_moves(
    entries: &[LibraryEntry],
    pattern: &Pattern,
    settings: &OrganiseSettings,
    roots: &[PathBuf],
    destination: Option<&Path>,
) -> Vec<PlannedMove> {
    let mut claimed: HashSet<String> = HashSet::new();

    entries
        .iter()
        .map(|entry| {
            let blocked = |destination: PathBuf, obstacle| PlannedMove {
                entry: entry.clone(),
                destination,
                blocked: Some(obstacle),
            };

            let Some(root) = destination.or_else(|| owning_root(&entry.path, roots)) else {
                return blocked(entry.path.clone(), Obstacle::NoRoot);
            };
            let Some(relative) = render(pattern, entry, settings) else {
                return blocked(entry.path.clone(), Obstacle::Unnameable);
            };

            let target = root.join(relative);
            if same_path(&target, &entry.path) {
                return blocked(target, Obstacle::AlreadyInPlace);
            }
            // Checked before anything is written, so the plan is truthful about
            // two files that render to one name.
            if !claimed.insert(comparable(&target)) {
                return blocked(target.clone(), Obstacle::Claimed(target));
            }
            // A destination that is the same file under another spelling is a
            // case-only rename, not something in the way: on Windows `exists`
            // cannot tell those apart, and refusing would block the one rename
            // most worth making.
            if !same_file(&target, &entry.path) && target.exists() {
                return blocked(target.clone(), Obstacle::Occupied(target));
            }

            PlannedMove {
                entry: entry.clone(),
                destination: target,
                blocked: None,
            }
        })
        .collect()
}

/// Files under a directory that is not part of the library.
///
/// Described rather than looked up: nothing outside a root is in the index, and
/// the flat `<download dir>/<Artist>/<peer's filename>` layout a fetch leaves
/// behind is exactly what the index has not seen.
pub fn loose_entries(
    source: &Path,
    extensions: &[&str],
    exclude: &Excludes,
) -> Result<Vec<LibraryEntry>> {
    if !source.is_dir() {
        return Err(Error::Config(format!(
            "not a directory: {}",
            source.display()
        )));
    }
    Ok(iter_audio_files(source, extensions, exclude)
        .iter()
        .filter_map(|path| describe(path, source).ok())
        .collect())
}

/// What carrying out a plan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub moved: usize,
    pub pruned: usize,
    /// The file that stayed where it was, and why.
    pub failures: Vec<(PathBuf, String)>,
}

/// Carry out the moves that are ready, logging each under one `organise` run.
pub fn apply_moves(
    index: &mut LibraryIndex,
    ready: &[&PlannedMove],
    roots: &[PathBuf],
    settings: &OrganiseSettings,
    now: f64,
) -> Result<Applied> {
    let log = ChangeLog::open(index, "organise", now)?;
    let mut applied = Applied::default();
    for planned in ready {
        let from = &planned.entry.path;
        // Logged before the rename: a row for a move that did not happen is
        // refused by `changes undo` as a file that is gone, where a move with no
        // row is a hole in the record.
        log.moved(index, from, &planned.destination)?;
        match move_file(from, &planned.destination) {
            Ok(()) => {
                let landed = spelled_on_disk(&planned.destination);
                index.relocate(from, &landed, roots)?;
                applied.moved += 1;
                if settings.remove_empty_directories {
                    if let Some(root) = owning_root(from, roots) {
                        applied.pruned += prune_empty(from, root).len();
                    }
                }
            }
            Err(error) => applied.failures.push((from.clone(), error.to_string())),
        }
    }
    Ok(applied)
}

/// Whether two paths are spelled the same, separators aside.
///
/// Compared component by component rather than as strings, because `a/b` and
/// `a` are one path on Windows and two different strings.
#[must_use]
pub fn same_path(left: &Path, right: &Path) -> bool {
    spelling(left) == spelling(right)
}

/// Whether two paths reach the same file.
///
/// On Windows and macOS, `Foo.flac` and `foo.flac` are one file. That makes it
/// different from [`same_path`], and the difference is the point: a case-only
/// rename is two spellings of one file, so it is a real change to make and not
/// a collision to refuse.
///
/// On Unix the filesystem is asked when both exist, because case sensitivity
/// belongs to the volume rather than the platform: APFS can be formatted
/// case-sensitive, and an exFAT drive mounted on Linux is not.
#[must_use]
pub fn same_file(left: &Path, right: &Path) -> bool {
    match (identity(left), identity(right)) {
        (Some(left), Some(right)) => left == right,
        _ => comparable(left) == comparable(right),
    }
}

#[cfg(unix)]
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn spelling(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// A path spelled the way the platform's default filesystem tells names apart.
///
/// Folding where the volume turns out to be case-sensitive only costs a
/// refused move; not folding where it is insensitive lets a second rename
/// overwrite the first.
fn comparable(path: &Path) -> String {
    let text = spelling(path);
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text
    }
}

/// Move one file, creating the directories it needs.
///
/// A rename across volumes is copied and then removed: the download directory
/// and the library routinely sit on different drives, and `fs::rename` cannot
/// cross one. The copy is removed again if anything after it fails, so a
/// failure never leaves two files.
pub fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::Config(format!("{}: {error}", parent.display())))?;
    }

    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if crosses_devices(&error) => copy_across(from, to),
        Err(error) => Err(Error::Config(format!(
            "{} -> {}: {error}",
            from.display(),
            to.display()
        ))),
    }
}

/// `EXDEV` on Unix, `ERROR_NOT_SAME_DEVICE` on Windows.
fn crosses_devices(error: &std::io::Error) -> bool {
    let across = if cfg!(windows) { 17 } else { 18 };
    error.raw_os_error() == Some(across)
}

fn copy_across(from: &Path, to: &Path) -> Result<()> {
    let failed = |error: std::io::Error| {
        // Leaving half a file behind would be worse than not moving at all.
        let _ = std::fs::remove_file(to);
        Error::Config(format!("{} -> {}: {error}", from.display(), to.display()))
    };

    let copied = std::fs::copy(from, to).map_err(failed)?;
    let size = from.metadata().map(|data| data.len()).map_err(failed)?;
    if copied != size {
        let _ = std::fs::remove_file(to);
        return Err(Error::Config(format!(
            "{} -> {}: copied {copied} of {size} bytes",
            from.display(),
            to.display()
        )));
    }
    std::fs::remove_file(from).map_err(failed)
}

/// Remove the directories a move emptied, up to but never including `root`.
///
/// `remove_dir` refuses a directory that is not empty, which is the guard: a
/// folder still holding `cover.jpg` is not empty and is correctly left alone.
/// Nothing is deleted to make a directory empty -- `--apply` on a command that
/// moves files did not consent to removing anyone's cover art.
pub fn prune_empty(from: &Path, root: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let mut directory = from.parent().map(Path::to_path_buf);

    while let Some(candidate) = directory {
        if same_file(&candidate, root) || !candidate.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(&candidate).is_err() {
            break;
        }
        directory = candidate.parent().map(Path::to_path_buf);
        removed.push(candidate);
    }
    removed
}

/// The path as the filesystem spells it, which need not be what was asked for.
///
/// `create_dir_all("zun")` silently reuses an existing `ZUN` on Windows, so a
/// key built from the requested path would claim a directory the file is not
/// in. The index stores paths verbatim, so it has to be told the real one.
#[must_use]
pub fn spelled_on_disk(path: &Path) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path.to_path_buf();
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return path.to_path_buf();
    };

    entries
        .filter_map(std::result::Result::ok)
        .find(|entry| entry.file_name().eq_ignore_ascii_case(name))
        .map_or_else(
            || path.to_path_buf(),
            |entry| parent.join(entry.file_name()),
        )
}
