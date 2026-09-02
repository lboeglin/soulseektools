//! The arithmetic and the album grouping behind `stk loudness` (#7).
//!
//! All of it is pure, so what a gain figure means is established here rather
//! than through ffmpeg. The measurement itself is in `loudness.rs`.

use std::path::PathBuf;

use stk::audio::loudness::Loudness;
use stk::config::LoudnessSettings;
use stk::models::{ReplayGainTags, TrackTags};
use stk::replaygain::{
    album_key, album_loudness, format_gain, format_peak, format_r128, gain_db, peak_amplitude,
    plan, select, Measured, Target,
};

fn settings() -> LoudnessSettings {
    LoudnessSettings::default()
}

fn loudness(integrated_lufs: f64, peak_dbfs: f64) -> Loudness {
    Loudness {
        integrated_lufs,
        peak_dbfs,
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
        duration_seconds: Some(180.0),
        current: ReplayGainTags::default(),
        r128: false,
    }
}

fn measured(path: &str, tags: TrackTags, integrated_lufs: f64) -> Measured {
    Measured {
        target: target(path, tags),
        loudness: Some(loudness(integrated_lufs, -1.0)),
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
fn a_peak_in_dbfs_becomes_a_linear_amplitude() {
    assert!((peak_amplitude(0.0) - 1.0).abs() < 1e-9);
    assert!((peak_amplitude(-6.0206) - 0.5).abs() < 1e-4);
    // A clipped master peaks above full scale, which is the case a player
    // applying positive gain needs the figure for.
    assert!(peak_amplitude(1.0) > 1.0);
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
        &[
            measured("/music/a.flac", tags("ZUN", "Heian Alien"), -9.0),
            measured("/music/b.flac", tags("ZUN", "Heian Alien"), -21.0),
        ],
        &settings(),
        true,
    );

    assert_eq!(changes.len(), 2);
    assert_ne!(
        changes[0].proposed.track_gain,
        changes[1].proposed.track_gain
    );
    assert_eq!(
        changes[0].proposed.album_gain,
        changes[1].proposed.album_gain
    );
}

#[test]
fn an_album_sits_between_its_loudest_and_its_quietest_track() {
    let album = album_loudness(&[
        (loudness(-9.0, -1.0), 180.0),
        (loudness(-21.0, -1.0), 180.0),
    ])
    .expect("an album loudness");

    assert!(album < -9.0 && album > -21.0);
}

/// Energy, not decibels: the loud track dominates the average the way it
/// dominates the record.
#[test]
fn an_album_follows_its_loud_tracks_rather_than_the_mean_of_the_numbers() {
    let album = album_loudness(&[
        (loudness(-9.0, -1.0), 180.0),
        (loudness(-21.0, -1.0), 180.0),
    ])
    .expect("an album loudness");

    assert!(album > -15.0, "{album}");
}

#[test]
fn a_long_track_weighs_more_in_its_album_than_a_short_one() {
    let quiet = loudness(-24.0, -1.0);
    let loud = loudness(-12.0, -1.0);

    let long_quiet = album_loudness(&[(quiet, 600.0), (loud, 60.0)]).expect("an album loudness");
    let long_loud = album_loudness(&[(quiet, 60.0), (loud, 600.0)]).expect("an album loudness");

    assert!(long_loud > long_quiet);
}

#[test]
fn an_album_peak_is_the_loudest_of_its_tracks() {
    let quiet = Measured {
        target: target("/music/a.flac", tags("ZUN", "Heian Alien")),
        loudness: Some(loudness(-12.0, -20.0)),
    };
    let loud = Measured {
        target: target("/music/b.flac", tags("ZUN", "Heian Alien")),
        loudness: Some(loudness(-12.0, -3.0)),
    };

    let changes = plan(&[quiet, loud], &settings(), true);

    assert_eq!(
        changes[0].proposed.album_peak,
        Some(format_peak(peak_amplitude(-3.0)))
    );
    assert_eq!(
        changes[0].proposed.album_peak,
        changes[1].proposed.album_peak
    );
}

#[test]
fn a_file_with_no_album_tag_gets_a_track_gain_and_nothing_else() {
    let changes = plan(
        &[measured("/music/single.flac", tags("ZUN", ""), -9.0)],
        &settings(),
        true,
    );

    assert!(changes[0].proposed.track_gain.is_some());
    assert_eq!(changes[0].proposed.album_gain, None);
    assert_eq!(changes[0].proposed.album_peak, None);
}

#[test]
fn two_albums_of_the_same_name_by_different_artists_stay_apart() {
    let changes = plan(
        &[
            measured("/music/a.flac", tags("ZUN", "Greatest Hits"), -9.0),
            measured("/music/b.flac", tags("DECO*27", "Greatest Hits"), -21.0),
        ],
        &settings(),
        true,
    );

    assert_ne!(
        changes[0].proposed.album_gain,
        changes[1].proposed.album_gain
    );
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

#[test]
fn track_only_leaves_every_album_field_empty() {
    let changes = plan(
        &[
            measured("/music/a.flac", tags("ZUN", "Heian Alien"), -9.0),
            measured("/music/b.flac", tags("ZUN", "Heian Alien"), -21.0),
        ],
        &settings(),
        false,
    );

    assert!(changes
        .iter()
        .all(|change| change.proposed.album_gain.is_none()));
}

#[test]
fn a_file_that_could_not_be_measured_is_planned_for_nothing() {
    let changes = plan(
        &[Measured {
            target: target("/music/broken.flac", tags("ZUN", "Heian Alien")),
            loudness: None,
        }],
        &settings(),
        true,
    );

    assert!(changes.is_empty());
}

/// The point of the default: a library somebody scanned in foobar2000 is not
/// re-measured out from under them.
#[test]
fn a_file_that_already_carries_a_gain_is_left_alone() {
    let mut scanned = target("/music/a.flac", tags("ZUN", ""));
    scanned.current.track_gain = Some("-7.29 dB".to_owned());

    let selection = select(vec![scanned], true, false);

    assert_eq!(selection.wanted.len(), 0);
    assert_eq!(selection.skipped.len(), 1);
}

#[test]
fn force_measures_a_file_that_already_carries_a_gain() {
    let mut scanned = target("/music/a.flac", tags("ZUN", ""));
    scanned.current.track_gain = Some("-7.29 dB".to_owned());

    let selection = select(vec![scanned], true, true);

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

    let selection = select(vec![done, pending], true, false);

    assert_eq!(selection.wanted.len(), 2);
    assert!(selection.skipped.is_empty());
}

#[test]
fn a_fully_scanned_album_is_left_alone() {
    let mut first = target("/music/a.flac", tags("ZUN", "Heian Alien"));
    first.current.track_gain = Some("-7.29 dB".to_owned());
    let mut second = target("/music/b.flac", tags("ZUN", "Heian Alien"));
    second.current.track_gain = Some("-4.10 dB".to_owned());

    let selection = select(vec![first, second], true, false);

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

    assert!(select(vec![scanned], true, false).wanted.is_empty());
}

#[test]
fn an_opus_file_is_planned_both_gains_and_everything_else_only_the_standard_ones() {
    let mut opus = measured("/music/a.opus", tags("ZUN", "Heian Alien"), -9.0);
    opus.target.r128 = true;
    let flac = measured("/music/b.flac", tags("ZUN", "Heian Alien"), -9.0);

    let changes = plan(&[opus, flac], &settings(), true);

    assert!(changes[0].proposed.r128_track_gain.is_some());
    assert!(changes[0].proposed.r128_album_gain.is_some());
    assert_eq!(changes[1].proposed.r128_track_gain, None);
    assert_eq!(changes[1].proposed.r128_album_gain, None);
}

#[test]
fn an_album_of_nothing_has_no_loudness() {
    assert_eq!(album_loudness(&[]), None);
}

/// Neither `stk` nor the index need know how long a track is for a scan to
/// work, so lengths that are all unknown weigh one apiece.
#[test]
fn an_album_whose_tracks_have_no_known_length_is_still_measured() {
    let album = album_loudness(&[(loudness(-9.0, -1.0), 0.0), (loudness(-21.0, -1.0), 0.0)]);

    assert!(album.is_some());
}
