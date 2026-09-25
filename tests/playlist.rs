//! The M3U writer, which is how an order gets back out of the library.

use std::path::{Path, PathBuf};

use stk::playlist::{relative_to, render, PlaylistEntry};

fn entry(path: &str, artist: &str, title: &str, seconds: Option<f64>) -> PlaylistEntry {
    PlaylistEntry {
        path: PathBuf::from(path),
        artist: artist.to_owned(),
        title: title.to_owned(),
        duration_seconds: seconds,
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

#[test]
fn an_entry_carries_its_length_and_name_before_its_path() {
    let text = render(
        &[entry("/music/ZUN/Song.flac", "ZUN", "Song", Some(212.0))],
        None,
    );

    assert_eq!(
        lines(&text),
        vec!["#EXTM3U", "#EXTINF:212,ZUN - Song", "/music/ZUN/Song.flac"]
    );
}

#[test]
fn a_track_with_no_duration_is_written_with_minus_one() {
    // The extended-M3U convention. A zero makes several players skip the entry.
    let text = render(&[entry("/music/a.flac", "A", "B", None)], None);

    assert!(text.contains("#EXTINF:-1,A - B"), "{text}");
}

#[test]
fn a_relative_playlist_reaches_the_music_from_where_it_sits() {
    let text = render(
        &[entry("/music/ZUN/Song.flac", "ZUN", "Song", None)],
        Some(Path::new("/music/playlists")),
    );

    assert!(text.contains("../ZUN/Song.flac"), "{text}");
}

#[test]
fn a_file_the_playlist_cannot_reach_is_written_absolute() {
    // Two Windows drives share no prefix, so no number of `..` connects them.
    assert_eq!(
        relative_to(Path::new(r"E:\Music\a.flac"), Path::new(r"C:\lists")),
        None
    );

    let text = render(
        &[entry(r"E:\Music\a.flac", "A", "B", None)],
        Some(Path::new(r"C:\lists")),
    );
    assert!(text.contains("E:/Music/a.flac"), "{text}");
}
