//! The artist-to-genre map, its overrides, and planning genre writes.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use stk::genres::{load_genre_map, plan_changes, unmapped_artists, GenreMap, TrackKey};
use stk::matching::NameForms;
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags};
use support::Scratch;

fn example_map() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data/genres.example.toml")
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
        replay_gain: ReplayGainTags::default(),
        isrc: None,
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
    map.genres_for(key(artist, "", ""))
}

fn key<'a>(artist: &'a str, album: &'a str, title: &'a str) -> TrackKey<'a> {
    TrackKey {
        artist,
        album,
        title,
    }
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
fn artists_match_on_their_normalised_name() {
    let scratch = Scratch::new("genres-normalised");
    let path = write_map(&scratch, "\"DECO*27\" = \"Vocaloid\"\n");
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(genres(&genre_map, "DECO_27"), vec!["Vocaloid".to_owned()]);
    assert_eq!(genres(&genre_map, "deco 27"), vec!["Vocaloid".to_owned()]);
}

#[test]
fn the_example_map_loads() {
    let genre_map = load_genre_map(&example_map()).expect("the shipped map");
    assert_eq!(genre_map.len(), 5);
}

#[test]
fn a_file_already_carrying_its_genre_is_not_a_change() {
    let genre_map = map_of(&[("zun", &["Touhou"])]);
    let planned = plan_changes(&[entry("ZUN", "Heian Alien", &["Touhou"])], &genre_map);

    assert!(!planned[0].is_change());
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
fn a_track_override_beats_an_album_override() {
    let scratch = Scratch::new("genres-track-override");
    let path = write_map(
        &scratch,
        "[artists]
Shihori = \"Touhou\"

[albums.Shihori]
Prismatic = \"J-Pop\"

         [tracks.Shihori]
\"Sakura Sunrise\" = \"Anime\"
",
    );
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(
        genre_map.genres_for(key("Shihori", "Prismatic", "Sakura Sunrise")),
        vec!["Anime".to_owned()]
    );
}

#[test]
fn an_artist_with_a_slash_in_their_name_is_still_one_override_key() {
    let scratch = Scratch::new("genres-override-slash");
    let path = write_map(
        &scratch,
        "[albums.\"AC/DC\"]
\"Back in Black\" = \"Rock\"
",
    );
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(
        genre_map.genres_for(key("AC/DC", "Back in Black", "")),
        vec!["Rock".to_owned()]
    );
}

#[test]
fn an_album_override_is_planned_for_the_files_it_names() {
    let genre_map = GenreMap::new(
        [("shihori".to_owned(), vec!["Touhou".to_owned()])]
            .into_iter()
            .collect(),
    )
    .with_albums(
        [(
            ("shihori".to_owned(), "prismatic".to_owned()),
            vec!["J-Pop".to_owned()],
        )]
        .into_iter()
        .collect(),
    );

    let mut on_the_album = entry("Shihori", "Sakura Sunrise", &[]);
    on_the_album.tags.album = "Prismatic".to_owned();
    let elsewhere = entry("Shihori", "Something Else", &[]);

    let planned = plan_changes(&[on_the_album, elsewhere], &genre_map);

    assert_eq!(planned[0].proposed, vec!["J-Pop".to_owned()]);
    assert_eq!(planned[1].proposed, vec!["Touhou".to_owned()]);
}

#[test]
fn a_flat_map_that_grows_an_override_section_is_refused_rather_than_emptied() {
    // Without [artists], adding [albums] would silently drop every artist in the file.
    let scratch = Scratch::new("genres-flat-plus-override");
    let path = write_map(
        &scratch,
        "Shihori = \"Touhou\"
ZUN = \"Touhou\"

[albums.Shihori]
Prismatic = \"J-Pop\"
",
    );

    let error = load_genre_map(&path).expect_err("a rejected map");
    assert!(error.to_string().contains("[artists]"), "{error}");
    assert!(error.to_string().contains("Shihori"), "{error}");
    assert!(error.to_string().contains("ZUN"), "{error}");
}
