//! Walking the library, describing files, and the SQLite index.

mod support;

use std::path::{Path, PathBuf};

use stk::library::index::{describe, LibraryIndex};
use stk::library::scan::{iter_audio_files, owning_root, Excludes};
use stk::library::tags;
use support::{encode, Encoded, Scratch};

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
        .refresh(
            roots,
            stk::library::scan::AUDIO_EXTENSIONS,
            exclude,
            false,
            None,
        )
        .expect("a refresh")
}

#[test]
fn scan_walks_nested_album_folders() {
    let scratch = Scratch::new("index-walk");
    let root = library_root(&scratch);

    let found = iter_audio_files(
        &root,
        stk::library::scan::AUDIO_EXTENSIONS,
        &Excludes::default(),
    );

    assert!(found.contains(&root.join("ZUN").join("Nested").join("15 Heian Alien.flac")));
    assert_eq!(found.len(), 4);
}

#[test]
fn describe_prefers_tags_over_the_folder_layout() {
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
    let scratch = Scratch::new("describe-untagged");
    let root = library_root(&scratch);

    let entry =
        describe(&root.join("beatMARIO").join("Night of Knights.m4a"), &root).expect("an entry");

    assert_eq!(entry.artist.raw, "beatMARIO");
    assert_eq!(entry.title.raw, "Night of Knights");
}

#[test]
fn a_second_refresh_rereads_nothing() {
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
fn a_changed_file_is_reread() {
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
    let scratch = Scratch::new("index-round-trip");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");
    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    let stored = index.entry(&track).expect("a lookup").expect("an entry");

    assert_eq!(stored, describe(&track, &root).expect("an entry"));
}

#[test]
fn reindex_picks_up_a_tag_this_tool_just_wrote() {
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
fn a_v2_database_is_upgraded_in_place_rather_than_rewritten() {
    let scratch = Scratch::new("index-v2");
    let database = scratch.join("v2.db");

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

    assert_eq!(index.count().expect("a count"), 1);
    let entry = index
        .entry(Path::new("E:\\Music\\a.flac"))
        .expect("a lookup")
        .expect("the migrated row");
    assert_eq!(entry.tags.artist, "ZUN");
    assert_eq!(entry.codec, "");
    assert_eq!(entry.bitrate, None);
    assert_eq!(index.schema_version().expect("a version"), 6);
    assert!(index.snapshots(None).expect("a series").is_empty());
}

/// Adding a column changes neither size nor mtime, so nothing else would re-read the row.
#[test]
fn an_upgraded_database_fills_its_loudness_columns_at_the_next_scan() {
    let scratch = Scratch::new("index-backfill");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");

    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);
    drop(index);
    let connection = rusqlite::Connection::open(scratch.join("index.db")).expect("the database");
    for column in [
        "replay_gain_track_gain",
        "replay_gain_track_peak",
        "replay_gain_album_gain",
        "replay_gain_album_peak",
        "r128_track_gain",
        "r128_album_gain",
    ] {
        connection
            .execute_batch(&format!("ALTER TABLE files DROP COLUMN {column}"))
            .expect("a pre-v6 files table");
    }
    drop(connection);

    stk::library::tags::write_replay_gain(
        &track,
        &stk::models::ReplayGainTags {
            track_gain: Some("-7.29 dB".to_owned()),
            ..stk::models::ReplayGainTags::default()
        },
    )
    .expect("an earlier scan");

    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    let entry = index
        .entry(&track)
        .expect("a lookup")
        .expect("the migrated row");
    assert_eq!(entry.replay_gain.track_gain, Some("-7.29 dB".to_owned()));
    assert!(entry.is_level_matched());
}

/// `upsert` replaces a whole row and would drop the verdict.
#[test]
fn the_backfill_leaves_a_recorded_verdict_where_it_was() {
    let scratch = Scratch::new("index-backfill-verdict");
    let root = library_root(&scratch);
    let track = root.join("DECO_27").join("01 - Roki.flac");

    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);
    index
        .set_verdict(&track, stk::models::Verdict::Ok, 100.0)
        .expect("a recorded verdict");
    drop(index);

    let connection = rusqlite::Connection::open(scratch.join("index.db")).expect("the database");
    connection
        .execute_batch("ALTER TABLE files DROP COLUMN replay_gain_track_gain")
        .expect("a pre-v6 files table");
    drop(connection);

    let mut index = open(&scratch, "index.db");
    refresh(&mut index, &root);

    assert_eq!(
        index.verdict(&track).expect("a lookup"),
        Some(stk::models::Verdict::Ok)
    );
}

/// A refresh deletes what it didn't walk, so a per-root loop would evict the other root.
#[test]
fn a_second_root_does_not_evict_the_first_roots_files() {
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

#[test]
fn an_exclusion_matches_whatever_the_platform_separator_is() {
    let root = Path::new("E:").join("Music");
    let exclude = Excludes::new(&["**/Samples/**".to_owned()]).expect("compiled globs");

    assert!(exclude.matches(&root, &root.join("Samples").join("kick.wav")));
    assert!(!exclude.matches(&root, &root.join("ZUN").join("kick.wav")));
}

#[test]
fn the_deepest_root_owns_a_file() {
    let outer = PathBuf::from(r"E:\Music");
    let inner = outer.join("Live");
    let roots = vec![outer.clone(), inner.clone()];

    let owner = owning_root(&inner.join("ZUN").join("a.flac"), &roots).expect("an owning root");

    assert_eq!(owner, inner);
    assert_eq!(owning_root(Path::new(r"F:\Elsewhere\a.flac"), &roots), None);
}

#[test]
fn a_verdict_goes_stale_when_the_file_moves_under_it() {
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
