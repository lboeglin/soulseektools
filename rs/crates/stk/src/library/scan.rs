//! Walking a music library on disk.
//!
//! The scripts this replaces used a two-level `listdir`, which silently ignored
//! anything filed under `Artist/Album/track.flac`. The walk here is recursive
//! and the extension set is configurable, so what counts as "in the library" is
//! a setting rather than a literal repeated in a dozen places.
//!
//! The walk itself lands with the index in step 4; the extension sets are here
//! because the ranker needs to know what is lossless.

use std::path::Path;

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
