//! The album grouping and the gain arithmetic behind `stk loudness`.
//!
//! All of it is pure, so what a gain figure means is established here rather
//! than through a decode. The measurement itself is in `loudness.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use stk::audio::loudness::Loudness;
use stk::config::LoudnessSettings;
use stk::models::{ReplayGainTags, TrackTags};
use stk::replaygain::{
    album_counts, album_key, format_gain, format_peak, format_r128, group, partial_records, plan,
    select, Scanned, Target,
};

fn settings() -> LoudnessSettings {
    LoudnessSettings::default()
}

fn loudness(integrated_lufs: f64, peak: f64) -> Loudness {
    Loudness {
        integrated_lufs,
        peak,
    }
}

fn tags(artist: &str, album: &str) -> TrackTags {
    TrackTags {
        artist: artist.to_owned(),
        album: album.to_owned(),
        title: "Something".to_owned(),
        ..TrackTags::default()
    }
}

fn target(path: &str, tags: TrackTags) -> Target {
    Target {
        path: PathBuf::from(path),
        tags,
        current: ReplayGainTags::default(),
        r128: false,
    }
}

/// One group's worth of scan, as the command hands it to `plan`.
fn scanned(album: Option<Loudness>, tracks: &[(Target, f64)]) -> Scanned {
    Scanned {
        album,
        tracks: tracks
            .iter()
            .map(|(target, integrated)| (target.clone(), Some(loudness(*integrated, 0.5))))
            .collect(),
    }
}

#[test]
fn a_gain_is_written_the_way_other_scanners_write_it() {
    assert_eq!(format_gain(-7.2949), "-7.29 dB");
    assert_eq!(format_gain(3.1), "3.10 dB");
    assert_eq!(format_peak(0.988_556_4), "0.988556");
}

/// The Opus header states its gain against a fixed -23 LUFS, whatever
/// reference the `REPLAYGAIN_*` figures used.
#[test]
fn an_opus_gain_is_the_q78_offset_from_minus_23_lufs() {
    // A file measured at -18 LUFS needs no gain to reach the -18 reference,
    // and -5 dB to reach -23.
    assert_eq!(format_r128(0.0, -18.0), (-5 * 256).to_string());
    assert_eq!(format_r128(-2.9, -18.0), "-2022");
    // Against a -23 reference the two spellings agree.
    assert_eq!(format_r128(4.0, -23.0), (4 * 256).to_string());
}

/// The field is a signed 16 bit integer: a wrapped value would be a loud file
/// asking to be made louder.
#[test]
fn an_absurd_opus_gain_saturates_rather_than_wrapping() {
    assert_eq!(format_r128(900.0, -18.0), i16::MAX.to_string());
    assert_eq!(format_r128(-900.0, -18.0), i16::MIN.to_string());
}

#[test]
fn an_album_gain_is_one_figure_across_its_tracks() {
    let changes = plan(
        &[scanned(
            Some(loudness(-13.0, 0.9)),
            &[
                (target("/music/a.flac", tags("ZUN", "Heian Alien")), -9.0),
                (target("/music/b.flac", tags("ZUN", "Heian Alien")), -21.0),
            ],
        )],
        &settings(),
    );

    assert_eq!(changes.len(), 2);
    assert_ne!(
        changes[0].proposed.track_gain,
        changes[1].proposed.track_gain
    );
    assert_eq!(changes[0].proposed.album_gain, Some("-5.00 dB".to_owned()));
    assert_eq!(
        changes[0].proposed.album_gain,
        changes[1].proposed.album_gain
    );
    assert_eq!(changes[0].proposed.album_peak, Some("0.900000".to_owned()));
}

#[test]
fn a_file_with_no_album_tag_gets_a_track_gain_and_nothing_else() {
    let changes = plan(
        &[scanned(
            None,
            &[(target("/music/single.flac", tags("ZUN", "")), -9.0)],
        )],
        &settings(),
    );

    assert_eq!(changes[0].proposed.track_gain, Some("-9.00 dB".to_owned()));
    assert_eq!(changes[0].proposed.album_gain, None);
    assert_eq!(changes[0].proposed.album_peak, None);
}

#[test]
fn two_albums_of_the_same_name_by_different_artists_stay_apart() {
    let groups = group(
        vec![
            target("/music/a.flac", tags("ZUN", "Greatest Hits")),
            target("/music/b.flac", tags("DECO*27", "Greatest Hits")),
        ],
        true,
    );

    assert_eq!(groups.len(), 2);
}

/// `normalize` drops a bracketed suffix, which would make a deluxe edition and
/// the record it expands one album. They are two, mastered separately.
#[test]
fn a_deluxe_edition_is_not_the_album_it_expands() {
    assert_ne!(
        album_key(&tags("ZUN", "Ghost Rule")),
        album_key(&tags("ZUN", "Ghost Rule (Deluxe Edition)"))
    );
}

/// An album gain describes a whole record. Measuring the half of an album that
/// has no gain yet would write an album figure derived from half of it, so the
/// skip is decided per record rather than per file.
#[test]
fn a_half_scanned_album_is_measured_entire() {
    let mut done = target("/music/a.flac", tags("ZUN", "Heian Alien"));
    done.current.track_gain = Some("-7.29 dB".to_owned());
    let pending = target("/music/b.flac", tags("ZUN", "Heian Alien"));

    let selection = select(group(vec![done, pending], true), false);

    assert_eq!(selection.wanted.len(), 1);
    assert_eq!(selection.wanted[0].targets.len(), 2);
    assert!(selection.skipped.is_empty());
}

/// An Opus file `rsgain` scanned in its native mode carries only the header
/// field. Re-measuring it would overwrite a scan.
#[test]
fn an_opus_file_carrying_only_r128_counts_as_scanned() {
    let mut scanned = target("/music/a.opus", tags("ZUN", ""));
    scanned.r128 = true;
    scanned.current.r128_track_gain = Some("-2022".to_owned());

    assert!(select(group(vec![scanned], true), false).wanted.is_empty());
}

/// An album gain describes a whole record, so naming one track of one has no
/// album figure to write: a figure measured from that track alone would tell a
/// player the record is as loud as its loudest song.
#[test]
fn a_run_covering_part_of_a_record_holds_no_album_figure_for_it() {
    let groups = group(
        vec![target("/music/a.flac", tags("ZUN", "Heian Alien"))],
        true,
    );
    let known: HashMap<(String, String), usize> =
        [(album_key(&tags("ZUN", "Heian Alien")).expect("a key"), 12)]
            .into_iter()
            .collect();

    let partial = partial_records(&groups, &known);

    assert_eq!(partial.len(), 1);
}

/// The whole-library run, and the folder nothing has scanned yet: an index that
/// has never heard of the record is a reason to scan, not to refuse a figure.
#[test]
fn a_record_the_index_does_not_know_is_taken_at_face_value() {
    let groups = group(
        vec![target("/music/a.flac", tags("ZUN", "Heian Alien"))],
        true,
    );

    assert!(partial_records(&groups, &album_counts(&[])).is_empty());
}
