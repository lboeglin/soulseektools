//! The policy that decides what a fetched file is tagged with (#11).
//!
//! `plan` is pure, so every rule about which side wins is checked here without
//! a file; the write, the log and the reindex are covered in `fetch.rs`.

use stk::models::{Track, TrackTags};
use stk::retag::{plan, RowTags, TagPolicy};

/// What a peer's tagger typically leaves.
fn peer() -> TrackTags {
    TrackTags {
        artist: "Unknown Artist".to_owned(),
        album_artist: "Various".to_owned(),
        album: "www.something.ru".to_owned(),
        title: "Track 01".to_owned(),
        genres: vec!["Vocaloid".to_owned()],
        track_number: Some(7),
    }
}

fn row() -> RowTags {
    RowTags {
        artist: "DECO*27".to_owned(),
        title: "Roki".to_owned(),
        album: "Ghost".to_owned(),
        track_number: None,
    }
}

#[test]
fn replace_writes_the_row_over_what_the_peer_left() {
    let planned = plan(TagPolicy::Replace, &row(), &peer());

    assert_eq!(planned.artist, "DECO*27");
    assert_eq!(planned.title, "Roki");
    assert_eq!(planned.album, "Ghost");
}

#[test]
fn replace_leaves_what_the_row_cannot_say_to_the_peer() {
    let planned = plan(TagPolicy::Replace, &row(), &peer());

    assert_eq!(planned.album_artist, "Various");
    assert_eq!(planned.genres, vec!["Vocaloid".to_owned()]);
    assert_eq!(planned.track_number, Some(7));
}

#[test]
fn a_track_number_the_row_carries_wins_under_replace() {
    let row = RowTags {
        track_number: Some(3),
        ..row()
    };

    assert_eq!(
        plan(TagPolicy::Replace, &row, &peer()).track_number,
        Some(3)
    );
}

/// An `stk get` row has no album, a title-only query no artist.
#[test]
fn a_blank_in_the_row_never_blanks_the_file() {
    let row = RowTags {
        artist: String::new(),
        album: String::new(),
        ..row()
    };

    let planned = plan(TagPolicy::Replace, &row, &peer());

    assert_eq!(planned.artist, "Unknown Artist");
    assert_eq!(planned.album, "www.something.ru");
    assert_eq!(planned.title, "Roki");
}

#[test]
fn fill_writes_only_the_fields_the_peer_left_empty() {
    let current = TrackTags {
        title: String::new(),
        track_number: None,
        ..peer()
    };
    let row = RowTags {
        track_number: Some(3),
        ..row()
    };

    let planned = plan(TagPolicy::Fill, &row, &current);

    assert_eq!(planned.title, "Roki");
    assert_eq!(planned.track_number, Some(3));
    assert_eq!(planned.artist, "Unknown Artist");
    assert_eq!(planned.album, "www.something.ru");
}

#[test]
fn keep_changes_nothing() {
    assert_eq!(plan(TagPolicy::Keep, &row(), &peer()), peer());
}

#[test]
fn an_untagged_file_ends_up_the_same_under_replace_and_fill() {
    let bare = TrackTags::default();

    assert_eq!(
        plan(TagPolicy::Replace, &row(), &bare),
        plan(TagPolicy::Fill, &row(), &bare)
    );
    assert_eq!(plan(TagPolicy::Fill, &row(), &bare).artist, "DECO*27");
}

#[test]
fn the_row_reads_its_track_number_out_of_the_export_columns() {
    let track = Track {
        source_row: vec![
            ("Track Name".to_owned(), "Roki".to_owned()),
            (" track number ".to_owned(), "3/12".to_owned()),
        ],
        ..Track::new("DECO*27", "Roki")
    };

    assert_eq!(RowTags::of(&track).track_number, Some(3));
}

#[test]
fn a_row_with_no_track_number_column_carries_none() {
    assert_eq!(
        RowTags::of(&Track::new("DECO*27", "Roki")).track_number,
        None
    );
}

#[test]
fn the_row_is_trimmed_before_it_is_compared() {
    let track = Track::new("  DECO*27 ", " Roki ");
    let row = RowTags::of(&track);

    assert_eq!(row.artist, "DECO*27");
    assert_eq!(row.title, "Roki");
}

#[test]
fn a_policy_round_trips_through_its_spelling() {
    for policy in TagPolicy::ALL {
        assert_eq!(policy.as_str().parse::<TagPolicy>(), Ok(policy));
        assert_eq!(policy.to_string(), policy.as_str());
    }
    assert!("overwrite".parse::<TagPolicy>().is_err());
}

#[test]
fn replace_is_the_default_policy() {
    assert_eq!(TagPolicy::default(), TagPolicy::Replace);
}
