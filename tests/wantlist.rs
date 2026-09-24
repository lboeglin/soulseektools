//! Ported from `py/tests/unit/test_wantlist.py`.

mod support;

use std::path::PathBuf;

use stk::matching::{MatchThresholds, NameForms};
use stk::models::{LibraryEntry, ReplayGainTags, Track, TrackTags};
use stk::wantlist::{diff, keep_played, read_export, split_query, Format, WantlistDiff};
use support::Scratch;

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
    }
}

fn write_csv(scratch: &Scratch, body: &str) -> PathBuf {
    let path = scratch.join("export.csv");
    // The Python writes utf-8-sig; the reader has to cope with the BOM either way.
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

    let tracks = read_export(&path, None).expect("an export");

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
        read_export(&path, None).expect("an export")[0].title,
        "Roki"
    );
}

#[test]
fn reads_a_duration_written_as_minutes_and_seconds() {
    let scratch = Scratch::new("wantlist-mmss");
    let path = write_csv(&scratch, "Title,Artist,Length\nRoki,DECO*27,3:35\n");

    let duration = read_export(&path, None).expect("an export")[0]
        .duration_seconds
        .expect("a duration");
    assert!((duration - 215.0).abs() < 1e-9);
}

#[test]
fn an_unreadable_duration_is_left_unset() {
    let scratch = Scratch::new("wantlist-bad-duration");
    let path = write_csv(&scratch, "Title,Artist,Length\nRoki,DECO*27,soon\n");

    assert_eq!(
        read_export(&path, None).expect("an export")[0].duration_seconds,
        None
    );
}

#[test]
fn the_original_row_is_kept() {
    let scratch = Scratch::new("wantlist-source-row");
    let path = write_csv(
        &scratch,
        "Track name,Artist name,Added at\nRoki,DECO*27,2026-01-01\n",
    );

    let source_row = &read_export(&path, None).expect("an export")[0].source_row;
    assert!(
        source_row.contains(&("Added at".to_owned(), "2026-01-01".to_owned())),
        "{source_row:?}"
    );
}

#[test]
fn blank_rows_are_skipped() {
    let scratch = Scratch::new("wantlist-blank");
    let path = write_csv(&scratch, "Track name,Artist name\nRoki,DECO*27\n,\n");

    assert_eq!(read_export(&path, None).expect("an export").len(), 1);
}

#[test]
fn a_file_without_the_needed_columns_is_rejected() {
    let scratch = Scratch::new("wantlist-no-columns");
    let path = write_csv(&scratch, "Foo,Bar\n1,2\n");

    let error = read_export(&path, None).expect_err("a rejected export");
    assert!(error.to_string().contains("need a title column"), "{error}");
}

#[test]
fn an_empty_file_is_rejected() {
    let scratch = Scratch::new("wantlist-empty");
    let path = write_csv(&scratch, "");

    let error = read_export(&path, None).expect_err("a rejected export");
    assert!(error.to_string().contains("empty"), "{error}");
}

#[test]
fn the_query_used_for_searching_joins_artist_and_title() {
    let scratch = Scratch::new("wantlist-query");
    let path = write_csv(&scratch, "Track name,Artist name\nRoki,DECO*27\n");

    assert_eq!(
        read_export(&path, None).expect("an export")[0].query(),
        "DECO*27 - Roki"
    );
}

#[test]
fn an_owned_track_is_not_missing() {
    let library = [entry("DECO*27", "Roki")];
    let result = split(&wanted("DECO*27", "Roki"), &library);

    assert!(result.missing.is_empty());
    assert_eq!(result.owned.len(), 1);
    assert!(result.owned[0].score > 0.9);
}

#[test]
fn a_track_absent_from_the_library_is_missing() {
    let result = split(&wanted("ZUN", "Heian Alien"), &[entry("DECO*27", "Roki")]);
    assert_eq!(result.missing.len(), 1);
}

#[test]
fn a_kanji_title_matches_its_romanised_file() {
    // Without romaji these share not a single character.
    let result = split(
        &wanted("Hatsune Miku", "\u{5343}\u{672c}\u{685c}"),
        &[entry("Hatsune Miku", "Senbonzakura")],
    );

    assert!(result.missing.is_empty());
}

#[test]
fn an_uncertain_match_stays_in_the_wantlist() {
    // Deliberate: a duplicate download costs megabytes, a dropped track is lost.
    let result = split(
        &wanted("DECO*27", "Roki"),
        &[entry("Some Other Artist", "A Different Song Entirely")],
    );

    assert_eq!(result.missing.len(), 1);
}

#[test]
fn the_best_of_several_matches_is_reported() {
    let library = [
        entry("Hatsune Miku", "Senbonzakura -Long Ver-"),
        entry("Hatsune Miku", "Senbonzakura"),
    ];

    let result = split(&wanted("Hatsune Miku", "Senbonzakura"), &library);

    assert_eq!(result.owned[0].entry.title.raw, "Senbonzakura");
}

#[test]
fn every_track_is_accounted_for() {
    let mut tracks = wanted("DECO*27", "Roki");
    tracks.extend(wanted("ZUN", "Heian Alien"));

    let result = split(&tracks, &[entry("DECO*27", "Roki")]);

    assert_eq!(result.total(), 2);
}

#[test]
fn an_empty_library_leaves_everything_missing() {
    assert_eq!(split(&wanted("DECO*27", "Roki"), &[]).missing.len(), 1);
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

/// The track number stays in the raw row and is read back by header, with the
/// same tolerance the export was read with (#11).
#[test]
fn a_track_number_is_found_whatever_its_header_spelling() {
    let scratch = Scratch::new("wantlist-track-number");
    let path = write_csv(
        &scratch,
        "Track Name,Artist Name(s),TRACK NUMBER \nRoki,DECO*27,3\n",
    );

    let track = &read_export(&path, None).expect("an export")[0];
    assert_eq!(
        track.source_value(stk::wantlist::TRACK_NUMBER_COLUMNS),
        Some("3")
    );
}

#[test]
fn an_empty_cell_reads_as_no_value() {
    let scratch = Scratch::new("wantlist-track-number-empty");
    let path = write_csv(
        &scratch,
        "Track Name,Artist Name(s),Track Number\nRoki,DECO*27,\n",
    );

    let track = &read_export(&path, None).expect("an export")[0];
    assert_eq!(
        track.source_value(stk::wantlist::TRACK_NUMBER_COLUMNS),
        None
    );
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

    let track = &read_export(&path, None).expect("an export")[0];
    assert_eq!(track.query(), "DECO*27 - Roki");
    assert_eq!(track.album, "Ghost");
    assert!(!track.title_guessed);
}

#[test]
fn a_title_list_without_an_artist_column_is_split_as_a_guess() {
    let scratch = Scratch::new("wantlist-titles-only");
    let path = write_csv(&scratch, "Title\nDECO*27 - Roki\nGhost Rule\n");

    let tracks = read_export(&path, None).expect("an export");
    assert_eq!(tracks[0].query(), "DECO*27 - Roki");
    assert!(tracks[0].title_guessed);
    assert_eq!(
        (tracks[1].artist.as_str(), tracks[1].title.as_str()),
        ("", "Ghost Rule")
    );
}

#[test]
fn reads_a_lastfm_export_whose_title_column_is_track() {
    let scratch = Scratch::new("wantlist-lastfm-headered");
    let path = write_csv(
        &scratch,
        "uts,utc_time,artist,artist_mbid,album,album_mbid,track,track_mbid\n\
         1700000000,14 Nov 2023 22:13,DECO*27,,Ghost,,Roki,\n",
    );

    let track = &read_export(&path, None).expect("an export")[0];
    assert_eq!(track.query(), "DECO*27 - Roki");
    assert_eq!(track.album, "Ghost");
}

#[test]
fn reads_a_headerless_lastfm_to_csv_log() {
    let scratch = Scratch::new("wantlist-lastfm-headerless");
    let path = write_csv(
        &scratch,
        "DECO*27,Ghost,Roki,31 Jan 2021 12:34\nDECO*27,Ghost,Ghost Rule,31 Jan 2021 12:30\n",
    );

    let tracks = read_export(&path, None).expect("an export");
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

#[test]
fn a_format_is_named_as_the_flag_spells_it() {
    assert_eq!("csv".parse::<Format>(), Ok(Format::Csv));
    let error = "xls".parse::<Format>().expect_err("an unknown format");
    assert!(error.contains("known: csv"), "{error}");
}
