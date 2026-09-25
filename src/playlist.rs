//! Writing M3U8 playlists.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

/// One line of a playlist: a file, and what to call it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub duration_seconds: Option<f64>,
}

/// What `#EXTINF` carries when the length is not known.
///
/// The extended-M3U convention; some players skip a `0`.
const UNKNOWN_DURATION: i64 = -1;

/// Render an extended M3U. `base` makes every path relative to it.
///
/// Paths use forward slashes on every platform.
#[must_use]
pub fn render(entries: &[PlaylistEntry], base: Option<&Path>) -> String {
    let mut text = String::from("#EXTM3U\n");
    for entry in entries {
        let seconds = duration_of(entry);
        let location = base.map_or_else(
            || entry.path.clone(),
            |base| relative_to(&entry.path, base).unwrap_or_else(|| entry.path.clone()),
        );
        let _ = writeln!(
            text,
            "#EXTINF:{seconds},{} - {}\n{}",
            entry.artist,
            entry.title,
            with_forward_slashes(&location)
        );
    }
    text
}

/// A track length as `#EXTINF` wants it: whole seconds, or `-1`.
#[allow(clippy::cast_possible_truncation)]
fn duration_of(entry: &PlaylistEntry) -> i64 {
    entry
        .duration_seconds
        .filter(|seconds| *seconds > 0.0)
        .map_or(UNKNOWN_DURATION, |seconds| seconds.round() as i64)
}

fn with_forward_slashes(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// `path` as reached from `base`, or `None` when there is no way to walk there.
///
/// `None` across Windows drives, which no `..` can connect.
#[must_use]
pub fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    let target: Vec<Component<'_>> = path.components().collect();
    let from: Vec<Component<'_>> = base.components().collect();

    let shared = target
        .iter()
        .zip(&from)
        .take_while(|(left, right)| left == right)
        .count();
    // Nothing in common means different volumes, since both are absolute.
    if shared == 0 {
        return None;
    }

    let mut relative = PathBuf::new();
    for _ in shared..from.len() {
        relative.push("..");
    }
    for component in &target[shared..] {
        relative.push(component);
    }
    Some(relative)
}
