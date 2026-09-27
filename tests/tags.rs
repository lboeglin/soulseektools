//! Reading and writing tags, covers, loudness tags and extras, per container.

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

/// Against tags ffmpeg wrote, so this isn't `lofty` checking `lofty`.
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
    // A cleared title must read back like one that never existed.
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
    // Rewriting the field would drop the total across an album.
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
    // ReplayGain written by other tools must survive a tag write.
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

/// Comments lofty would fold together or reformat, as real files carry them.
const ALIASES: &[(&str, &str)] = &[
    ("LABEL", "KARENT"),
    ("ORGANIZATION", "KARENT"),
    ("KEY", "F minor"),
    ("INITIALKEY", "F minor"),
    ("ORIGINALDATE", "2021-06-16"),
    ("ORIGINALYEAR", "2021"),
    ("tracknumber", "08/12"),
    ("discnumber", "02"),
];

fn vorbis_comments(path: &std::path::Path) -> lofty::ogg::VorbisComments {
    use lofty::file::AudioFile;
    let mut file = std::fs::File::open(path).expect("a readable file");
    let parse = lofty::config::ParseOptions::new();
    match path.extension().and_then(|e| e.to_str()) {
        Some("flac") => lofty::flac::FlacFile::read_from(&mut file, parse)
            .expect("a flac")
            .vorbis_comments()
            .cloned()
            .unwrap_or_default(),
        Some("ogg") => lofty::ogg::VorbisFile::read_from(&mut file, parse)
            .expect("an ogg")
            .vorbis_comments()
            .clone(),
        _ => lofty::ogg::OpusFile::read_from(&mut file, parse)
            .expect("an opus")
            .vorbis_comments()
            .clone(),
    }
}

fn set_vorbis_comments(path: &std::path::Path, items: &[(&str, &str)]) {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;
    let mut comments = lofty::ogg::VorbisComments::default();
    for (key, value) in items {
        comments.push((*key).to_owned(), (*value).to_owned());
    }
    let mut file = std::fs::File::open(path).expect("a readable file");
    let parse = lofty::config::ParseOptions::new();
    match path.extension().and_then(|e| e.to_str()) {
        Some("flac") => {
            let mut flac = lofty::flac::FlacFile::read_from(&mut file, parse).expect("a flac");
            flac.set_vorbis_comments(comments);
            drop(file);
            flac.save_to_path(path, WriteOptions::default())
        }
        Some("ogg") => {
            let mut ogg = lofty::ogg::VorbisFile::read_from(&mut file, parse).expect("an ogg");
            ogg.set_vorbis_comments(comments);
            drop(file);
            ogg.save_to_path(path, WriteOptions::default())
        }
        _ => {
            let mut opus = lofty::ogg::OpusFile::read_from(&mut file, parse).expect("an opus");
            opus.set_vorbis_comments(comments);
            drop(file);
            opus.save_to_path(path, WriteOptions::default())
        }
    }
    .expect("written comments");
}

#[test]
fn writes_leave_every_other_vorbis_comment_exactly_as_it_was() {
    // Regression: every write went through lofty's generic tag, which merged
    // ORGANIZATION into LABEL and KEY into INITIALKEY, added ENCODER, and
    // rewrote TRACKNUMBER=08/12 as 8 plus a TRACKTOTAL.
    let scratch = Scratch::new("tags-vorbis-raw");
    for suffix in ["flac", "ogg", "opus"] {
        let path = encode(&scratch.join(&format!("t.{suffix}")), &Encoded::default());
        let mut items = ALIASES.to_vec();
        items.push(("TITLE", "Heian Alien"));
        set_vorbis_comments(&path, &items);
        let before = vorbis_comments(&path);

        let mut edited = tags::read_tags(&path).expect("readable tags");
        edited.title = "Native Faith".to_owned();
        tags::write_tags(&path, &edited).expect("written tags");
        tags::write_genres(&path, &["Touhou".to_owned()]).expect("written genres");
        tags::write_replay_gain(&path, &measurement()).expect("written gain");
        tags::write_artwork(&path, &artwork(1)).expect("written cover");
        let extras = tags::read_extras(&path).expect("readable extras");
        tags::write_extras(&path, &extras).expect("written extras");

        let after = vorbis_comments(&path);
        let untouched = |comments: &lofty::ogg::VorbisComments| -> Vec<(String, String)> {
            comments
                .items()
                .filter(|(key, _)| ALIASES.iter().any(|(alias, _)| alias == key))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect()
        };
        assert_eq!(untouched(&after), untouched(&before), "{suffix}");
        // lofty's reader reformats some on the way in, so check the bytes too.
        let bytes = std::fs::read(&path).expect("a readable file");
        for (key, value) in ALIASES {
            let stored = format!("{key}={value}");
            assert!(
                bytes
                    .windows(stored.len())
                    .any(|window| window == stored.as_bytes()),
                "{suffix}: {stored}"
            );
        }
        assert_eq!(after.vendor(), before.vendor(), "{suffix}");
        assert_eq!(after.get("ENCODER"), None, "{suffix}");
        assert_eq!(after.get("TITLE"), Some("Native Faith"), "{suffix}");
    }
}

#[test]
fn an_mp3_gain_write_updates_the_frame_whatever_its_case_and_keeps_the_rest() {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::id3::v2::{CommentFrame, Frame, Id3v2Tag};
    use lofty::mpeg::MpegFile;
    use lofty::TextEncoding;

    let read = |path: &std::path::Path| {
        let mut file = std::fs::File::open(path).expect("a readable file");
        MpegFile::read_from(&mut file, ParseOptions::new()).expect("an mp3")
    };
    let scratch = Scratch::new("tags-id3-raw");
    let path = encode(&scratch.join("t.mp3"), &Encoded::default());
    let mut mp3 = read(&path);
    let mut tag = Id3v2Tag::default();
    tag.insert_user_text("replaygain_track_peak".to_owned(), "1.000000".to_owned());
    tag.insert(Frame::Comment(CommentFrame::new(
        TextEncoding::UTF8,
        *b"jpn",
        String::new(),
        "コメント".to_owned(),
    )));
    mp3.set_id3v2(tag);
    mp3.save_to_path(&path, WriteOptions::default())
        .expect("a written tag");

    tags::write_replay_gain(&path, &measurement()).expect("written gain");

    let tag = read(&path).id3v2().cloned().expect("an ID3v2 tag");
    let peaks: Vec<(String, String)> = (&tag)
        .into_iter()
        .filter_map(|frame| match frame {
            Frame::UserText(text)
                if text
                    .description
                    .eq_ignore_ascii_case("REPLAYGAIN_TRACK_PEAK") =>
            {
                Some((text.description.clone(), text.content.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        peaks,
        [("replaygain_track_peak".to_owned(), "0.988556".to_owned())]
    );
    let comment = tag.comments().next().expect("the comment");
    assert_eq!(
        (comment.language, comment.content.as_str()),
        (*b"jpn", "コメント")
    );
}

#[test]
fn an_mp4_write_keeps_one_byte_integer_atoms_one_byte_wide() {
    // Regression: lofty rewrote rtng as four bytes, which players read as 0.
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::mp4::{AdvisoryRating, Mp4File};

    let scratch = Scratch::new("tags-mp4-raw");
    let path = encode(&scratch.join("t.m4a"), &Encoded::default());
    let mut file = std::fs::File::open(&path).expect("a readable file");
    let mut mp4 = Mp4File::read_from(&mut file, ParseOptions::new()).expect("an m4a");
    drop(file);
    let mut ilst = mp4.ilst().cloned().unwrap_or_default();
    ilst.set_advisory_rating(AdvisoryRating::Clean);
    mp4.set_ilst(ilst);
    mp4.save_to_path(&path, WriteOptions::default())
        .expect("a written tag");

    tags::write_replay_gain(&path, &measurement()).expect("written gain");

    let bytes = std::fs::read(&path).expect("a readable file");
    let at = bytes
        .windows(4)
        .position(|window| window == b"rtng")
        .expect("an rtng atom");
    let size = u32::from_be_bytes(bytes[at - 4..at].try_into().expect("four bytes"));
    // Atom header, data header, then the one byte.
    assert_eq!(size, 8 + 16 + 1);
    assert_eq!(bytes[at + 20], 2);
}

#[test]
fn an_mp3_whose_comment_has_no_language_can_still_be_written() {
    // Regression: lofty reads a COMM frame with a zeroed language but refuses to
    // save it, so every write to such a file failed.
    let scratch = Scratch::new("tags-id3-language");
    let path = encode(&scratch.join("t.mp3"), &Encoded::default());
    let mut frame = vec![3, 0, 0, 0, 0];
    frame.extend(b"kept");
    let mut tag = b"ID3\x04\x00\x00".to_vec();
    tag.extend(syncsafe(10 + frame.len()));
    tag.extend(b"COMM");
    tag.extend(syncsafe(frame.len()));
    tag.extend([0, 0]);
    tag.extend(&frame);
    let audio = std::fs::read(&path).expect("a readable file");
    std::fs::write(&path, [tag, audio].concat()).expect("a written file");

    tags::write_genres(&path, &["Vocaloid".to_owned()]).expect("a written genre");

    let file = lofty::read_from_path(&path).expect("a readable file");
    let tag = file.primary_tag().expect("a tag");
    assert_eq!(tag.get_string(&ItemKey::Comment), Some("kept"));
    assert_eq!(tag.get_string(&ItemKey::Genre), Some("Vocaloid"));
}

fn syncsafe(size: usize) -> [u8; 4] {
    let size = u32::try_from(size).expect("a small frame");
    [3, 2, 1, 0].map(|shift| u8::try_from((size >> (7 * shift)) & 0x7f).expect("seven bits"))
}
