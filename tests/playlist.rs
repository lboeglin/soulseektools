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
fn a_playlist_opens_with_the_extended_header() {
    assert!(render(&[], None).starts_with("#EXTM3U\n"));
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
fn a_playlist_has_no_byte_order_mark() {
    // Unlike this project's CSV writers: .m3u8 is UTF-8 by definition, and a
    // BOM makes several players fail on the first path.
    let text = render(&[entry("/music/a.flac", "A", "B", None)], None);

    assert!(!text.starts_with('\u{feff}'), "{text:?}");
}

#[test]
fn every_path_is_written_with_forward_slashes() {
    let text = render(
        &[entry(r"E:\Music\ZUN\Song.flac", "ZUN", "Song", None)],
        None,
    );

    assert!(text.contains("E:/Music/ZUN/Song.flac"), "{text}");
    assert!(!text.contains('\\'), "{text}");
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
fn a_playlist_beside_the_music_names_it_without_walking_up() {
    let text = render(
        &[entry("/music/ZUN/Song.flac", "ZUN", "Song", None)],
        Some(Path::new("/music")),
    );

    assert!(text.contains("ZUN/Song.flac"), "{text}");
    assert!(!text.contains(".."), "{text}");
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

#[test]
fn the_entries_keep_the_order_they_were_given() {
    let text = render(
        &[
            entry("/music/second.flac", "A", "Second", None),
            entry("/music/first.flac", "A", "First", None),
        ],
        None,
    );

    let names: Vec<&str> = lines(&text)
        .into_iter()
        .filter(|line| line.starts_with("#EXTINF"))
        .collect();
    assert_eq!(names, vec!["#EXTINF:-1,A - Second", "#EXTINF:-1,A - First"]);
}
