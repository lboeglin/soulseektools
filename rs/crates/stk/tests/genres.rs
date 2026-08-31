//! Ported from `py/tests/unit/test_genres.py`.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use stk::genres::{apply_change, load_genre_map, plan_changes, unmapped_artists, GenreMap};
use stk::library::index::describe;
use stk::library::tags;
use stk::matching::NameForms;
use stk::models::{LibraryEntry, TrackTags};
use support::{encode, Encoded, Scratch};

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

fn example_map() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/genres.example.toml")
}

fn entry(folder_artist: &str, title: &str, genres: &[&str]) -> LibraryEntry {
    LibraryEntry {
        path: PathBuf::from(folder_artist).join(format!("{title}.flac")),
        size: 1,
        mtime: 1.0,
        folder_artist: folder_artist.to_owned(),
        tags: TrackTags {
            artist: folder_artist.to_owned(),
            title: title.to_owned(),
            genres: genres.iter().map(|genre| (*genre).to_owned()).collect(),
            ..TrackTags::default()
        },
        artist: NameForms::of(folder_artist),
        title: NameForms::of(title),
        duration_seconds: None,
        artwork_bytes: 0,
        codec: String::new(),
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
    }
}

fn write_map(scratch: &Scratch, body: &str) -> PathBuf {
    let path = scratch.join("genres.toml");
    std::fs::write(&path, body).expect("a written map");
    path
}

fn map_of(pairs: &[(&str, &[&str])]) -> GenreMap {
    GenreMap::new(
        pairs
            .iter()
            .map(|(artist, genres)| {
                (
                    (*artist).to_owned(),
                    genres.iter().map(|genre| (*genre).to_owned()).collect(),
                )
            })
            .collect::<HashMap<String, Vec<String>>>(),
    )
}

fn genres(map: &GenreMap, artist: &str) -> Vec<String> {
    map.genres_for(artist)
}

#[test]
fn reads_a_flat_table() {
    let scratch = Scratch::new("genres-flat");
    let path = write_map(&scratch, "ZUN = \"Touhou\"\nAdo = \"J-Pop\"\n");

    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(genres(&genre_map, "ZUN"), vec!["Touhou".to_owned()]);
    assert_eq!(genre_map.len(), 2);
}

#[test]
fn reads_a_table_nested_under_artists() {
    let scratch = Scratch::new("genres-nested");
    let path = write_map(&scratch, "[artists]\nZUN = \"Touhou\"\n");

    assert_eq!(
        genres(&load_genre_map(&path).expect("a genre map"), "ZUN"),
        vec!["Touhou".to_owned()]
    );
}

#[test]
fn an_artist_may_have_several_genres() {
    let scratch = Scratch::new("genres-several");
    let path = write_map(&scratch, "\"SOUND HOLIC\" = [\"Touhou\", \"Eurobeat\"]\n");

    assert_eq!(
        genres(&load_genre_map(&path).expect("a genre map"), "SOUND HOLIC"),
        vec!["Touhou".to_owned(), "Eurobeat".to_owned()]
    );
}

#[test]
fn artists_match_on_their_normalised_name() {
    // A "DECO*27" entry has to cover the "DECO_27" folder it is filed under.
    let scratch = Scratch::new("genres-normalised");
    let path = write_map(&scratch, "\"DECO*27\" = \"Vocaloid\"\n");
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(genres(&genre_map, "DECO_27"), vec!["Vocaloid".to_owned()]);
    assert_eq!(genres(&genre_map, "deco 27"), vec!["Vocaloid".to_owned()]);
}

#[test]
fn an_unmapped_artist_yields_nothing() {
    let scratch = Scratch::new("genres-unmapped");
    let path = write_map(&scratch, "ZUN = \"Touhou\"\n");
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert!(genres(&genre_map, "Nobody").is_empty());
    assert!(!genre_map.covers("Nobody"));
}

#[test]
fn a_non_string_value_is_rejected() {
    let scratch = Scratch::new("genres-bad-value");
    let path = write_map(&scratch, "ZUN = 42\n");

    let error = load_genre_map(&path).expect_err("a rejected map");
    assert!(error.to_string().contains("must be a genre"), "{error}");
}

#[test]
fn a_missing_file_is_reported_as_a_config_error() {
    let scratch = Scratch::new("genres-absent");
    let error = load_genre_map(&scratch.join("absent.toml")).expect_err("a missing file");
    assert!(error.to_string().contains("cannot read"), "{error}");
}

#[test]
fn the_example_map_loads() {
    let genre_map = load_genre_map(&example_map()).expect("the shipped map");
    assert!(genre_map.len() > 100, "{} entries", genre_map.len());
}

#[test]
fn the_example_map_preserves_the_entries_it_was_converted_from() {
    let genre_map = load_genre_map(&example_map()).expect("the shipped map");

    assert_eq!(genres(&genre_map, "DECO_27"), vec!["Vocaloid".to_owned()]);
    assert_eq!(genres(&genre_map, "ZUN"), vec!["Touhou".to_owned()]);
    assert_eq!(
        genres(&genre_map, "SOUND HOLIC"),
        vec!["Touhou".to_owned(), "Eurobeat".to_owned()]
    );
    assert_eq!(
        genres(&genre_map, "\u{7a32}\u{8449}\u{66c7}"),
        vec!["Vocaloid".to_owned()]
    );
}

#[test]
fn only_mapped_artists_are_planned() {
    let genre_map = map_of(&[("zun", &["Touhou"])]);
    let entries = [
        entry("ZUN", "Heian Alien", &[]),
        entry("Nobody", "Unknown", &[]),
    ];

    let planned = plan_changes(&entries, &genre_map);

    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].proposed, vec!["Touhou".to_owned()]);
}

#[test]
fn a_file_already_carrying_its_genre_is_not_a_change() {
    let genre_map = map_of(&[("zun", &["Touhou"])]);
    let planned = plan_changes(&[entry("ZUN", "Heian Alien", &["Touhou"])], &genre_map);

    assert!(!planned[0].is_change());
}

#[test]
fn a_differing_genre_is_a_change() {
    let genre_map = map_of(&[("zun", &["Touhou"])]);
    let planned = plan_changes(&[entry("ZUN", "Heian Alien", &["Game"])], &genre_map);

    assert!(planned[0].is_change());
    assert_eq!(planned[0].current, vec!["Game".to_owned()]);
}

#[test]
fn lists_artists_absent_from_the_map_with_counts() {
    let genre_map = map_of(&[("zun", &["Touhou"])]);
    let entries = [
        entry("ZUN", "Heian Alien", &[]),
        entry("Nobody", "One", &[]),
        entry("Nobody", "Two", &[]),
        entry("Someone", "Three", &[]),
    ];

    assert_eq!(
        unmapped_artists(&entries, &genre_map),
        vec![("Nobody".to_owned(), 2), ("Someone".to_owned(), 1)]
    );
}

#[test]
fn the_largest_folders_come_first() {
    let mut entries: Vec<LibraryEntry> = (0..3)
        .map(|index| entry("Big", &index.to_string(), &[]))
        .collect();
    entries.push(entry("Small", "x", &[]));

    assert_eq!(unmapped_artists(&entries, &GenreMap::default())[0].0, "Big");
}

#[test]
fn apply_change_writes_the_genre_to_disk() {
    needs_ffmpeg!();
    let scratch = Scratch::new("genres-apply");
    let root = scratch.path();
    let track = encode(
        &scratch.join("ZUN").join("Heian Alien.flac"),
        &Encoded::default(),
    );

    let genre_map = map_of(&[("zun", &["Touhou"])]);
    let described = describe(&track, root).expect("an entry");
    let change = &plan_changes(std::slice::from_ref(&described), &genre_map)[0];

    apply_change(change).expect("a written genre");

    assert_eq!(
        tags::read_tags(&track).expect("readable tags").genres,
        vec!["Touhou".to_owned()]
    );
}

#[test]
fn apply_change_writes_several_genres() {
    needs_ffmpeg!();
    let scratch = Scratch::new("genres-apply-many");
    let root = scratch.path();
    let track = encode(
        &scratch.join("SOUND HOLIC").join("Song.flac"),
        &Encoded::default(),
    );

    let genre_map = map_of(&[("sound holic", &["Touhou", "Eurobeat"])]);
    let described = describe(&track, root).expect("an entry");
    let change = &plan_changes(std::slice::from_ref(&described), &genre_map)[0];

    apply_change(change).expect("written genres");

    assert_eq!(
        tags::read_tags(&track).expect("readable tags").genres,
        vec!["Touhou".to_owned(), "Eurobeat".to_owned()]
    );
}
