//! Writing M3U8 playlists.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub duration_seconds: Option<f64>,
}

/// The extended-M3U convention; some players skip a `0`.
const UNKNOWN_DURATION: i64 = -1;

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
