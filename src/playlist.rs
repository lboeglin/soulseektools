//! Writing playlists, which is how an order gets back out of the library.
//!
//! The library is built from an ordered export and the order is then thrown
//! away: the index is keyed by path and reads back sorted by path, so the
//! sequence someone actually listens in survives nowhere (#3). An M3U file is
//! the cheapest way to hand it back, and every player reads one.
//!
//! Rendering is a pure function over entries so the format is tested directly,
//! without a library on disk to walk.

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
/// The extended-M3U convention, and not something to invent a substitute for:
/// a player reading `-1` shows the track without a duration, where a `0` makes
/// several of them skip it.
const UNKNOWN_DURATION: i64 = -1;

/// Render an extended M3U. `base` makes every path relative to it.
///
/// Paths are written with forward slashes whatever the platform. That is what
/// the format has always used, every player accepts it on Windows too, and one
/// rule means a playlist written on one machine is not subtly wrong on another.
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
///
/// No track is long enough to reach the edge of an `i64`, and a file whose
/// header claims otherwise is better clamped than wrapped.
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
/// Two paths on different Windows drives share no prefix, so no number of `..`
/// segments connects them and the caller has to fall back to an absolute path.
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
