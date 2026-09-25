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

/// An Opus file scanned in its native mode carries only the header field, and
/// is level-matched wherever it is played.
#[test]
fn an_opus_file_carrying_only_r128_counts_as_level_matched() {
    let mut scanned = titled("/music/a.opus");
    scanned.replay_gain.r128_track_gain = Some("-2022".to_owned());

    assert_eq!(snapshot(&[scanned]).with_replay_gain, Some(1));
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
