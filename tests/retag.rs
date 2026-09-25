//! What a fetched file is tagged with; the write itself is covered in `fetch.rs`.

use stk::models::{Track, TrackTags};
use stk::retag::{plan, RowTags, TagPolicy};

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
        title_guessed: false,
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
fn a_guessed_title_does_not_overwrite_the_one_the_peer_wrote() {
    let guessed = RowTags {
        title_guessed: true,
        ..row()
    };

    let planned = plan(TagPolicy::Replace, &guessed, &peer());

    assert_eq!(planned.title, "Track 01");
    // Only the title is demoted; artist and album came from the request.
    assert_eq!(planned.artist, "DECO*27");
    assert_eq!(planned.album, "Ghost");
}

#[test]
fn a_guessed_title_still_fills_a_file_that_has_none() {
    let guessed = RowTags {
        title_guessed: true,
        ..row()
    };
    let untitled = TrackTags {
        title: String::new(),
        ..peer()
    };

    assert_eq!(plan(TagPolicy::Replace, &guessed, &untitled).title, "Roki");
}

#[test]
fn a_track_column_holding_the_title_is_not_a_track_number() {
    let track = Track {
        source_row: vec![("track".to_owned(), "22".to_owned())],
        ..Track::new("Taylor Swift", "22")
    };

    assert_eq!(RowTags::of(&track).track_number, None);
}

#[test]
fn a_policy_round_trips_through_its_spelling() {
    for policy in TagPolicy::ALL {
        assert_eq!(policy.as_str().parse::<TagPolicy>(), Ok(policy));
        assert_eq!(policy.to_string(), policy.as_str());
    }
    assert!("overwrite".parse::<TagPolicy>().is_err());
    assert_eq!(TagPolicy::default(), TagPolicy::Replace);
}
