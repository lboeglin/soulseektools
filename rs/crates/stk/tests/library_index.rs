//! Ported from `py/tests/unit/test_library_index.py`.

mod support;

use std::path::{Path, PathBuf};

use stk::library::index::{default_extensions, describe, LibraryIndex};
use stk::library::scan::{folder_artist, iter_audio_files, owning_root, Excludes};
use stk::library::tags;
use support::{encode, Encoded, Scratch};

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

/// A small library laid out the way the real one is: `<root>/<artist>/...`.
fn library_root(scratch: &Scratch) -> PathBuf {
    let root = scratch.join("Music");
    encode(
        &root.join("DECO_27").join("01 - Roki.flac"),
        &Encoded {
            metadata: &[
                ("artist", "DECO*27"),
                ("title", "Roki"),
                ("album", "Ghost"),
                ("genre", "Vocaloid"),
            ],
            ..Encoded::default()
        },
    );
    encode(
        &root.join("DECO_27").join("02 - Yowamushi Montblanc.mp3"),
        &Encoded {
            metadata: &[("artist", "DECO*27"), ("title", "Yowamushi Montblanc")],
            ..Encoded::default()
        },
    );
    encode(
        &root.join("ZUN").join("Nested").join("15 Heian Alien.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN")],
            ..Encoded::default()
        },
    );
    encode(
        &root.join("beatMARIO").join("Night of Knights.m4a"),
        &Encoded::default(),
    );
    root
}

fn open(scratch: &Scratch, name: &str) -> LibraryIndex {
    LibraryIndex::open(&scratch.join(name)).expect("an index")
}

fn refresh(index: &mut LibraryIndex, root: &Path) -> stk::library::index::RefreshResult {
    refresh_roots(index, &[root.to_path_buf()], &Excludes::default())
}

fn refresh_roots(
    index: &mut LibraryIndex,
    roots: &[PathBuf],
    exclude: &Excludes,
) -> stk::library::index::RefreshResult {
    index
        .refresh(roots, default_extensions(), exclude, false, None)
        .expect("a refresh")
}

#[test]
fn scan_walks_nested_album_folders() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-walk");
    let root = library_root(&scratch);

    let found = iter_audio_files(&root, default_extensions(), &Excludes::default());

    assert!(found.contains(&root.join("ZUN").join("Nested").join("15 Heian Alien.flac")));
    assert_eq!(found.len(), 4);
}

#[test]
fn scan_honours_the_extension_filter() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-filter");
    let root = library_root(&scratch);

    let found = iter_audio_files(&root, &[".flac"], &Excludes::default());

    assert!(found.iter().all(|path| path.extension().unwrap() == "flac"));
    assert_eq!(found.len(), 2);
}

#[test]
fn scan_ignores_non_audio_files() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-non-audio");
    let root = library_root(&scratch);
    std::fs::write(root.join("DECO_27").join("cover.jpg"), b"not audio").expect("a written file");

    let found = iter_audio_files(&root, default_extensions(), &Excludes::default());

    assert!(found.iter().all(|path| path.extension().unwrap() != "jpg"));
}

#[test]
fn folder_artist_is_the_first_component_below_the_root() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-folder-artist");
    let root = library_root(&scratch);
    let nested = root.join("ZUN").join("Nested").join("15 Heian Alien.flac");

    assert_eq!(folder_artist(&nested, &root), "ZUN");
}

#[test]
fn a_file_in_the_root_has_no_implied_artist() {
    let scratch = Scratch::new("index-loose");
    let root = scratch.join("Music");
    assert_eq!(folder_artist(&root.join("loose.flac"), &root), "");
}

#[test]
fn a_path_outside_the_root_has_no_implied_artist() {
    let scratch = Scratch::new("index-outside");
    let root = scratch.join("Music");
    assert_eq!(folder_artist(Path::new("/elsewhere/track.flac"), &root), "");
}

#[test]
fn describe_prefers_tags_over_the_folder_layout() {
    needs_ffmpeg!();
    let scratch = Scratch::new("describe-tags");
    let root = library_root(&scratch);

    let entry = describe(&root.join("DECO_27").join("01 - Roki.flac"), &root).expect("an entry");

    assert_eq!(entry.tags.artist, "DECO*27");
    assert_eq!(entry.artist.raw, "DECO*27");
    assert_eq!(entry.title.raw, "Roki");
    assert_eq!(entry.folder_artist, "DECO_27");
}

#[test]
fn describe_falls_back_to_the_folder_and_filename_when_untagged() {
    needs_ffmpeg!();
    let scratch = Scratch::new("describe-untagged");
    let root = library_root(&scratch);

    let entry =
        describe(&root.join("beatMARIO").join("Night of Knights.m4a"), &root).expect("an entry");

    assert_eq!(entry.artist.raw, "beatMARIO");
    assert_eq!(entry.title.raw, "Night of Knights");
}

#[test]
fn describe_strips_the_track_number_from_a_filename_derived_title() {
    needs_ffmpeg!();
    let scratch = Scratch::new("describe-track-number");
    let root = library_root(&scratch);

    let entry = describe(
        &root.join("ZUN").join("Nested").join("15 Heian Alien.flac"),
        &root,
    )
    .expect("an entry");

    assert_eq!(entry.title.raw, "Heian Alien");
}

#[test]
fn describe_records_size_duration_and_artwork() {
    needs_ffmpeg!();
    let scratch = Scratch::new("describe-properties");
    let root = library_root(&scratch);

    let entry = describe(&root.join("DECO_27").join("01 - Roki.flac"), &root).expect("an entry");

    assert!(entry.size > 0);
    assert!((entry.duration_seconds.expect("a duration") - 1.0).abs() < 0.15);
    assert_eq!(entry.artwork_bytes, 0);
    assert!(!entry.has_artwork());
}

#[test]
fn describe_precomputes_the_name_forms_used_for_matching() {
    needs_ffmpeg!();
    let scratch = Scratch::new("describe-forms");
    let root = library_root(&scratch);

    let entry = describe(&root.join("DECO_27").join("01 - Roki.flac"), &root).expect("an entry");

    assert_eq!(entry.artist.normalized, "deco 27");
    assert_eq!(entry.title.romaji, "roki");
}

/// New in schema v3 (#2): the same header read now also records the stream.
#[test]
fn describe_records_the_stream_properties_the_upgrade_flow_needs() {
    needs_ffmpeg!();
    let scratch = Scratch::new("describe-stream");
    let root = library_root(&scratch);

    let entry = describe(&root.join("DECO_27").join("01 - Roki.flac"), &root).expect("an entry");

    assert_eq!(entry.codec, "flac");
    assert_eq!(entry.sample_rate, Some(44100));
    assert!(entry.bitrate.is_some());
}

#[test]
fn refresh_indexes_every_file() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-refresh");
    let root = library_root(&scratch);
    let mut index = open(&scratch, "index.db");

    let result = refresh(&mut index, &root);

    assert_eq!(result.added, 4);
    assert_eq!(result.unchanged, 0);
    assert_eq!(index.count().expect("a count"), 4);
}

#[test]
fn a_second_refresh_rereads_nothing() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-incremental");
    let root = library_root(&scratch);
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    let result = refresh(&mut index, &root);

    assert_eq!(result.unchanged, 4);
    assert_eq!(result.added, 0);
    assert_eq!(result.updated, 0);
}

#[test]
fn full_refresh_rereads_everything() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-full");
    let root = library_root(&scratch);
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    let result = index
        .refresh(
            std::slice::from_ref(&root),
            default_extensions(),
            &Excludes::default(),
            true,
            None,
        )
        .expect("a refresh");

    assert_eq!(result.updated, 4);
    assert_eq!(result.unchanged, 0);
}

#[test]
fn a_changed_file_is_reread() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-changed");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    encode(
        &track,
        &Encoded {
            seconds: 2.0,
            metadata: &[("artist", "DECO*27"), ("title", "Roki")],
            ..Encoded::default()
        },
    );

    let result = refresh(&mut index, &root);

    assert_eq!(result.updated, 1);
    assert_eq!(result.unchanged, 3);
    let entry = index.entry(&track).expect("a lookup").expect("an entry");
    assert!((entry.duration_seconds.expect("a duration") - 2.0).abs() < 0.15);
}

#[test]
fn a_deleted_file_is_dropped() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-deleted");
    let root = library_root(&scratch);
    let track = root.join("beatMARIO").join("Night of Knights.m4a");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    std::fs::remove_file(&track).expect("a removed file");
    let result = refresh(&mut index, &root);

    assert_eq!(result.removed, 1);
    assert_eq!(index.count().expect("a count"), 3);
    assert!(index.entry(&track).expect("a lookup").is_none());
}

#[test]
fn entries_round_trip_through_sqlite() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-round-trip");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    let stored = index.entry(&track).expect("a lookup").expect("an entry");

    assert_eq!(stored, describe(&track, &root).expect("an entry"));
}

#[test]
fn entries_are_returned_in_a_deterministic_order() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-order");
    let root = library_root(&scratch);
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    let paths: Vec<String> = index
        .entries()
        .expect("entries")
        .iter()
        .map(|entry| entry.path.display().to_string())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();

    assert_eq!(paths, sorted);
}

#[test]
fn the_index_survives_being_reopened() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-reopen");
    let root = library_root(&scratch);
    {
        let mut index = open(&scratch, "index.db");
        refresh(&mut index, &root);
    }

    let mut reopened = open(&scratch, "index.db");
    assert_eq!(reopened.count().expect("a count"), 4);
    assert_eq!(refresh(&mut reopened, &root).unchanged, 4);
}

#[test]
fn artist_counts_group_by_folder() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-artists");
    let root = library_root(&scratch);
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    assert_eq!(
        index.artist_counts().expect("counts"),
        vec![
            ("DECO_27".to_owned(), 2),
            ("ZUN".to_owned(), 1),
            ("beatMARIO".to_owned(), 1),
        ]
    );
}

#[test]
fn refresh_reports_progress_per_file() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-progress");
    let root = library_root(&scratch);
    let mut index = open(&scratch, "index.db");

    let mut seen: Vec<PathBuf> = Vec::new();
    let mut record = |path: &Path| seen.push(path.to_path_buf());
    index
        .refresh(
            std::slice::from_ref(&root),
            default_extensions(),
            &Excludes::default(),
            false,
            Some(&mut record),
        )
        .expect("a refresh");

    assert_eq!(seen.len(), 4);
}

#[test]
fn creates_the_database_directory() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-nested-db");
    let root = library_root(&scratch);
    let mut index = LibraryIndex::open(&scratch.join("nested").join("deeper").join("index.db"))
        .expect("an index");

    assert_eq!(refresh(&mut index, &root).added, 4);
}

#[test]
fn reindex_picks_up_a_tag_this_tool_just_wrote() {
    needs_ffmpeg!();
    // Without this, a command that edits tags leaves the next command reporting
    // what was true before the edit.
    let scratch = Scratch::new("index-reindex");
    let root = library_root(&scratch);
    let track = root.join("ZUN").join("Nested").join("15 Heian Alien.flac");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    assert!(index
        .entry(&track)
        .expect("a lookup")
        .expect("an entry")
        .tags
        .genres
        .is_empty());

    tags::write_genres(&track, &["Touhou".to_owned()]).expect("written genres");
    index
        .reindex(&track, std::slice::from_ref(&root))
        .expect("a reindex");

    assert_eq!(
        index
            .entry(&track)
            .expect("a lookup")
            .expect("an entry")
            .tags
            .genres,
        vec!["Touhou".to_owned()]
    );
}

#[test]
fn reindexing_a_file_that_is_gone_does_nothing() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-reindex-absent");
    let root = library_root(&scratch);
    let mut index = open(&scratch, "index.db");

    assert!(index
        .reindex(&root.join("absent.flac"), std::slice::from_ref(&root))
        .expect("a reindex")
        .is_none());
}

/// Both builds have to open the same file while `py/` still exists (#1).
#[test]
fn a_v2_database_is_upgraded_in_place_rather_than_rewritten() {
    let scratch = Scratch::new("index-v2");
    let database = scratch.join("v2.db");

    // Exactly what the Python build creates: v2, without the six v3 columns.
    let connection = rusqlite::Connection::open(&database).expect("a database");
    connection
        .execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE files (
                path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime REAL NOT NULL,
                duration_seconds REAL, artwork_bytes INTEGER NOT NULL DEFAULT 0,
                folder_artist TEXT NOT NULL DEFAULT '',
                tag_artist TEXT NOT NULL DEFAULT '',
                tag_album_artist TEXT NOT NULL DEFAULT '',
                tag_album TEXT NOT NULL DEFAULT '', tag_title TEXT NOT NULL DEFAULT '',
                tag_genres TEXT NOT NULL DEFAULT '[]', tag_track_number INTEGER,
                artist_raw TEXT NOT NULL DEFAULT '', artist_normalized TEXT NOT NULL DEFAULT '',
                artist_romaji TEXT NOT NULL DEFAULT '', title_raw TEXT NOT NULL DEFAULT '',
                title_normalized TEXT NOT NULL DEFAULT '', title_romaji TEXT NOT NULL DEFAULT ''
             );
             INSERT INTO meta VALUES ('schema_version', '2');
             INSERT INTO files (path, size, mtime, tag_artist)
                VALUES ('E:\\Music\\a.flac', 10, 1.0, 'ZUN');",
        )
        .expect("a v2 database");
    drop(connection);

    let index = LibraryIndex::open(&database).expect("an upgraded index");

    // The existing row survives, and the new columns read back as absent.
    assert_eq!(index.count().expect("a count"), 1);
    let entry = index
        .entry(Path::new("E:\\Music\\a.flac"))
        .expect("a lookup")
        .expect("the migrated row");
    assert_eq!(entry.tags.artist, "ZUN");
    assert_eq!(entry.codec, "");
    assert_eq!(entry.bitrate, None);
    assert_eq!(index.schema_version().expect("a version"), 4);
}

/// `verify` stops being a one-shot report once its verdict is persisted (#2).
#[test]
fn a_verdict_is_recorded_against_the_file() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-verdict");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    assert_eq!(index.verdict(&track).expect("a lookup"), None);

    index
        .set_verdict(&track, stk::models::Verdict::Fake, 1.0)
        .expect("a recorded verdict");

    assert_eq!(
        index.verdict(&track).expect("a lookup"),
        Some(stk::models::Verdict::Fake)
    );
}

/// Multiple roots, from #2: a library spread across two drives.
#[test]
fn files_under_two_roots_land_in_one_index() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-two-roots");
    let first = library_root(&scratch);
    let second = scratch.join("More");
    encode(
        &second.join("ZUN").join("Bad Apple.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("title", "Bad Apple")],
            ..Encoded::default()
        },
    );
    let mut index = open(&scratch, "index.db");

    let roots = vec![first.clone(), second.clone()];
    let result = refresh_roots(&mut index, &roots, &Excludes::default());

    assert_eq!(result.added, 5);
    assert_eq!(index.count().expect("a count"), 5);
    let entry = index
        .entry(&second.join("ZUN").join("Bad Apple.flac"))
        .expect("a lookup")
        .expect("the second root's file");
    assert_eq!(entry.folder_artist, "ZUN");
}

/// A refresh deletes what it did not walk, so a per-root loop would make each
/// root evict the other's rows.
#[test]
fn a_second_root_does_not_evict_the_first_roots_files() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-no-evict");
    let first = library_root(&scratch);
    let second = scratch.join("More");
    encode(
        &second.join("ZUN").join("Bad Apple.flac"),
        &Encoded::default(),
    );
    let mut index = open(&scratch, "index.db");

    let roots = vec![first.clone(), second];
    refresh_roots(&mut index, &roots, &Excludes::default());
    let result = refresh_roots(&mut index, &roots, &Excludes::default());

    assert_eq!(result.removed, 0);
    assert_eq!(result.unchanged, 5);
}

/// Exclusions, from #2: audiobooks and sample packs stay out of the index.
#[test]
fn an_excluded_folder_stays_out_of_the_walk() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-exclude");
    let root = library_root(&scratch);
    encode(
        &root.join("Audiobooks").join("Chapter 1.mp3"),
        &Encoded::default(),
    );
    let exclude = Excludes::new(&["**/Audiobooks/**".to_owned()]).expect("compiled globs");

    let found = iter_audio_files(&root, default_extensions(), &exclude);

    assert_eq!(found.len(), 4);
    assert!(found
        .iter()
        .all(|path| !path.starts_with(root.join("Audiobooks"))));
}

#[test]
fn an_exclusion_matches_whatever_the_platform_separator_is() {
    let root = Path::new("E:").join("Music");
    let exclude = Excludes::new(&["**/Samples/**".to_owned()]).expect("compiled globs");

    assert!(exclude.matches(&root, &root.join("Samples").join("kick.wav")));
    assert!(!exclude.matches(&root, &root.join("ZUN").join("kick.wav")));
}

#[test]
fn an_unparseable_exclusion_is_reported_rather_than_ignored() {
    assert!(Excludes::new(&["[".to_owned()]).is_err());
}

/// Roots may nest, and the deeper one is what names the artist.
#[test]
fn the_deepest_root_owns_a_file() {
    let outer = PathBuf::from(r"E:\Music");
    let inner = outer.join("Live");
    let roots = vec![outer.clone(), inner.clone()];

    let owner = owning_root(&inner.join("ZUN").join("a.flac"), &roots).expect("an owning root");

    assert_eq!(owner, inner);
    assert_eq!(owning_root(Path::new(r"F:\Elsewhere\a.flac"), &roots), None);
}

/// A re-described file drops its verdict: `upsert` replaces the whole row, so
/// the judgement cannot outlive the audio it was made about.
#[test]
fn reindexing_a_file_clears_its_verdict() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-verdict-cleared");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);
    index
        .set_verdict(&track, stk::models::Verdict::Ok, 1.0)
        .expect("a recorded verdict");

    index
        .reindex(&track, std::slice::from_ref(&root))
        .expect("a reindex");

    assert_eq!(index.verdict(&track).expect("a lookup"), None);
}

#[test]
fn a_verdict_goes_stale_when_the_file_moves_under_it() {
    needs_ffmpeg!();
    let scratch = Scratch::new("index-verdict-stale");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);
    index
        .set_verdict(&track, stk::models::Verdict::Ok, 1.0)
        .expect("a recorded verdict");

    assert_eq!(
        index.current_verdict(&track).expect("a lookup"),
        Some(stk::models::Verdict::Ok)
    );

    encode(&track, &Encoded::default());

    assert_eq!(index.current_verdict(&track).expect("a lookup"), None);
    assert_eq!(
        index.verdict(&track).expect("a lookup"),
        Some(stk::models::Verdict::Ok)
    );
}
