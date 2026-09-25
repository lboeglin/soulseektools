//! Walking a music library on disk, and the container extension sets.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::errors::{Error, Result};

/// Containers whose tags can be read and written.
pub const TAGGABLE_EXTENSIONS: &[&str] = &[".flac", ".mp3", ".m4a", ".mp4", ".ogg", ".opus"];

/// Every container recognised as part of a library, taggable or not.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    ".aif", ".aiff", ".alac", ".flac", ".m4a", ".mp3", ".mp4", ".ogg", ".opus", ".wav",
];

/// Containers that can hold lossless audio, and so are worth verifying.
pub const LOSSLESS_EXTENSIONS: &[&str] = &[".flac", ".wav", ".aiff", ".aif", ".alac", ".m4a"];

/// The lossy containers `stk verify` can hold to their bitrate: the
/// cutoffs it knows are LAME's.
pub const LOSSY_VERIFIED_EXTENSIONS: &[&str] = &[".mp3"];

/// Containers a loudness scan can both decode and write its result to.
///
/// [`TAGGABLE_EXTENSIONS`] less Opus, which has no pure-Rust decoder.
pub const MEASURABLE_EXTENSIONS: &[&str] = &[".flac", ".m4a", ".mp3", ".mp4", ".ogg"];

/// Folders the scan leaves alone, as configured globs.
///
/// Matched case-insensitively against the path relative to its root, with `/`
/// separators on every platform.
#[derive(Debug, Default, Clone)]
pub struct Excludes {
    set: Option<GlobSet>,
}

impl Excludes {
    /// Compile the configured patterns, reporting the one that will not parse.
    pub fn new(patterns: &[String]) -> Result<Self> {
        if patterns.is_empty() {
            return Ok(Self::default());
        }
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            let glob = Glob::new(pattern)
                .map_err(|error| Error::Config(format!("library.exclude: {error}")))?;
            builder.add(glob);
        }
        let set = builder
            .build()
            .map_err(|error| Error::Config(format!("library.exclude: {error}")))?;
        Ok(Self { set: Some(set) })
    }

    #[must_use]
    pub fn matches(&self, root: &Path, path: &Path) -> bool {
        let Some(set) = &self.set else {
            return false;
        };
        let relative = path.strip_prefix(root).unwrap_or(path);
        set.is_match(relative.to_string_lossy().replace('\\', "/"))
    }
}

/// The configured root a file belongs to: the longest one that prefixes it.
///
/// Roots may nest; the deeper one names the artist.
#[must_use]
pub fn owning_root<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a Path> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .map(PathBuf::as_path)
}

/// Every audio file under `root`, recursively, in a stable order.
#[must_use]
pub fn iter_audio_files(root: &Path, extensions: &[&str], exclude: &Excludes) -> Vec<PathBuf> {
    let wanted: Vec<String> = extensions
        .iter()
        .map(|extension| extension.to_lowercase())
        .collect();
    let mut found = Vec::new();
    walk(root, root, &wanted, exclude, &mut found);
    found.sort_by_key(|path| sort_key(path));
    found
}

/// Sort by component, lowercased on Windows: `Ado` before `ALICESOFT`, and
/// `S3RL/` before `S3RL & Sara/`.
fn sort_key(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy().into_owned();
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text
            }
        })
        .collect()
}

fn walk(
    root: &Path,
    directory: &Path,
    wanted: &[String],
    exclude: &Excludes,
    found: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, wanted, exclude, found);
        } else if path.is_file() {
            let suffix = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| format!(".{}", extension.to_lowercase()));
            if suffix.is_some_and(|suffix| wanted.contains(&suffix))
                && !exclude.matches(root, &path)
            {
                found.push(path);
            }
        }
    }
}

/// The artist implied by a file's position in the library.
///
/// The first component below the root, or empty for a file in the root itself.
#[must_use]
pub fn folder_artist(path: &Path, root: &Path) -> String {
    let Ok(relative) = path.strip_prefix(root) else {
        return String::new();
    };
    let components: Vec<_> = relative.components().collect();
    if components.len() > 1 {
        components[0].as_os_str().to_string_lossy().into_owned()
    } else {
        String::new()
    }
}
