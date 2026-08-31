//! Walking a music library on disk.
//!
//! The scripts this replaces used a two-level `listdir`, which silently ignored
//! anything filed under `Artist/Album/track.flac`. The walk here is recursive
//! and the extension set is configurable, so what counts as "in the library" is
//! a setting rather than a literal repeated in a dozen places.
//!
//! The walk itself lands with the index in step 4; the extension sets are here
//! because the ranker needs to know what is lossless.

use std::path::{Path, PathBuf};

/// Containers whose tags can be read and written.
pub const TAGGABLE_EXTENSIONS: &[&str] = &[".flac", ".mp3", ".m4a", ".mp4", ".ogg", ".opus"];

/// Every container recognised as part of a library, taggable or not.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    ".aif", ".aiff", ".alac", ".ape", ".flac", ".m4a", ".mp3", ".mp4", ".ogg", ".opus", ".wav",
    ".wv",
];

/// Containers that can hold lossless audio, and so are worth verifying.
pub const LOSSLESS_EXTENSIONS: &[&str] = &[
    ".flac", ".wav", ".aiff", ".aif", ".ape", ".wv", ".alac", ".m4a",
];

/// Every audio file under `root`, sorted for reproducible output.
///
/// The scripts this replaces used a two-level listing, which silently ignored
/// anything filed under `Artist/Album/track.flac`.
#[must_use]
pub fn iter_audio_files(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let wanted: Vec<String> = extensions
        .iter()
        .map(|extension| extension.to_lowercase())
        .collect();
    let mut found = Vec::new();
    walk(root, &wanted, &mut found);
    found.sort_by_key(|path| sort_key(path));
    found
}

/// How Python orders paths, which is what the captured report order reflects.
///
/// `PurePath` compares its *components*, each through `os.path.normcase`. Two
/// differences from sorting the flat string, both visible in the real library:
///
/// - normcase lowercases on Windows, so `Ado` sorts before `ALICESOFT` rather
///   than after it;
/// - comparing component by component puts `S3RL` before `S3RL & Sara`, where
///   the flat string compares the separator against a space and reverses them.
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

fn walk(directory: &Path, wanted: &[String], found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, wanted, found);
        } else if path.is_file() {
            let suffix = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| format!(".{}", extension.to_lowercase()));
            if suffix.is_some_and(|suffix| wanted.contains(&suffix)) {
                found.push(path);
            }
        }
    }
}

/// The artist implied by a file's position in the library.
///
/// Libraries are laid out as `<root>/<artist>/...`, so the first component below
/// the root names the artist. A file sitting directly in the root has no implied
/// artist.
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
