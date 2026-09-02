//! Ported from `py/tests/unit/test_tags.py`.

mod support;

use lofty::prelude::{ItemKey, TaggedFileExt};
use stk::library::tags::{self, TagField};
use stk::models::{Artwork, ReplayGainTags, TrackTags};
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
fn writing_tags_creates_a_tag_block_in_a_file_that_has_none() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-write-untagged");
    let path = encode(&scratch.join("t.flac"), &Encoded::default());

    tags::write_tags(&path, &full_tags()).expect("written tags");

    assert_eq!(tags::read_tags(&path).expect("readable tags"), full_tags());
}

#[test]
fn writing_tags_leaves_the_embedded_cover_alone() {
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
fn a_changed_track_number_leaves_the_total_where_it_was() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-write-track-changed");
    let path = encode(
        &scratch.join("t.flac"),
        &Encoded {
            metadata: &[("track", "3/12")],
            ..Encoded::default()
        },
    );

    let mut edited = tags::read_tags(&path).expect("readable tags");
    edited.track_number = Some(4);
    tags::write_tags(&path, &edited).expect("written tags");

    let file = lofty::read_from_path(&path).expect("a readable file");
    let tag = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .expect("a tag");
    assert_eq!(tag.get_string(&ItemKey::TrackNumber), Some("4"));
}

#[test]
fn a_field_reads_back_the_value_a_dump_spelled() {
    let tags = full_tags();

    assert_eq!(TagField::Artist.value(&tags), "DECO*27");
    assert_eq!(TagField::TrackNumber.value(&tags), "3");
    assert_eq!(TagField::Genres.value(&tags), "Vocaloid");
    assert_eq!(TagField::TrackNumber.value(&TrackTags::default()), "");
}

#[test]
fn several_genres_share_one_cell_and_come_back_in_order() {
    let mut tags = TrackTags::default();
    TagField::Genres.set(&mut tags, "Touhou; Eurobeat");

    assert_eq!(
        tags.genres,
        vec!["Touhou".to_owned(), "Eurobeat".to_owned()]
    );
    assert_eq!(TagField::Genres.value(&tags), "Touhou; Eurobeat");
}

#[test]
fn a_track_number_that_is_not_a_number_leaves_the_field_empty() {
    let mut tags = full_tags();
    TagField::TrackNumber.set(&mut tags, "side B");

    assert_eq!(tags.track_number, None);
}

#[test]
fn a_field_name_round_trips_through_its_spelling() {
    for field in TagField::ALL {
        assert_eq!(field.as_str().parse::<TagField>(), Ok(field));
    }
    assert!("nonsense".parse::<TagField>().is_err());
}

#[test]
fn a_tag_this_project_does_not_manage_survives_a_write() {
    // ReplayGain is the case that matters: a library scanned in foobar2000
    // carries four of these per file, and `write_tags` replaces six item keys
    // rather than the tag block, so everything else has to come through
    // untouched. Losing them would mean rescanning the whole library.
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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

#[test]
fn a_file_nothing_has_scanned_carries_no_loudness_tags() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-loudness-absent");
    let path = encode(&scratch.join("t.flac"), &Encoded::default());

    let read = tags::read_replay_gain(&path).expect("readable loudness tags");

    assert!(read.is_empty());
    assert!(!read.is_scanned());
}

/// Undoing a first-ever scan means removing the tags, not writing empty ones.
#[test]
fn clearing_a_measurement_leaves_the_file_the_way_it_started() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-loudness-clear");
    let path = encode(&scratch.join("t.flac"), &Encoded::default());
    tags::write_replay_gain(&path, &measurement()).expect("a loudness write");

    tags::write_replay_gain(&path, &ReplayGainTags::default()).expect("a cleared measurement");

    assert!(tags::read_replay_gain(&path)
        .expect("readable loudness tags")
        .is_empty());
}

/// The inverse of `a_tag_this_project_does_not_manage_survives_a_write`: a scan
/// must not cost a file its artist, and a tag edit must not cost it its scan.
#[test]
fn a_scan_and_a_tag_edit_leave_each_other_alone() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-loudness-coexist");
    let path = encode(
        &scratch.join("t.flac"),
        &Encoded {
            metadata: &[("title", "Heian Alien")],
            ..Encoded::default()
        },
    );

    tags::write_replay_gain(&path, &measurement()).expect("a loudness write");
    assert_eq!(
        tags::read_tags(&path).expect("readable tags").title,
        "Heian Alien"
    );

    tags::write_tags(&path, &full_tags()).expect("written tags");
    assert_eq!(
        tags::read_replay_gain(&path).expect("readable loudness tags"),
        measurement()
    );
}

/// Opus keeps its gain in the comment header as `R128_TRACK_GAIN`, and that is
/// what players read there.
#[test]
fn an_opus_file_carries_r128_alongside_the_standard_tags() {
    needs_ffmpeg!();
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

/// Decided by the container the parse identified, not by a second guess at the
/// filename. The two agree today -- `lofty` commits to a container from the
/// suffix, so a misnamed file does not reach this at all -- but only one of
/// them can ever disagree with what was actually read, and
/// [`write_replay_gain`] replaces the whole set rather than patching it, so
/// disagreeing would take an `R128_TRACK_GAIN` away without writing one back.
#[test]
fn an_r128_field_is_asked_of_opus_and_of_nothing_else() {
    needs_ffmpeg!();
    let scratch = Scratch::new("tags-r128-container");
    for extension in TAGGABLE {
        let path = encode(
            &scratch.join(&format!("t.{extension}")),
            &Encoded::default(),
        );
        let described = tags::describe_file(&path).expect("a described file");

        assert_eq!(described.r128, *extension == "opus", "{extension}");
    }
}
