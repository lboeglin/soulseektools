//! Duplicate detection by title and by audio, grouping, and resolving.
mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use stk::audio::fingerprint::Fingerprint;
use stk::config::OrganiseSettings;
use stk::duplicates::{self, DuplicateGroup, DuplicatePair, Keep, Loser, Resolution};
use stk::library::changes::{apply_undo, plan_undo, ChangeFilter, ChangeLog, UNDO_COMMAND};
use stk::library::index::LibraryIndex;
use stk::matching::{MatchThresholds, NameForms};
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags};
use stk::organise::PlannedMove;
use support::{encode, Encoded, Scratch};

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
        isrc: None,
    }
}

fn by_title(entries: &[LibraryEntry]) -> Vec<DuplicatePair> {
    duplicates::find_by_title(entries, &MatchThresholds::default())
}

/// Not a shift of another print, which the alignment search would rightly call a match.
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
    // One producer, two folders, no characters in common.
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
fn the_same_title_by_a_clearly_different_artist_is_not_paired() {
    let entries = [
        entry("YOASOBI/Idol.flac", "YOASOBI", "Idol"),
        entry("beatMARIO/Idol.flac", "beatMARIO", "Idol"),
    ];

    assert!(by_title(&entries).is_empty());
}

#[test]
fn a_file_is_never_paired_with_itself() {
    let single = entry("Ado/Odo.flac", "Ado", "Odo");
    assert!(by_title(&[single.clone(), single]).is_empty());
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
        std::collections::HashSet::from([
            duplicated.path.as_path(),
            Path::new("Elsewhere/copy.flac")
        ])
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
    // Same length and size, different recordings.
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
fn an_undecodable_file_is_skipped_rather_than_aborting_the_run() {
    let entries = [
        entry("X/a.flac", "One", "Track"),
        entry("Y/broken.flac", "Two", "Other"),
        entry("Z/c.flac", "Three", "Third"),
    ];

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
    assert_eq!(
        names,
        std::collections::HashSet::from(["a.flac".to_owned()])
    );
}

#[test]
fn a_print_the_index_had_to_compute_is_stored_for_the_next_run() {
    let scratch = Scratch::new("index-stores-prints");
    let track = support::encode(
        &scratch.join("Music").join("a.flac"),
        &support::Encoded {
            seconds: 4.0,
            modulation_hz: 0.7,
            ..support::Encoded::default()
        },
    );
    let described =
        stk::library::index::describe(&track, &scratch.join("Music")).expect("an entry");

    let mut index = LibraryIndex::open(&scratch.join("index.db")).expect("an index");
    assert!(index
        .fingerprint(&track, described.mtime)
        .expect("a lookup")
        .is_none());

    let prints = index.fingerprints(std::slice::from_ref(&described));

    let computed = prints[0].as_ref().expect("a computed print");
    assert!(!computed.is_empty());
    assert_eq!(
        index
            .fingerprint(&track, described.mtime)
            .expect("a lookup")
            .as_ref(),
        Some(computed)
    );
}

fn pair(left: &LibraryEntry, right: &LibraryEntry, score: f64) -> DuplicatePair {
    DuplicatePair {
        left: left.clone(),
        right: right.clone(),
        score,
        evidence: format!("score {score:.2}"),
    }
}

fn sized(path: &str, size: u64) -> LibraryEntry {
    entry_with(path, "DECO*27", "Roki", size, Some(200.0))
}

fn paths(entries: &[LibraryEntry]) -> Vec<&Path> {
    entries.iter().map(|entry| entry.path.as_path()).collect()
}

fn one_group(entries: &[LibraryEntry]) -> DuplicateGroup {
    let pairs = entries
        .windows(2)
        .map(|window| pair(&window[0], &window[1], 1.0));
    duplicates::group(pairs).remove(0)
}

#[test]
fn three_copies_reported_as_three_pairs_are_one_group() {
    let [a, b, c] = [sized("a.flac", 1), sized("b.flac", 2), sized("c.flac", 3)];

    let groups = duplicates::group([pair(&a, &b, 1.0), pair(&b, &c, 1.0), pair(&a, &c, 1.0)]);

    assert_eq!(groups.len(), 1);
    assert_eq!(
        paths(&groups[0].entries),
        [
            Path::new("a.flac"),
            Path::new("b.flac"),
            Path::new("c.flac")
        ]
    );
    assert_eq!(groups[0].pairs.len(), 3);
}

#[test]
fn a_pair_both_finders_report_is_counted_once_on_the_stronger_score() {
    let [a, b] = [sized("a.flac", 1), sized("b.flac", 2)];

    let groups = duplicates::group([pair(&a, &b, 0.9), pair(&b, &a, 0.97)]);

    assert_eq!(groups[0].pairs.len(), 1);
    assert!((groups[0].pairs[0].score - 0.97).abs() < f64::EPSILON);
}

#[test]
fn lossless_keeps_the_flac_even_when_its_mp3_is_larger() {
    let group = one_group(&[sized("Roki.flac", 1_000), sized("Roki.mp3", 5_000)]);

    let resolution = duplicates::choose(&group, Keep::Lossless);

    assert_eq!(resolution.keeper.path, Path::new("Roki.flac"));
    assert_eq!(resolution.losers.len(), 1);
    assert_eq!(resolution.losers[0].entry.path, Path::new("Roki.mp3"));
    assert!(
        resolution.losers[0].reason.contains("lossy"),
        "{:?}",
        resolution.losers
    );
}

#[test]
fn tagged_keeps_the_copy_with_more_filled_in() {
    let bare = sized("bare.flac", 9_000);
    let mut full = sized("full.flac", 1_000);
    full.tags.album = "Ghost".to_owned();
    full.artwork_bytes = 10;

    let resolution = duplicates::choose(&one_group(&[bare, full]), Keep::Tagged);

    assert_eq!(resolution.keeper.path, Path::new("full.flac"));
    assert_eq!(
        resolution.losers[0].reason,
        "2 of 4 tag fields filled, against 4"
    );
}

#[test]
fn oldest_keeps_the_copy_that_was_there_first() {
    let mut old = sized("old.mp3", 1_000);
    old.mtime = 10.0;
    let mut new = sized("new.flac", 9_000);
    new.mtime = 20.0;

    let resolution = duplicates::choose(&one_group(&[old, new]), Keep::Oldest);

    assert_eq!(resolution.keeper.path, Path::new("old.mp3"));
    assert_eq!(resolution.losers[0].reason, "newer");
}

#[test]
fn identical_copies_resolve_by_path_whatever_order_they_came_in() {
    let [a, b] = [sized("a.flac", 1_000), sized("b.flac", 1_000)];

    for pairs in [[pair(&a, &b, 1.0)], [pair(&b, &a, 1.0)]] {
        let resolution = duplicates::choose(&duplicates::group(pairs).remove(0), Keep::Lossless);
        assert_eq!(resolution.keeper.path, Path::new("a.flac"));
        assert!(resolution.losers[0].reason.contains("first by path"));
    }
}

#[test]
fn quarantined_files_leave_the_index_and_an_undo_puts_them_back() {
    let scratch = Scratch::new("dupes-quarantine");
    let root = scratch.join("Music");
    let keeper = encode(&root.join("DECO_27").join("Roki.flac"), &Encoded::default());
    let loser = encode(&root.join("Loose").join("Roki.mp3"), &Encoded::default());
    let beside = encode(&root.join("Shared").join("Roki.mp3"), &Encoded::default());
    std::fs::write(root.join("Shared").join("cover.jpg"), b"jpeg").expect("a cover");
    let roots = [root.clone()];

    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");
    let mut entries = Vec::new();
    for path in [&keeper, &loser, &beside] {
        entries.push(
            index
                .reindex(path, &roots)
                .expect("indexed")
                .expect("a file"),
        );
    }
    let resolved = Resolution {
        keeper: entries[0].clone(),
        losers: entries[1..]
            .iter()
            .map(|entry| Loser {
                entry: entry.clone(),
                reason: "lossy, and the kept copy is lossless".to_owned(),
            })
            .collect(),
    };

    let log = ChangeLog::open(&index, duplicates::RESOLVE_COMMAND, 1.0).expect("a log");
    let directory = duplicates::quarantine_directory(&index, log.run_id());
    assert_eq!(directory, scratch.join("quarantine").join("1"));
    let planned = duplicates::plan_quarantine(&[resolved], &roots, &directory);
    let ready: Vec<&PlannedMove> = planned.iter().collect();
    let applied = duplicates::quarantine(
        &mut index,
        &log,
        &ready,
        &roots,
        &OrganiseSettings::default(),
    )
    .expect("a quarantine");

    assert_eq!(applied.moved, 2);
    assert!(applied.failures.is_empty());
    assert_eq!(applied.pruned, 1, "only the folder left genuinely empty");
    assert!(keeper.is_file());
    assert!(!root.join("Loose").exists());
    assert!(root.join("Shared").join("cover.jpg").is_file());
    let parked = directory.join("Loose").join("Roki.mp3");
    assert!(parked.is_file());
    assert!(directory.join("Shared").join("Roki.mp3").is_file());
    assert_eq!(index.count().expect("a count"), 1);

    let changes = index.changes(&ChangeFilter::default()).expect("the log");
    assert_eq!(changes.len(), 2);
    assert!(changes
        .iter()
        .all(|change| change.command == "dupes resolve" && change.field == "move"));

    let undo = ChangeLog::open(&index, UNDO_COMMAND, 2.0).expect("an undo log");
    for change in changes {
        let plan = plan_undo(&index, change).expect("a plan");
        assert!(plan.ready(), "{:?}", plan.blocked);
        apply_undo(&mut index, &plan, &undo, &roots).expect("an undo");
    }

    assert!(loser.is_file());
    assert!(beside.is_file());
    assert!(!parked.exists());
    assert_eq!(index.count().expect("a count"), 3);
    assert!(index.entry(&loser).expect("a lookup").is_some());
}
