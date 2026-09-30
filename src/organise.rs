//! Laying files out on disk from what their tags say.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{OrganiseSettings, CREDIT_SEPARATORS};
use crate::errors::{Error, Result};
use crate::library::changes::ChangeLog;
use crate::library::index::{describe, LibraryIndex};
use crate::library::scan::{iter_audio_files, owning_root, Excludes};
use crate::matching::NameForms;
use crate::models::LibraryEntry;
use crate::text::normalize::{sanitize_folder_name, UNKNOWN_ARTIST};
use unicode_normalization::UnicodeNormalization;

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

    /// An absent artist becomes `Unknown Artist`, as `fetch` files it.
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

/// Known placeholders only, and a shape that cannot escape its root.
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
    /// Whether the first directory is the artist alone, so an existing artist
    /// folder can stand in for it.
    fn leads_with_artist(&self) -> bool {
        self.components.len() > 1
            && matches!(
                self.components[0].as_slice(),
                [Segment::Field(
                    Placeholder::Artist | Placeholder::AlbumArtist
                )]
            )
    }

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

/// An empty placeholder takes the next literal with it: `{track} - {title}` gives `Title`.
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
                    // Only the literal immediately after, never one further on.
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
        // Drop an empty directory rather than invent "Unknown Album".
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

fn with_artist_folder(relative: &Path, known: &ArtistFolders) -> PathBuf {
    let mut parts = relative.components();
    let Some(first) = parts.next() else {
        return relative.to_path_buf();
    };
    let artist = known.resolve(&first.as_os_str().to_string_lossy());
    Path::new(&artist).join(parts.as_path())
}

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

/// Without `destination`, each file stays under its own root.
#[must_use]
/// The artist folders already under a root, by how their names read, so a
/// file tagged `稲葉曇` joins an existing `Inabakumori`.
pub struct ArtistFolders {
    by_reading: HashMap<String, String>,
}

impl ArtistFolders {
    /// Aliases win over a folder that merely reads the same.
    pub fn read(root: &Path, aliases: &BTreeMap<String, String>) -> Self {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .map(|listing| {
                listing
                    .filter_map(std::result::Result::ok)
                    .filter(|item| item.path().is_dir())
                    .map(|item| item.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();

        let mut by_reading = HashMap::new();
        for name in names {
            let key = reading(&name);
            if !key.is_empty() {
                by_reading.entry(key).or_insert(name);
            }
        }
        for (spelling, folder) in aliases {
            let key = reading(spelling);
            if !key.is_empty() {
                by_reading.insert(key, folder.clone());
            }
        }
        Self { by_reading }
    }

    /// The folder for the whole credit, else for its lead artist; a new
    /// artist keeps the name it came with.
    #[must_use]
    pub fn resolve(&self, artist: &str) -> String {
        [artist, lead_artist(artist)]
            .iter()
            .find_map(|name| self.by_reading.get(&reading(name)))
            .cloned()
            .unwrap_or_else(|| artist.to_owned())
    }
}

fn lead_artist(credit: &str) -> &str {
    let lowered = credit.to_lowercase();
    let end = CREDIT_SEPARATORS
        .iter()
        .filter_map(|separator| lowered.find(separator))
        .min()
        .filter(|at| credit.is_char_boundary(*at))
        .unwrap_or(credit.len());
    credit[..end].trim()
}

/// Romaji without accents, spaces or punctuation: `稲葉曇`, `Inabakumori` and
/// `INABAKUMORI` all read `inabakumori`, `Jäger` reads `jager`.
fn reading(name: &str) -> String {
    NameForms::of(name)
        .romaji
        .nfkd()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

#[must_use]
pub fn plan_moves(
    entries: &[LibraryEntry],
    pattern: &Pattern,
    settings: &OrganiseSettings,
    roots: &[PathBuf],
    destination: Option<&Path>,
) -> Vec<PlannedMove> {
    let mut claimed: HashSet<String> = HashSet::new();
    let mut folders: HashMap<PathBuf, ArtistFolders> = HashMap::new();

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
            let Some(mut relative) = render(pattern, entry, settings) else {
                return blocked(entry.path.clone(), Obstacle::Unnameable);
            };
            if pattern.leads_with_artist() {
                let known = folders
                    .entry(root.to_path_buf())
                    .or_insert_with(|| ArtistFolders::read(root, &settings.artist_aliases));
                relative = with_artist_folder(&relative, known);
            }

            let target = root.join(relative);
            if same_path(&target, &entry.path) {
                return blocked(target, Obstacle::AlreadyInPlace);
            }
            // Two files rendering to one name are caught while planning.
            if !claimed.insert(comparable(&target)) {
                return blocked(target.clone(), Obstacle::Claimed(target));
            }
            // The same file under another case is a rename, not a collision.
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub moved: usize,
    pub pruned: usize,
    pub failures: Vec<(PathBuf, String)>,
}

/// `source` is the folder a move from outside the library came from, such as
/// slskd's downloads; folders it leaves empty are pruned up to it.
pub fn apply_moves(
    index: &mut LibraryIndex,
    ready: &[&PlannedMove],
    roots: &[PathBuf],
    source: Option<&Path>,
    settings: &OrganiseSettings,
    now: f64,
) -> Result<Applied> {
    let log = ChangeLog::open(index, "organise", now)?;
    let mut applied = Applied::default();
    for planned in ready {
        let from = &planned.entry.path;
        // Logged before the rename; see `ChangeLog::moved`.
        log.moved(index, from, &planned.destination)?;
        match move_file(from, &planned.destination) {
            Ok(()) => {
                let landed = spelled_on_disk(&planned.destination);
                index.relocate(from, &landed, roots)?;
                applied.moved += 1;
                let boundary = owning_root(from, roots)
                    .or_else(|| source.filter(|source| from.starts_with(source)));
                if let (true, Some(boundary)) = (settings.remove_empty_directories, boundary) {
                    applied.pruned += prune_empty(from, boundary).len();
                }
            }
            Err(error) => applied.failures.push((from.clone(), error.to_string())),
        }
    }
    Ok(applied)
}

/// Compared by component, because `a/b` and `a\b` are one path on Windows.
#[must_use]
pub fn same_path(left: &Path, right: &Path) -> bool {
    spelling(left) == spelling(right)
}

/// Unlike [`same_path`], folds case on Windows and macOS; on Unix asks the filesystem.
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

/// Folding on a case-sensitive volume only costs a refused move.
fn comparable(path: &Path) -> String {
    let text = spelling(path);
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text
    }
}

/// Across volumes, copies then removes, cleaning up the copy on failure.
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

/// `remove_dir` refuses a non-empty directory, so one holding `cover.jpg` stays.
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

/// `create_dir_all("zun")` reuses an existing `ZUN` on Windows.
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
