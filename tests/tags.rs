//! Reading and writing tags, covers, loudness tags and extras per container.

mod support;

use lofty::prelude::{ItemKey, TaggedFileExt};
use stk::library::tags::{self};
use stk::models::{Artwork, ReplayGainTags, TrackTags};
use support::{committed, encode, Encoded, Scratch};

const TAGGABLE: &[&str] = &["flac", "mp3", "m4a", "ogg", "opus"];

const PNG_PIXEL: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x00, 0x00, 0x02,
    0x00, 0x01, 0x00, 0xff, 0xff, 0x03, 0x00, 0x00, 0x06, 0x00, 0x05, 0x57, 0xbf, 0xab, 0xd4, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn artwork(repeats: usize) -> Artwork {
    Artwork {
        data: PNG_PIXEL.repeat(repeats),
        mime: "image/png".to_owned(),
    }
}

/// Against tags an independent writer produced.
///
/// The committed `tagged.*` fixtures carry what ffmpeg wrote into each
/// container; a fixture this suite tagged itself would be checking `lofty`
/// against `lofty` and would agree whatever either of them did.
#[test]
fn read_tags_returns_the_same_fields_for_every_container() {
    for suffix in TAGGABLE {
        let path = committed(&format!("tagged.{suffix}"));

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
fn write_artwork_replaces_the_existing_cover() {
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
fn reading_artwork_from_an_unsupported_container_is_explicit() {
    let scratch = Scratch::new("tags-artwork-unsupported");
    let path = encode(&scratch.join("track.wav"), &Encoded::default());

    let error = tags::read_artwork(&path).expect_err("an unsupported container");
    assert!(
        matches!(error, stk::errors::Error::UnsupportedFormat(_)),
        "{error}"
    );
}

fn full_tags() -> TrackTags {
    TrackTags {
        artist: "DECO*27".to_owned(),
        album_artist: "DECO*27".to_owned(),
        album: "Ghost".to_owned(),
        title: "Roki".to_owned(),
        genres: vec!["Vocaloid".to_owned()],
        track_number: Some(3),
    }
}

#[test]
fn every_taggable_container_round_trips_written_tags() {
    let scratch = Scratch::new("tags-write-round-trip");

    for suffix in TAGGABLE {
        let path = encode(&scratch.join(&format!("t.{suffix}")), &Encoded::default());
        tags::write_tags(&path, &full_tags()).expect("written tags");

        assert_eq!(
            tags::read_tags(&path).expect("readable tags"),
            full_tags(),
            "{suffix}"
        );
    }
}

#[test]
fn writing_one_field_leaves_the_others_where_they_were() {
    let scratch = Scratch::new("tags-write-one");
    let path = encode(&scratch.join("t.flac"), &Encoded::default());
    tags::write_tags(&path, &full_tags()).expect("written tags");

    let mut edited = full_tags();
    edited.title = "Ghost Rule".to_owned();
    tags::write_tags(&path, &edited).expect("an edited title");

    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.title, "Ghost Rule");
    assert_eq!(read.artist, "DECO*27");
    assert_eq!(read.album, "Ghost");
    assert_eq!(read.track_number, Some(3));
    assert_eq!(read.genres, vec!["Vocaloid".to_owned()]);
}

#[test]
fn an_empty_value_removes_the_key_rather_than_writing_a_blank() {
    // A file whose title was cleared has to read back the way one that never
    // had a title does, or "is this tagged" gets two different answers.
    let scratch = Scratch::new("tags-write-clear");
    let path = encode(&scratch.join("t.flac"), &Encoded::default());
    tags::write_tags(&path, &full_tags()).expect("written tags");

    tags::write_tags(&path, &TrackTags::default()).expect("cleared tags");

    assert_eq!(
        tags::read_tags(&path).expect("readable tags"),
        TrackTags::default()
    );
}

#[test]
fn writing_tags_leaves_the_embedded_cover_alone() {
    let scratch = Scratch::new("tags-write-artwork");
    let path = encode(&scratch.join("t.flac"), &Encoded::default());
    tags::write_artwork(&path, &artwork(1)).expect("an embedded cover");

    tags::write_tags(&path, &full_tags()).expect("written tags");

    assert_eq!(
        tags::read_artwork(&path).expect("readable artwork"),
        Some(artwork(1))
    );
}

#[test]
fn a_total_after_the_track_number_survives_a_write_that_keeps_the_number() {
    // `read_tags` keeps only the leading digits of "3/12", so a write that
    // rewrote the field regardless would drop the total across a whole album
    // for anyone editing only a title.
    let scratch = Scratch::new("tags-write-track");
    let path = encode(
        &scratch.join("t.flac"),
        &Encoded {
            metadata: &[("track", "3/12"), ("title", "Roki")],
            ..Encoded::default()
        },
    );
    assert_eq!(
        tags::read_tags(&path).expect("readable tags").track_number,
        Some(3)
    );

    let mut edited = tags::read_tags(&path).expect("readable tags");
    edited.title = "Ghost Rule".to_owned();
    tags::write_tags(&path, &edited).expect("written tags");

    assert_eq!(
        tags::read_tags(&path).expect("readable tags").track_number,
        Some(3)
    );
    let file = lofty::read_from_path(&path).expect("a readable file");
    let tag = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .expect("a tag");
    assert_eq!(tag.get_string(&ItemKey::TrackTotal), Some("12"));
}

#[test]
fn a_tag_this_project_does_not_manage_survives_a_write() {
    // ReplayGain is the case that matters: a library scanned in foobar2000
    // carries four of these per file, and `write_tags` replaces six item keys
    // rather than the tag block, so everything else has to come through
    // untouched. Losing them would mean rescanning the whole library.
    let scratch = Scratch::new("tags-foreign");
    let path = encode(
        &scratch.join("t.flac"),
        &Encoded {
            metadata: &[
                ("title", "Heian Alien"),
                ("REPLAYGAIN_TRACK_GAIN", "-7.29 dB"),
                ("REPLAYGAIN_TRACK_PEAK", "0.988556"),
            ],
            ..Encoded::default()
        },
    );

    tags::write_tags(&path, &full_tags()).expect("written tags");

    let file = lofty::read_from_path(&path).expect("a readable file");
    let tag = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .expect("a tag");
    assert_eq!(
        tag.get_string(&ItemKey::ReplayGainTrackGain),
        Some("-7.29 dB")
    );
    assert_eq!(
        tag.get_string(&ItemKey::ReplayGainTrackPeak),
        Some("0.988556")
    );
    assert_eq!(tags::read_tags(&path).expect("readable tags"), full_tags());
}

fn measurement() -> ReplayGainTags {
    ReplayGainTags {
        track_gain: Some("-7.29 dB".to_owned()),
        track_peak: Some("0.988556".to_owned()),
        album_gain: Some("-6.25 dB".to_owned()),
        album_peak: Some("0.995000".to_owned()),
        r128_track_gain: None,
        r128_album_gain: None,
    }
}

#[test]
fn a_loudness_measurement_round_trips_through_every_container() {
    let scratch = Scratch::new("tags-loudness");
    for extension in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("t.{extension}")),
            &Encoded::default(),
        );
        let mut written = measurement();
        if tags::describe_file(&path).expect("a described file").r128 {
            written.r128_track_gain = Some("-2022".to_owned());
            written.r128_album_gain = Some("-1600".to_owned());
        }

        tags::write_replay_gain(&path, &written).expect("a loudness write");

        assert_eq!(
            tags::read_replay_gain(&path).expect("readable loudness tags"),
            written,
            "{extension}"
        );
    }
}

/// Opus keeps its gain in the comment header as `R128_TRACK_GAIN`, and that is
/// what players read there.
#[test]
fn an_opus_file_carries_r128_alongside_the_standard_tags() {
    let scratch = Scratch::new("tags-loudness-opus");
    let path = encode(&scratch.join("t.opus"), &Encoded::default());
    let mut written = measurement();
    written.r128_track_gain = Some("-2022".to_owned());

    tags::write_replay_gain(&path, &written).expect("a loudness write");

    let file = lofty::read_from_path(&path).expect("a readable file");
    let tag = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .expect("a tag");
    assert_eq!(
        tag.get_string(&ItemKey::Unknown("R128_TRACK_GAIN".to_owned())),
        Some("-2022")
    );
    assert_eq!(
        tag.get_string(&ItemKey::ReplayGainTrackGain),
        Some("-7.29 dB")
    );
}

/// What a Soulseek peer's tagger typically leaves beside the fields that matter.
fn peer_block() -> Encoded<'static> {
    Encoded {
        metadata: &[
            ("artist", "Unknown Artist"),
            ("title", "Track 01"),
            ("genre", "Vocaloid"),
            ("disc", "2"),
            ("comment", "ripped by someone"),
            ("REPLAYGAIN_TRACK_GAIN", "-7.29 dB"),
        ],
        ..Encoded::default()
    }
}

#[test]
fn stripping_the_extras_keeps_every_managed_field_and_the_cover() {
    let scratch = Scratch::new("tags-extras-strip");
    let path = encode(&scratch.join("t.flac"), &peer_block());
    tags::write_artwork(&path, &artwork(1)).expect("an embedded cover");

    tags::write_extras(&path, &[]).expect("stripped extras");

    assert!(tags::read_extras(&path)
        .expect("readable extras")
        .is_empty());
    let read = tags::read_tags(&path).expect("readable tags");
    assert_eq!(read.artist, "Unknown Artist");
    assert_eq!(read.title, "Track 01");
    assert_eq!(read.genres, vec!["Vocaloid".to_owned()]);
    assert_eq!(
        tags::read_replay_gain(&path)
            .expect("readable loudness tags")
            .track_gain
            .as_deref(),
        Some("-7.29 dB")
    );
    assert_eq!(
        tags::read_artwork(&path).expect("readable artwork"),
        Some(artwork(1))
    );
    let file = lofty::read_from_path(&path).expect("a readable file");
    let tag = file.primary_tag().expect("a tag");
    assert_eq!(tag.get_string(&ItemKey::DiscNumber), Some("2"));
}

#[test]
fn extras_round_trip_through_every_container() {
    let scratch = Scratch::new("tags-extras-round-trip");
    for suffix in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("t.{suffix}")),
            &Encoded {
                metadata: &[("title", "Roki"), ("comment", "ripped by someone")],
                ..Encoded::default()
            },
        );
        let before = tags::read_extras(&path).expect("readable extras");
        assert_eq!(before.len(), 1, "{suffix}: {before:?}");

        tags::write_extras(&path, &[]).expect("stripped extras");
        assert!(
            tags::read_extras(&path)
                .expect("readable extras")
                .is_empty(),
            "{suffix}"
        );

        tags::write_extras(&path, &before).expect("restored extras");
        assert_eq!(
            tags::read_extras(&path).expect("readable extras"),
            before,
            "{suffix}"
        );
        assert_eq!(
            tags::read_tags(&path).expect("readable tags").title,
            "Roki",
            "{suffix}"
        );
    }
}

#[test]
fn a_vbr_mp3_is_told_from_a_cbr_one_by_its_first_frame() {
    assert!(tags::mp3_is_vbr(&committed("noise-v0.mp3")));
    assert!(tags::mp3_is_vbr(&committed("noise-128k-as-vbr320.mp3")));
    assert!(!tags::mp3_is_vbr(&committed("noise-320k.mp3")));
    assert!(!tags::mp3_is_vbr(&committed("tagged.mp3")));
}
