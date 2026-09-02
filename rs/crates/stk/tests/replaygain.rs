//! The album grouping and the gain arithmetic behind `stk loudness` (#7).
//!
//! All of it is pure, so what a gain figure means is established here rather
//! than through a decode. The measurement itself is in `loudness.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use stk::audio::loudness::Loudness;
use stk::config::LoudnessSettings;
use stk::models::{ReplayGainTags, TrackTags};
use stk::replaygain::{
    album_counts, album_key, format_gain, format_peak, format_r128, gain_db, group,
    partial_records, plan, select, Scanned, Target,
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
fn a_gain_is_the_reference_less_the_measured_loudness() {
    assert!((gain_db(-9.0, -18.0) - -9.0).abs() < 1e-9);
    assert!((gain_db(-23.0, -18.0) - 5.0).abs() < 1e-9);
}

#[test]
fn a_quieter_track_earns_a_larger_gain() {
    let loud = gain_db(-9.0, -18.0);
    let quiet = gain_db(-21.0, -18.0);

    assert!(quiet > loud);
    assert!((quiet - loud - 12.0).abs() < 1e-9);
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
fn a_file_that_could_not_be_measured_is_planned_for_nothing() {
    let changes = plan(
        &[Scanned {
            album: None,
            tracks: vec![(target("/music/broken.flac", tags("ZUN", "Ghost")), None)],
        }],
        &settings(),
    );

    assert!(changes.is_empty());
}

#[test]
fn an_opus_file_is_planned_both_spellings_and_everything_else_only_the_standard_ones() {
    let mut opus = target("/music/a.opus", tags("ZUN", "Heian Alien"));
    opus.r128 = true;
    let flac = target("/music/b.flac", tags("ZUN", "Heian Alien"));

    let changes = plan(
        &[scanned(
            Some(loudness(-13.0, 0.9)),
            &[(opus, -9.0), (flac, -9.0)],
        )],
        &settings(),
    );

    assert!(changes[0].proposed.r128_track_gain.is_some());
    assert!(changes[0].proposed.r128_album_gain.is_some());
    assert_eq!(changes[1].proposed.r128_track_gain, None);
    assert_eq!(changes[1].proposed.r128_album_gain, None);
}

#[test]
fn the_tracks_of_one_album_are_gathered_into_one_group() {
    let groups = group(
        vec![
            target("/music/a.flac", tags("ZUN", "Heian Alien")),
            target("/music/b.flac", tags("ZUN", "Heian Alien")),
        ],
        true,
    );

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].targets.len(), 2);
    assert!(groups[0].album.is_some());
}

#[test]
fn a_file_with_no_album_tag_is_a_group_of_its_own() {
    let groups = group(
        vec![
            target("/music/a.flac", tags("ZUN", "")),
            target("/music/b.flac", tags("ZUN", "")),
        ],
        true,
    );

    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|group| group.album.is_none()));
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

#[test]
fn an_album_artist_decides_the_group_where_one_is_tagged() {
    let mut compilation = tags("Some Guest", "Various");
    compilation.album_artist = "ZUN".to_owned();

    assert_eq!(album_key(&compilation), album_key(&tags("ZUN", "Various")));
}

/// `--track-only`: nothing is measured against anything else.
#[test]
fn track_only_makes_every_file_a_group_of_one() {
    let groups = group(
        vec![
            target("/music/a.flac", tags("ZUN", "Heian Alien")),
            target("/music/b.flac", tags("ZUN", "Heian Alien")),
        ],
        false,
    );

    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|group| group.album.is_none()));
}

/// The point of the default: a library somebody scanned in foobar2000 is not
/// re-measured out from under them.
#[test]
fn a_file_that_already_carries_a_gain_is_left_alone() {
    let mut scanned = target("/music/a.flac", tags("ZUN", ""));
    scanned.current.track_gain = Some("-7.29 dB".to_owned());

    let selection = select(group(vec![scanned], true), false);

    assert!(selection.wanted.is_empty());
    assert_eq!(selection.skipped.len(), 1);
}

#[test]
fn force_measures_a_file_that_already_carries_a_gain() {
    let mut scanned = target("/music/a.flac", tags("ZUN", ""));
    scanned.current.track_gain = Some("-7.29 dB".to_owned());

    let selection = select(group(vec![scanned], true), true);

    assert_eq!(selection.wanted.len(), 1);
    assert!(selection.skipped.is_empty());
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

#[test]
fn a_fully_scanned_album_is_left_alone() {
    let mut first = target("/music/a.flac", tags("ZUN", "Heian Alien"));
    first.current.track_gain = Some("-7.29 dB".to_owned());
    let mut second = target("/music/b.flac", tags("ZUN", "Heian Alien"));
    second.current.track_gain = Some("-4.10 dB".to_owned());

    let selection = select(group(vec![first, second], true), false);

    assert!(selection.wanted.is_empty());
    assert_eq!(selection.skipped.len(), 2);
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

#[test]
fn a_run_covering_a_whole_record_holds_its_album_figure() {
    let groups = group(
        vec![
            target("/music/a.flac", tags("ZUN", "Heian Alien")),
            target("/music/b.flac", tags("ZUN", "Heian Alien")),
        ],
        true,
    );
    let known: HashMap<(String, String), usize> =
        [(album_key(&tags("ZUN", "Heian Alien")).expect("a key"), 2)]
            .into_iter()
            .collect();

    assert!(partial_records(&groups, &known).is_empty());
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

#[test]
fn a_file_with_no_album_tag_is_never_a_partial_record() {
    let groups = group(vec![target("/music/a.flac", tags("ZUN", ""))], true);

    assert!(partial_records(&groups, &album_counts(&[])).is_empty());
}
