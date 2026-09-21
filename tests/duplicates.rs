//! Ported from `py/tests/unit/test_duplicates.py`.
//!
//! The Python monkeypatches `fingerprints.fingerprint` so no decoding happens;
//! here the fingerprint source is a closure the caller supplies, so a table
//! keyed by filename substitutes for it directly.
mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use stk::audio::fingerprint::Fingerprint;
use stk::duplicates::{self, DuplicatePair};
use stk::library::index::LibraryIndex;
use stk::matching::{MatchThresholds, NameForms};
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags};
use support::Scratch;

fn entry(path: &str, artist: &str, title: &str) -> LibraryEntry {
    entry_with(path, artist, title, 1_000_000, Some(200.0))
}

fn entry_with(
    path: &str,
    artist: &str,
    title: &str,
    size: u64,
    duration: Option<f64>,
) -> LibraryEntry {
    LibraryEntry {
        path: PathBuf::from(path),
        size,
        mtime: 1.0,
        folder_artist: artist.to_owned(),
        tags: TrackTags {
            artist: artist.to_owned(),
            title: title.to_owned(),
            ..TrackTags::default()
        },
        artist: NameForms::of(artist),
        title: NameForms::of(title),
        duration_seconds: duration,
        artwork_bytes: 0,
        codec: String::new(),
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
        replay_gain: ReplayGainTags::default(),
    }
}

fn by_title(entries: &[LibraryEntry]) -> Vec<DuplicatePair> {
    duplicates::find_by_title(entries, &MatchThresholds::default())
}

/// A fingerprint whose words are unique to `seed`.
///
/// Deliberately not a shift of another one: the alignment search would rightly
/// call a shifted fingerprint the same audio. Two of these agree on about half
/// their bits, which is what unrelated tracks score.
fn print(seed: u64, length: usize) -> Fingerprint {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    let values = (0..length)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (state >> 32) as u32
        })
        .collect();
    Fingerprint {
        values,
        duration_seconds: None,
    }
}

/// Fingerprints keyed by filename, so no decoding happens in these tests.
fn from_table(
    table: HashMap<String, Fingerprint>,
) -> impl FnMut(&LibraryEntry) -> Option<Fingerprint> {
    move |entry: &LibraryEntry| {
        let name = entry.path.file_name()?.to_string_lossy().into_owned();
        table.get(&name).cloned()
    }
}

fn by_audio(
    entries: &[LibraryEntry],
    threshold: f64,
    tolerance: f64,
    table: HashMap<String, Fingerprint>,
) -> Vec<DuplicatePair> {
    let mut source = from_table(table);
    duplicates::find_by_audio(entries, threshold, tolerance, &mut source)
}

fn table(pairs: &[(&str, Fingerprint)]) -> HashMap<String, Fingerprint> {
    pairs
        .iter()
        .map(|(name, print)| ((*name).to_owned(), print.clone()))
        .collect()
}

#[test]
fn the_same_song_under_two_artist_spellings_is_found() {
    // The pairing the whole romaji layer exists for: one producer, two folders,
    // no shared characters between the spellings.
    let entries = [
        entry("Inabakumori/Lagtrain.flac", "Inabakumori", "Ragutorein"),
        entry(
            "\u{7a32}\u{8449}\u{66c7}/\u{30e9}\u{30b0}\u{30c8}\u{30ec}\u{30a4}\u{30f3}.flac",
            "\u{7a32}\u{8449}\u{66c7}",
            "\u{30e9}\u{30b0}\u{30c8}\u{30ec}\u{30a4}\u{30f3}",
        ),
    ];

    let pairs = by_title(&entries);

    assert_eq!(pairs.len(), 1);
    assert!(pairs[0].score > 0.8);
}

#[test]
fn unrelated_tracks_are_not_paired() {
    let entries = [
        entry("ZUN/Heian Alien.flac", "ZUN", "Heian Alien"),
        entry(
            "beatMARIO/Night of Knights.flac",
            "beatMARIO",
            "Night of Knights",
        ),
    ];

    assert!(by_title(&entries).is_empty());
}

#[test]
fn the_same_title_by_a_clearly_different_artist_is_not_paired() {
    let entries = [
        entry("YOASOBI/Idol.flac", "YOASOBI", "Idol"),
        entry("beatMARIO/Idol.flac", "beatMARIO", "Idol"),
    ];

    assert!(by_title(&entries).is_empty());
}

#[test]
fn a_strong_title_match_pairs_similar_artist_names() {
    // The rule that lets a kanji artist match its romanisation also pairs two
    // merely similar names when the title is identical. Deliberate: the command
    // reports candidates for review and deletes nothing.
    let entries = [
        entry("Ado/Odo.flac", "Ado", "Odo"),
        entry("Syudou/Odo.flac", "Syudou", "Odo"),
    ];

    let pairs = by_title(&entries);

    assert_eq!(pairs.len(), 1);
    assert!(pairs[0].score < 1.0);
}

#[test]
fn a_file_is_never_paired_with_itself() {
    let single = entry("Ado/Odo.flac", "Ado", "Odo");
    assert!(by_title(&[single.clone(), single]).is_empty());
}

#[test]
fn pairs_are_ordered_by_confidence() {
    let entries = [
        entry("A/Senbonzakura.flac", "Hatsune Miku", "Senbonzakura"),
        entry("B/Senbonzakura.flac", "Hatsune Miku", "Senbonzakura"),
        entry(
            "C/Senbonzakura Long.flac",
            "Hatsune Miku",
            "Senbonzakura -Long Ver-",
        ),
    ];

    let scores: Vec<f64> = by_title(&entries).iter().map(|pair| pair.score).collect();
    let mut sorted = scores.clone();
    sorted.sort_by(|left, right| right.partial_cmp(left).expect("comparable scores"));

    assert_eq!(scores, sorted);
}

#[test]
fn the_larger_copy_is_identified_as_the_one_to_keep() {
    let entries = [
        entry_with("A/Odo.mp3", "Ado", "Odo", 5_000_000, Some(200.0)),
        entry_with("B/Odo.flac", "Ado", "Odo", 40_000_000, Some(200.0)),
    ];

    let pairs = by_title(&entries);
    let pair = &pairs[0];

    assert_eq!(
        pair.larger().path.extension().expect("an extension"),
        "flac"
    );
    assert_eq!(
        pair.smaller().path.extension().expect("an extension"),
        "mp3"
    );
}

#[test]
fn blocking_does_not_lose_a_match_hidden_in_a_larger_library() {
    let mut state = 20_260_830_u64;
    let mut distinct = || {
        (0..12)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                char::from(b'a' + u8::try_from((state >> 33) % 26).unwrap_or(0))
            })
            .collect::<String>()
    };

    let mut entries: Vec<LibraryEntry> = (0..120)
        .map(|_| {
            let (artist, title) = (distinct(), distinct());
            entry(&format!("{artist}/{title}.flac"), &artist, &title)
        })
        .collect();
    let duplicated = entries[42].clone();
    entries.push(entry(
        "Elsewhere/copy.flac",
        &duplicated.artist.raw,
        &duplicated.title.raw,
    ));

    let pairs = by_title(&entries);

    assert_eq!(pairs.len(), 1);
    let found: std::collections::HashSet<&Path> =
        [pairs[0].left.path.as_path(), pairs[0].right.path.as_path()]
            .into_iter()
            .collect();
    assert_eq!(
        found,
        [duplicated.path.as_path(), Path::new("Elsewhere/copy.flac")]
            .into_iter()
            .collect()
    );
}

#[test]
fn identical_audio_is_paired_whatever_the_names_say() {
    let shape = print(0, 40);
    let entries = [
        entry("X/a.flac", "Unknown", "track01"),
        entry("Y/b.flac", "Some Other Artist", "18"),
    ];

    let pairs = by_audio(
        &entries,
        duplicates::DEFAULT_AUDIO_THRESHOLD,
        duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
        table(&[("a.flac", shape.clone()), ("b.flac", shape)]),
    );

    assert_eq!(pairs.len(), 1);
    assert!((pairs[0].score - 1.0).abs() < 1e-9);
}

#[test]
fn different_audio_of_the_same_length_is_not_paired() {
    // Exactly the coincidence that made a duration-and-size fingerprint
    // unreliable: same length, same size, different recordings.
    let entries = [
        entry_with("X/a.flac", "One", "Track", 1_000_000, Some(200.0)),
        entry_with("Y/b.flac", "Two", "Other", 1_000_000, Some(200.0)),
    ];

    assert!(by_audio(
        &entries,
        duplicates::DEFAULT_AUDIO_THRESHOLD,
        duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
        table(&[("a.flac", print(0, 40)), ("b.flac", print(3, 40))]),
    )
    .is_empty());
}

#[test]
fn tracks_of_very_different_length_are_never_fingerprinted() {
    let entries = [
        entry_with("X/a.flac", "One", "Track", 1_000_000, Some(100.0)),
        entry_with("Y/b.flac", "Two", "Other", 1_000_000, Some(400.0)),
    ];

    assert!(by_audio(
        &entries,
        duplicates::DEFAULT_AUDIO_THRESHOLD,
        duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
        HashMap::new(),
    )
    .is_empty());
}

#[test]
fn the_duration_tolerance_is_configurable() {
    let shape = print(0, 40);
    let entries = [
        entry_with("X/a.flac", "One", "Track", 1_000_000, Some(200.0)),
        entry_with("Y/b.flac", "Two", "Other", 1_000_000, Some(205.0)),
    ];
    let prints = table(&[("a.flac", shape.clone()), ("b.flac", shape)]);

    assert!(by_audio(
        &entries,
        duplicates::DEFAULT_AUDIO_THRESHOLD,
        duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
        prints.clone(),
    )
    .is_empty());
    assert!(!by_audio(&entries, duplicates::DEFAULT_AUDIO_THRESHOLD, 10.0, prints).is_empty());
}

#[test]
fn files_without_a_known_duration_are_skipped() {
    let entries = [
        entry_with("X/a.flac", "One", "Track", 1_000_000, None),
        entry_with("Y/b.flac", "Two", "Other", 1_000_000, None),
    ];

    assert!(by_audio(
        &entries,
        duplicates::DEFAULT_AUDIO_THRESHOLD,
        duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
        HashMap::new(),
    )
    .is_empty());
}

#[test]
fn the_threshold_is_configurable() {
    let entries = [
        entry("X/a.flac", "One", "Track"),
        entry("Y/b.flac", "Two", "Other"),
    ];

    assert!(!by_audio(
        &entries,
        0.1,
        duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
        table(&[("a.flac", print(0, 40)), ("b.flac", print(3, 40))]),
    )
    .is_empty());
}

#[test]
fn an_undecodable_file_is_skipped_rather_than_aborting_the_run() {
    let entries = [
        entry("X/a.flac", "One", "Track"),
        entry("Y/broken.flac", "Two", "Other"),
        entry("Z/c.flac", "Three", "Third"),
    ];

    // The source reports nothing for the broken file, which is what a decode
    // failure looks like to `find_by_audio`.
    let shape = print(0, 40);
    let mut source = |entry: &LibraryEntry| {
        let name = entry.path.file_name()?.to_string_lossy().into_owned();
        (name != "broken.flac").then(|| shape.clone())
    };

    let pairs = duplicates::find_by_audio(
        &entries,
        duplicates::DEFAULT_AUDIO_THRESHOLD,
        duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
        &mut source,
    );

    let names: std::collections::HashSet<String> = pairs
        .iter()
        .map(|pair| {
            pair.left
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["a.flac".to_owned()].into_iter().collect());
}

#[test]
fn fingerprints_are_cached_in_the_index() {
    let scratch = Scratch::new("dupes-cache");
    let entries = [
        entry("X/a.flac", "One", "Track"),
        entry("Y/b.flac", "Two", "Other"),
    ];
    let shape = print(0, 40);

    let mut index = LibraryIndex::open(&scratch.join("index.db")).expect("an index");
    let mut calls = 0;

    for _ in 0..2 {
        let mut source = |entry: &LibraryEntry| {
            if let Ok(Some(cached)) = index.fingerprint(&entry.path, entry.mtime) {
                return Some(cached);
            }
            calls += 1;
            let value = shape.clone();
            let _ = index.store_fingerprint(&entry.path, entry.mtime, &value);
            Some(value)
        };
        duplicates::find_by_audio(
            &entries,
            duplicates::DEFAULT_AUDIO_THRESHOLD,
            duplicates::DEFAULT_DURATION_TOLERANCE_SECONDS,
            &mut source,
        );
    }

    // Two files fingerprinted on the first pass, none on the second.
    assert_eq!(calls, 2);
}
