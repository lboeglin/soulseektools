//! Ported from `py/tests/unit/test_tags.py`.

mod support;

use stk::library::tags;
use stk::models::Artwork;
use support::{encode, Encoded, Scratch};

const TAGGABLE: &[&str] = &["flac", "mp3", "m4a", "ogg", "opus"];

const PNG_PIXEL: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x00, 0x00, 0x02,
    0x00, 0x01, 0x00, 0xff, 0xff, 0x03, 0x00, 0x00, 0x06, 0x00, 0x05, 0x57, 0xbf, 0xab, 0xd4, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

fn artwork(repeats: usize) -> Artwork {
    Artwork {
        data: PNG_PIXEL.repeat(repeats),
        mime: "image/png".to_owned(),
    }
}

#[test]
fn read_tags_returns_the_same_fields_for_every_container() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-read");

    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded {
                metadata: &[
                    ("artist", "DECO*27"),
                    ("album", "Ghost"),
                    ("title", "Roki"),
                    ("genre", "Vocaloid"),
                    ("track", "3"),
                ],
                ..Encoded::default()
            },
        );

        let read = tags::read_tags(&path).expect("readable tags");
        assert_eq!(read.artist, "DECO*27", "{suffix}");
        assert_eq!(read.album, "Ghost", "{suffix}");
        assert_eq!(read.title, "Roki", "{suffix}");
        assert_eq!(read.genres, vec!["Vocaloid".to_owned()], "{suffix}");
        assert_eq!(read.track_number, Some(3), "{suffix}");
    }
}

#[test]
fn read_tags_treats_an_untagged_file_as_empty_rather_than_an_error() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-untagged");

    for suffix in TAGGABLE.iter().chain(["wav"].iter()) {
        let path = encode(
            &scratch.join(&format!("bare.{suffix}")),
            &Encoded::default(),
        );
        assert_eq!(
            tags::read_tags(&path).expect("readable tags").artist,
            "",
            "{suffix}"
        );
    }
}

#[test]
fn read_duration_comes_from_the_container_header() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-duration");

    for suffix in TAGGABLE.iter().chain(["wav"].iter()) {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded {
                seconds: 2.0,
                ..Encoded::default()
            },
        );

        let duration = tags::read_duration(&path)
            .expect("a readable header")
            .expect("a duration");
        assert!((duration - 2.0).abs() < 0.15, "{suffix}: {duration}");
    }
}

#[test]
fn write_genres_round_trips() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-genres");

    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded::default(),
        );
        tags::write_genres(&path, &["Touhou".to_owned(), "Eurobeat".to_owned()])
            .expect("written genres");

        assert_eq!(
            tags::read_tags(&path).expect("readable tags").genres,
            vec!["Touhou".to_owned(), "Eurobeat".to_owned()],
            "{suffix}"
        );
    }
}

#[test]
fn write_genres_replaces_rather_than_appends() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-genres-replace");

    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded {
                metadata: &[("genre", "Pop")],
                ..Encoded::default()
            },
        );
        tags::write_genres(&path, &["Vocaloid".to_owned()]).expect("written genres");

        assert_eq!(
            tags::read_tags(&path).expect("readable tags").genres,
            vec!["Vocaloid".to_owned()],
            "{suffix}"
        );
    }
}

#[test]
fn artwork_round_trips() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-artwork");
    let cover = artwork(40);

    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded::default(),
        );
        assert!(
            tags::read_artwork(&path)
                .expect("a readable file")
                .is_none(),
            "{suffix}"
        );

        tags::write_artwork(&path, &cover).expect("written artwork");

        let embedded = tags::read_artwork(&path)
            .expect("a readable file")
            .expect("an embedded cover");
        assert_eq!(embedded.data, cover.data, "{suffix}");
        assert_eq!(embedded.mime, cover.mime, "{suffix}");
    }
}

#[test]
fn write_artwork_replaces_the_existing_cover() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-artwork-replace");
    let cover = artwork(40);
    let replacement = artwork(80);

    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded::default(),
        );
        tags::write_artwork(&path, &cover).expect("written artwork");
        tags::write_artwork(&path, &replacement).expect("replaced artwork");

        let embedded = tags::read_artwork(&path)
            .expect("a readable file")
            .expect("an embedded cover");
        assert_eq!(embedded.data.len(), replacement.data.len(), "{suffix}");
    }
}

#[test]
fn artwork_size_reports_zero_when_there_is_no_cover() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-artwork-zero");

    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded::default(),
        );
        assert_eq!(tags::artwork_size(&path), 0, "{suffix}");
    }
}

#[test]
fn artwork_size_matches_the_embedded_bytes() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-artwork-size");
    let cover = artwork(40);

    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("track.{suffix}")),
            &Encoded::default(),
        );
        tags::write_artwork(&path, &cover).expect("written artwork");

        assert_eq!(
            tags::artwork_size(&path),
            cover.data.len() as u64,
            "{suffix}"
        );
    }
}

#[test]
fn artwork_size_is_zero_for_a_container_that_cannot_hold_a_cover() {
    needs_ffmpeg!();
    // The original scripts raised or crashed here; callers only ever want a number.
    let scratch = Scratch::new("tags-artwork-wav");
    let path = encode(&scratch.join("track.wav"), &Encoded::default());

    assert_eq!(tags::artwork_size(&path), 0);
}

#[test]
fn reading_artwork_from_an_unsupported_container_is_explicit() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-artwork-unsupported");
    let path = encode(&scratch.join("track.wav"), &Encoded::default());

    let error = tags::read_artwork(&path).expect_err("an unsupported container");
    assert!(
        matches!(error, stk::errors::Error::UnsupportedFormat(_)),
        "{error}"
    );
}

#[test]
fn reading_a_non_audio_file_is_explicit() {
    let scratch = Scratch::new("tags-not-audio");
    let path = scratch.join("notes.txt");
    std::fs::write(&path, "not audio").expect("a written file");

    let error = tags::read_tags(&path).expect_err("a non-audio file");
    assert!(
        matches!(error, stk::errors::Error::UnsupportedFormat(_)),
        "{error}"
    );
}

#[test]
fn every_taggable_extension_is_covered_by_these_tests() {
    let covered: std::collections::HashSet<String> =
        TAGGABLE.iter().map(|suffix| format!(".{suffix}")).collect();
    let declared: std::collections::HashSet<String> = tags::TAGGABLE_EXTENSIONS
        .iter()
        .map(|extension| (*extension).to_owned())
        .filter(|extension| extension != ".mp4")
        .collect();

    assert_eq!(covered, declared);
}
