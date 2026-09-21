//! Ported from `py/tests/unit/test_genres.py`.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use stk::genres::{
    apply_change, load_genre_map, plan_changes, unmapped_artists, GenreMap, TrackKey,
};
use stk::library::index::describe;
use stk::library::tags;
use stk::matching::NameForms;
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags};
use support::{encode, Encoded, Scratch};

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
    assert!(!genre_map.covers(key("Nobody", "", "")));
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

#[test]
fn an_album_override_beats_the_artist_entry() {
    let scratch = Scratch::new("genres-album-override");
    let path = write_map(
        &scratch,
        "[artists]
Shihori = \"Touhou\"

[albums.Shihori]
Prismatic = \"J-Pop\"
",
    );
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(
        genre_map.genres_for(key("Shihori", "Prismatic", "Anything")),
        vec!["J-Pop".to_owned()]
    );
    assert_eq!(
        genre_map.genres_for(key("Shihori", "Another Record", "Anything")),
        vec!["Touhou".to_owned()]
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
fn an_override_may_name_several_genres() {
    let scratch = Scratch::new("genres-override-several");
    let path = write_map(
        &scratch,
        "[albums.Shihori]
Prismatic = [\"Touhou\", \"Eurobeat\"]
",
    );
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(
        genre_map.genres_for(key("Shihori", "Prismatic", "")),
        vec!["Touhou".to_owned(), "Eurobeat".to_owned()]
    );
}

#[test]
fn an_override_matches_on_its_normalised_form() {
    // The same rule artist keys already follow: what is on disk is rarely
    // spelled the way the map is.
    let scratch = Scratch::new("genres-override-normalised");
    let path = write_map(
        &scratch,
        "[albums.\"DECO*27\"]
\"Ghost Rule\" = \"Vocaloid\"
",
    );
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(
        genre_map.genres_for(key("DECO_27", "ghost rule", "")),
        vec!["Vocaloid".to_owned()]
    );
}

#[test]
fn an_artist_with_a_slash_in_their_name_is_still_one_override_key() {
    // A composite "Artist/Album" key could not tell which slash separated.
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
fn an_artist_only_map_reads_exactly_as_it_did_before() {
    let scratch = Scratch::new("genres-override-compat");
    let path = write_map(
        &scratch,
        "ZUN = \"Touhou\"
Ado = \"J-Pop\"
",
    );
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(genres(&genre_map, "ZUN"), vec!["Touhou".to_owned()]);
    assert_eq!(genre_map.len(), 2);
}

#[test]
fn a_map_of_overrides_alone_does_not_read_its_sections_as_artists() {
    // Without an [artists] section the whole document is the artist table, so
    // an [albums] section would otherwise become an artist named "albums".
    let scratch = Scratch::new("genres-override-only");
    let path = write_map(
        &scratch,
        "[albums.Shihori]
Prismatic = \"J-Pop\"
",
    );
    let genre_map = load_genre_map(&path).expect("a genre map");

    assert_eq!(genre_map.len(), 0);
    assert!(genres(&genre_map, "albums").is_empty());
    assert_eq!(
        genre_map.genres_for(key("Shihori", "Prismatic", "")),
        vec!["J-Pop".to_owned()]
    );
}

#[test]
fn an_override_section_that_is_not_a_table_of_artists_is_rejected() {
    let scratch = Scratch::new("genres-override-bad");
    let path = write_map(
        &scratch,
        "albums = 42
",
    );

    let error = load_genre_map(&path).expect_err("a rejected map");
    assert!(error.to_string().contains("must be a table"), "{error}");
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
fn an_artist_covered_only_by_an_override_is_still_reported_as_unmapped() {
    // `genre check` asks what the map has nothing general to say about, and one
    // album's worth of exception is not an answer to that.
    let genre_map = GenreMap::default().with_albums(
        [(
            ("shihori".to_owned(), "prismatic".to_owned()),
            vec!["J-Pop".to_owned()],
        )]
        .into_iter()
        .collect(),
    );

    assert_eq!(
        unmapped_artists(&[entry("Shihori", "Sakura Sunrise", &[])], &genre_map),
        vec![("Shihori".to_owned(), 1)]
    );
}

#[test]
fn a_flat_map_that_grows_an_override_section_is_refused_rather_than_emptied() {
    // The trap the sections open: without [artists] the whole document is the
    // artist table, so adding [albums] would otherwise drop every artist in it
    // without a word and turn `genre apply` into a silent no-op.
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
