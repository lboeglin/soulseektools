//! Reading exports in every format, and diffing them against the library.

mod support;

use std::path::PathBuf;

use stk::matching::{MatchThresholds, NameForms};
use stk::models::{LibraryEntry, ReplayGainTags, Track, TrackTags};
use stk::wantlist::{diff, keep_played, read_export, split_query, WantlistDiff};
use support::{encode, Encoded, Scratch};

fn entry(artist: &str, title: &str) -> LibraryEntry {
    LibraryEntry {
        path: PathBuf::from(artist).join(format!("{title}.flac")),
        size: 1,
        mtime: 1.0,
        folder_artist: artist.to_owned(),
        tags: TrackTags {
            artist: artist.to_owned(),
            title: title.to_owned(),
            ..TrackTags::default()
        },
        artist: NameForms::of(artist),
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

fn write_csv(scratch: &Scratch, body: &str) -> PathBuf {
    let path = scratch.join("export.csv");
    let mut bytes = b"\xef\xbb\xbf".to_vec();
    bytes.extend_from_slice(body.as_bytes());
    std::fs::write(&path, bytes).expect("a written export");
    path
}

fn wanted(artist: &str, title: &str) -> Vec<Track> {
    vec![Track::new(artist, title)]
}

fn split(tracks: &[Track], library: &[LibraryEntry]) -> WantlistDiff {
    diff(tracks, library, &MatchThresholds::default())
}

#[test]
fn reads_the_spotify_column_names() {
    let scratch = Scratch::new("wantlist-spotify");
    let path = write_csv(
        &scratch,
        "Track name,Artist name,Album name,Duration (ms)\nRoki,DECO*27,Ghost,215000\n",
    );

    let tracks = read_export(&path, None, None).expect("an export");

    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].artist, "DECO*27");
    assert_eq!(tracks[0].title, "Roki");
    assert_eq!(tracks[0].album, "Ghost");
    assert!((tracks[0].duration_seconds.expect("a duration") - 215.0).abs() < 1e-9);
}

#[test]
fn reads_alternative_column_names() {
    let scratch = Scratch::new("wantlist-alt");
    let path = write_csv(&scratch, "Title,Artist\nRoki,DECO*27\n");

    assert_eq!(
        read_export(&path, None, None).expect("an export")[0].title,
        "Roki"
    );
}

#[test]
fn a_file_without_the_needed_columns_is_rejected() {
    let scratch = Scratch::new("wantlist-no-columns");
    let path = write_csv(&scratch, "Foo,Bar\n1,2\n");

    let error = read_export(&path, None, None).expect_err("a rejected export");
    assert!(error.to_string().contains("need a title column"), "{error}");
}

#[test]
fn a_kanji_title_matches_its_romanised_file() {
    // Without romaji these share no characters.
    let result = split(
        &wanted("Hatsune Miku", "\u{5343}\u{672c}\u{685c}"),
        &[entry("Hatsune Miku", "Senbonzakura")],
    );

    assert!(result.missing.is_empty());
}

#[test]
fn an_uncertain_match_stays_in_the_wantlist() {
    // An uncertain match stays missing: a duplicate download is cheaper than a lost track.
    let result = split(
        &wanted("DECO*27", "Roki"),
        &[entry("Some Other Artist", "A Different Song Entirely")],
    );

    assert_eq!(result.missing.len(), 1);
}

#[test]
fn every_track_is_accounted_for() {
    let mut tracks = wanted("DECO*27", "Roki");
    tracks.extend(wanted("ZUN", "Heian Alien"));

    let result = split(&tracks, &[entry("DECO*27", "Roki")]);

    assert_eq!(result.total(), 2);
}

#[test]
fn blocking_finds_a_match_inside_a_large_library() {
    let mut library: Vec<LibraryEntry> = (0..300)
        .map(|index| entry(&format!("Artist {index}"), &format!("Some Song {index}")))
        .collect();
    library.push(entry("DECO*27", "Roki"));

    let result = split(&wanted("DECO_27", "Roki"), &library);

    assert!(result.missing.is_empty());
}

#[test]
fn a_query_splits_on_a_hyphen_or_either_dash() {
    for query in [
        "DECO*27 - Roki",
        "DECO*27 \u{2013} Roki",
        "DECO*27 \u{2014} Roki",
    ] {
        assert_eq!(
            split_query(query),
            ("DECO*27".to_owned(), "Roki".to_owned())
        );
    }
    assert_eq!(split_query("Roki"), (String::new(), "Roki".to_owned()));
}

#[test]
fn reads_a_youtube_takeout() {
    let scratch = Scratch::new("wantlist-youtube");
    let path = write_csv(
        &scratch,
        "Video ID,Song Title,Album Title,Artist Name 1,Artist Name 2\n\
         abc123,Roki,Ghost,DECO*27,\n",
    );

    let track = &read_export(&path, None, None).expect("an export")[0];
    assert_eq!(track.query(), "DECO*27 - Roki");
    assert_eq!(track.album, "Ghost");
    assert!(!track.title_guessed);
}

#[test]
fn a_title_list_without_an_artist_column_is_split_as_a_guess() {
    let scratch = Scratch::new("wantlist-titles-only");
    let path = write_csv(&scratch, "Title\nDECO*27 - Roki\nGhost Rule\n");

    let tracks = read_export(&path, None, None).expect("an export");
    assert_eq!(tracks[0].query(), "DECO*27 - Roki");
    assert!(tracks[0].title_guessed);
    assert_eq!(
        (tracks[1].artist.as_str(), tracks[1].title.as_str()),
        ("", "Ghost Rule")
    );
}

#[test]
fn reads_a_headerless_lastfm_to_csv_log() {
    let scratch = Scratch::new("wantlist-lastfm-headerless");
    let path = write_csv(
        &scratch,
        "DECO*27,Ghost,Roki,31 Jan 2021 12:34\nDECO*27,Ghost,Ghost Rule,31 Jan 2021 12:30\n",
    );

    let tracks = read_export(&path, None, None).expect("an export");
    assert_eq!(tracks.len(), 2, "the first row is a play, not a header");
    assert_eq!(tracks[0].query(), "DECO*27 - Roki");
    assert_eq!(tracks[0].album, "Ghost");
    assert!(
        tracks[0]
            .source_row
            .contains(&("date".to_owned(), "31 Jan 2021 12:34".to_owned())),
        "{:?}",
        tracks[0].source_row
    );
}

#[test]
fn keeping_played_tracks_collapses_repeats_and_drops_the_rare_ones() {
    let plays = [
        Track::new("DECO*27", "Roki"),
        Track::new("Ado", "Usseewa"),
        Track::new("deco*27", "ROKI"),
        Track::new("Ado", "Odo"),
        Track::new("Ado", "Odo"),
    ];

    let kept: Vec<String> = keep_played(plays.to_vec(), 2)
        .iter()
        .map(Track::query)
        .collect();
    assert_eq!(kept, ["DECO*27 - Roki", "Ado - Odo"]);
    assert_eq!(keep_played(plays.to_vec(), 1).len(), 3);
}

fn write_library(scratch: &Scratch, items: &str) -> PathBuf {
    let path = scratch.join("Library.xml");
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Major Version</key><integer>1</integer>
	<key>Tracks</key>
	<dict>{items}</dict>
	<key>Playlists</key><array/>
</dict>
</plist>
"#
    );
    std::fs::write(&path, body).expect("a written library");
    path
}

const ROKI: &str = r"
		<key>101</key>
		<dict>
			<key>Track ID</key><integer>101</integer>
			<key>Name</key><string>Roki</string>
			<key>Artist</key><string>DECO*27</string>
			<key>Album</key><string>Ghost</string>
			<key>Kind</key><string>Apple Lossless audio file</string>
			<key>Total Time</key><integer>215000</integer>
			<key>Track Number</key><integer>3</integer>
			<key>Date Added</key><date>2021-01-31T12:34:00Z</date>
		</dict>";

#[test]
fn reads_an_apple_music_library() {
    let scratch = Scratch::new("wantlist-apple");
    let path = write_library(&scratch, ROKI);

    let tracks = read_export(&path, None, None).expect("a library");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].query(), "DECO*27 - Roki");
    assert_eq!(tracks[0].album, "Ghost");
    assert!((tracks[0].duration_seconds.expect("a duration") - 215.0).abs() < 1e-9);
    assert_eq!(
        tracks[0].source_value(stk::wantlist::TRACK_NUMBER_COLUMNS),
        Some("3")
    );
    assert_eq!(
        tracks[0].source_value(&["date added"]),
        Some("2021-01-31T12:34:00Z")
    );
}

#[test]
fn podcasts_films_and_audiobooks_in_an_apple_library_are_skipped() {
    let scratch = Scratch::new("wantlist-apple-not-music");
    let items = format!(
        r"{ROKI}
		<key>2</key><dict>
			<key>Name</key><string>Episode 12</string><key>Artist</key><string>A Show</string>
			<key>Podcast</key><true/>
		</dict>
		<key>3</key><dict>
			<key>Name</key><string>A Film</string><key>Artist</key><string>A Studio</string>
			<key>Movie</key><true/>
		</dict>
		<key>4</key><dict>
			<key>Name</key><string>Chapter 1</string><key>Artist</key><string>A Reader</string>
			<key>Kind</key><string>Audiobook</string>
		</dict>"
    );
    let path = write_library(&scratch, &items);

    let titles: Vec<String> = read_export(&path, None, None)
        .expect("a library")
        .into_iter()
        .map(|track| track.title)
        .collect();
    assert_eq!(titles, ["Roki"]);
}

#[test]
fn an_apple_library_is_recognised_by_its_content_not_its_name() {
    let scratch = Scratch::new("wantlist-apple-renamed");
    let library = write_library(&scratch, ROKI);
    let renamed = scratch.join("export.csv");
    std::fs::rename(&library, &renamed).expect("a renamed library");

    assert_eq!(
        read_export(&renamed, None, None).expect("a library")[0].title,
        "Roki"
    );
}

fn write_playlist(scratch: &Scratch, name: &str, body: &str) -> PathBuf {
    let path = scratch.join(name);
    let mut bytes = b"\xef\xbb\xbf".to_vec();
    bytes.extend_from_slice(body.as_bytes());
    std::fs::write(&path, bytes).expect("a written playlist");
    path
}

#[test]
fn a_playlist_entry_on_this_machine_is_described_by_its_tags() {
    let scratch = Scratch::new("wantlist-m3u-tags");
    encode(
        &scratch.join("music").join("01 - track.flac"),
        &Encoded {
            metadata: &[
                ("artist", "DECO*27"),
                ("title", "Roki"),
                ("album", "Ghost"),
                ("track", "3"),
            ],
            ..Encoded::default()
        },
    );
    let path = write_playlist(
        &scratch,
        "list.m3u8",
        "#EXTM3U\n#EXTINF:1,Someone Else - Wrong Title\nmusic/01 - track.flac\n",
    );

    let track = &read_export(&path, None, None).expect("a playlist")[0];
    assert_eq!(track.query(), "DECO*27 - Roki");
    assert_eq!(track.album, "Ghost");
    assert!((track.duration_seconds.expect("a duration") - 2.0).abs() < 0.1);
    assert!(!track.title_guessed);
    assert_eq!(
        track.source_value(stk::wantlist::TRACK_NUMBER_COLUMNS),
        Some("3")
    );
    assert_eq!(track.source_value(&["path"]), Some("music/01 - track.flac"));
}

#[test]
fn a_missing_playlist_entry_is_described_by_its_extinf_line() {
    let scratch = Scratch::new("wantlist-m3u-extinf");
    let path = write_playlist(
        &scratch,
        "list.m3u8",
        "#EXTM3U\n#EXTINF:215,DECO*27 - Roki\nE:/elsewhere/whatever.flac\n\
         #EXTINF:-1 tvg-id=\"x\",Ado - Usseewa\nhttp://example.invalid/stream\n",
    );

    let tracks = read_export(&path, None, None).expect("a playlist");
    assert_eq!(tracks[0].query(), "DECO*27 - Roki");
    assert!((tracks[0].duration_seconds.expect("a duration") - 215.0).abs() < 1e-9);
    assert!(!tracks[0].title_guessed);
    assert_eq!(tracks[1].query(), "Ado - Usseewa");
    assert_eq!(tracks[1].duration_seconds, None);
}

#[test]
fn a_bare_playlist_entry_is_guessed_from_its_filename() {
    let scratch = Scratch::new("wantlist-m3u-filename");
    let path = write_playlist(
        &scratch,
        "list.m3u",
        "D:\\Music\\DECO_27\\01 - DECO*27 - Roki.flac\n# a comment\n\n02. Ghost Rule.mp3\n",
    );

    let tracks = read_export(&path, None, None).expect("a playlist");
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].query(), "DECO*27 - Roki");
    assert!(tracks[0].title_guessed);
    assert_eq!(
        (tracks[1].artist.as_str(), tracks[1].title.as_str()),
        ("", "Ghost Rule")
    );
}

#[test]
fn an_isrc_matches_a_recording_whatever_script_its_title_is_in() {
    let tagged = LibraryEntry {
        isrc: Some("JPZ001600001".to_owned()),
        ..entry("DECO*27", "Ghost Rule")
    };
    let wanted = |isrc: &str| Track {
        source_row: vec![("ISRC".to_owned(), isrc.to_owned())],
        ..Track::new("DECO*27", "ゴーストルール")
    };

    let matched = diff(
        &[wanted("JP-Z00-16-00001")],
        std::slice::from_ref(&tagged),
        &MatchThresholds::default(),
    );
    assert_eq!(matched.owned.len(), 1);
    assert!((matched.owned[0].score - 1.0).abs() < f64::EPSILON);

    let other = diff(
        &[wanted("JPZ001699999")],
        &[tagged],
        &MatchThresholds::default(),
    );
    assert_eq!(
        other.missing.len(),
        1,
        "names alone don't match across scripts"
    );
}

#[test]
fn an_isrc_is_read_in_any_of_its_spellings() {
    use stk::text::normalize::isrc;

    assert_eq!(isrc("jp-z00-16-00001").as_deref(), Some("JPZ001600001"));
    assert_eq!(isrc("bad; USRC17607839").as_deref(), Some("USRC17607839"));
    assert_eq!(isrc("not an isrc"), None);
}

#[test]
fn a_track_without_an_artist_reads_as_its_title_alone() {
    assert_eq!(Track::new("", "Roki").query(), "Roki");
    assert_eq!(Track::new("DECO*27", "Roki").query(), "DECO*27 - Roki");
}
