//! Measuring the library over time, behind `stk library trend`.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use stk::library::index::LibraryIndex;
use stk::library::stats::{date_of, measure, LibrarySnapshot};
use stk::matching::NameForms;
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags, Verdict};

fn entry(path: &str, tags: TrackTags, artwork_bytes: u64) -> LibraryEntry {
    LibraryEntry {
        path: PathBuf::from(path),
        size: 1,
        mtime: 1.0,
        folder_artist: String::new(),
        artist: NameForms::of(&tags.artist),
        title: NameForms::of(&tags.title),
        duration_seconds: None,
        artwork_bytes,
        tags,
        codec: String::new(),
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
        replay_gain: ReplayGainTags::default(),
    }
}

fn titled(path: &str) -> LibraryEntry {
    entry(
        path,
        TrackTags {
            title: "Something".to_owned(),
            ..TrackTags::default()
        },
        0,
    )
}

fn untitled(path: &str) -> LibraryEntry {
    entry(path, TrackTags::default(), 0)
}

fn snapshot(entries: &[LibraryEntry]) -> LibrarySnapshot {
    measure(entries, &BTreeMap::new(), 0.0)
}

#[test]
fn a_file_counts_as_tagged_only_when_it_has_a_title() {
    let measured = snapshot(&[titled("/music/a.flac"), untitled("/music/b.flac")]);

    assert_eq!(measured.total, 2);
    assert_eq!(measured.tagged, 1);
}

#[test]
fn artwork_and_genre_are_counted_separately() {
    let with_both = entry(
        "/music/a.flac",
        TrackTags {
            title: "Something".to_owned(),
            genres: vec!["Touhou".to_owned()],
            ..TrackTags::default()
        },
        4096,
    );
    let measured = snapshot(&[with_both, untitled("/music/b.flac")]);

    assert_eq!(measured.with_artwork, 1);
    assert_eq!(measured.with_genre, 1);
}

/// The figure exists to notice a fetch that left un-scanned files behind,
/// whoever did the scanning.
#[test]
fn a_file_counts_as_level_matched_only_with_a_track_gain() {
    let mut scanned = titled("/music/a.flac");
    scanned.replay_gain.track_gain = Some("-7.29 dB".to_owned());
    // A peak on its own says nothing a player can act on.
    let mut peak_only = titled("/music/b.flac");
    peak_only.replay_gain.track_peak = Some("0.988556".to_owned());

    let measured = snapshot(&[scanned, peak_only, titled("/music/c.flac")]);

    assert_eq!(measured.with_replay_gain, Some(1));
}

/// An Opus file scanned in its native mode carries only the header field, and
/// is level-matched wherever it is played.
#[test]
fn an_opus_file_carrying_only_r128_counts_as_level_matched() {
    let mut scanned = titled("/music/a.opus");
    scanned.replay_gain.r128_track_gain = Some("-2022".to_owned());

    assert_eq!(snapshot(&[scanned]).with_replay_gain, Some(1));
}

/// A measurement taken before the column existed did not count this, which is
/// not the same as having counted none.
#[test]
fn a_snapshot_taken_before_v6_reports_no_loudness_figure() {
    let before = LibrarySnapshot {
        taken_at: 1.0,
        total: 10,
        with_replay_gain: None,
        ..LibrarySnapshot::default()
    };
    let after = LibrarySnapshot {
        with_replay_gain: Some(0),
        ..before
    };

    assert_ne!(before.counts(), after.counts());
}

#[test]
fn lossless_is_counted_by_the_same_rule_verify_uses() {
    // `stk verify` picks what to analyse by container, so the two figures have
    // to describe one set of files or the trend contradicts the report.
    let measured = snapshot(&[
        untitled("/music/a.flac"),
        untitled("/music/b.WAV"),
        untitled("/music/c.mp3"),
        untitled("/music/d.opus"),
    ]);

    assert_eq!(measured.lossless, 2);
}

#[test]
fn everything_verify_doubted_is_counted_as_flagged() {
    // FAKE, SUSPECT and UPSAMPLED go in one column because anyone chasing
    // them down treats them the same way.
    let verdicts: BTreeMap<Verdict, usize> = [
        (Verdict::Ok, 10),
        (Verdict::Fake, 2),
        (Verdict::Suspect, 3),
        (Verdict::Upsampled, 4),
        (Verdict::Error, 1),
    ]
    .into_iter()
    .collect();

    let measured = measure(&[], &verdicts, 0.0);

    assert_eq!(measured.verified_clean, 10);
    assert_eq!(measured.flagged, 9);
}

fn measured(total: usize) -> LibrarySnapshot {
    LibrarySnapshot {
        taken_at: 1.0,
        total,
        ..LibrarySnapshot::default()
    }
}

#[test]
fn a_measurement_that_repeats_the_last_one_is_not_recorded_again() {
    // Scanning four times on a quiet afternoon should leave one point on the
    // trend, not four: a run of identical rows is not evidence of anything.
    let scratch = support::Scratch::new("stats-repeat");
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");

    assert!(index
        .record_snapshot(&measured(10))
        .expect("a first")
        .is_some());
    assert!(index
        .record_snapshot(&LibrarySnapshot {
            taken_at: 99_999.0,
            ..measured(10)
        })
        .expect("a repeat")
        .is_none());

    assert_eq!(index.snapshots(None).expect("the series").len(), 1);
}

#[test]
fn a_measurement_that_moved_is_recorded() {
    let scratch = support::Scratch::new("stats-moved");
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");

    index.record_snapshot(&measured(10)).expect("a first");
    index.record_snapshot(&measured(11)).expect("a second");

    let series = index.snapshots(None).expect("the series");
    assert_eq!(series.len(), 2);
    // Newest first, as the change log reads.
    assert_eq!(series[0].total, 11);
}

#[test]
fn the_series_can_be_limited_to_the_most_recent_measurements() {
    let scratch = support::Scratch::new("stats-limit");
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");
    for total in 1..=5 {
        index.record_snapshot(&measured(total)).expect("a row");
    }

    let series = index.snapshots(Some(2)).expect("the series");
    assert_eq!(
        series.iter().map(|row| row.total).collect::<Vec<_>>(),
        vec![5, 4]
    );
}

#[test]
fn a_measurement_is_dated_by_the_day_it_was_taken() {
    assert_eq!(date_of(0.0), "1970-01-01");
    assert_eq!(date_of(86_399.0), "1970-01-01");
    assert_eq!(date_of(86_400.0), "1970-01-02");
    assert_eq!(date_of(1_788_220_800.0), "2026-09-01");
}

#[test]
fn a_leap_day_is_dated_as_one() {
    // The 400-year rule, which a naive divide-by-365 gets wrong.
    assert_eq!(date_of(951_782_400.0), "2000-02-29");
    assert_eq!(date_of(1_709_164_800.0), "2024-02-29");
}

#[test]
fn a_year_end_does_not_slip_into_the_next_one() {
    assert_eq!(date_of(1_767_225_599.0), "2025-12-31");
    assert_eq!(date_of(1_767_225_600.0), "2026-01-01");
}

#[test]
fn a_measurement_is_taken_from_what_the_index_holds() {
    let scratch = support::Scratch::new("stats-measure-index");
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");

    assert!(index.record_measurement(5.0).expect("a first").is_some());
    assert!(index.record_measurement(6.0).expect("a repeat").is_none());

    let series = index.snapshots(None).expect("the series");
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].total, 0);
    assert!((series[0].taken_at - 5.0).abs() < f64::EPSILON);
}
