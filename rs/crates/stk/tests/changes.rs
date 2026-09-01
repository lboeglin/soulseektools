//! The `--apply` write log and its undo, from #2.
//!
//! The log exists because a command that rewrites two hundred files with a bad
//! genre map or the wrong artist's cover previously left no record of what it
//! had done. These check the two properties that makes it worth having: every
//! applied write is recorded, and the record is enough to put the file back.

mod support;

use std::path::{Path, PathBuf};

use stk::library::changes::{
    apply_undo, decode_genres, digest, plan_undo, Blocked, ChangeFilter, ChangeLog,
};
use stk::library::index::{default_extensions, LibraryIndex};
use stk::library::scan::Excludes;
use stk::library::tags::{self, TagField};
use stk::models::Artwork;
use support::{encode, Encoded, Scratch};

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

fn library(scratch: &Scratch) -> PathBuf {
    let root = scratch.join("Music");
    encode(
        &root.join("ZUN").join("Heian Alien.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("title", "Heian Alien")],
            ..Encoded::default()
        },
    );
    encode(
        &root.join("ZUN").join("Bad Apple.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("title", "Bad Apple")],
            ..Encoded::default()
        },
    );
    root
}

fn indexed(scratch: &Scratch, root: &Path) -> LibraryIndex {
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");
    index
        .refresh(
            std::slice::from_ref(&root.to_path_buf()),
            default_extensions(),
            &Excludes::default(),
            false,
            None,
        )
        .expect("a refresh");
    index
}

/// A tiny PNG-shaped payload: the log stores bytes, not images.
fn cover(byte: u8) -> Artwork {
    Artwork {
        data: vec![byte; 2048],
        mime: "image/jpeg".to_owned(),
    }
}

#[test]
fn a_genre_write_is_logged_with_both_values() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-genre");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "genre apply", 100.0).expect("a run");
    tags::write_genres(&track, &["Touhou".to_owned()]).expect("a genre write");
    log.genre(&mut index, &track, &[], &["Touhou".to_owned()])
        .expect("a logged change");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].command, "genre apply");
    assert_eq!(changes[0].field, "genre");
    assert_eq!(
        decode_genres(changes[0].old_value.as_deref()),
        Vec::<String>::new()
    );
    assert_eq!(
        decode_genres(changes[0].new_value.as_deref()),
        vec!["Touhou".to_owned()]
    );
}

#[test]
fn undoing_a_genre_write_puts_the_old_tag_back() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-genre-undo");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);
    tags::write_genres(&track, &["Doujin".to_owned()]).expect("a starting genre");

    let log = ChangeLog::open(&index, "genre apply", 100.0).expect("a run");
    tags::write_genres(&track, &["Touhou".to_owned()]).expect("a genre write");
    log.genre(
        &mut index,
        &track,
        &["Doujin".to_owned()],
        &["Touhou".to_owned()],
    )
    .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    assert!(plan.ready(), "{:?}", plan.blocked);

    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    assert_eq!(
        tags::read_tags(&track).expect("readable tags").genres,
        vec!["Doujin".to_owned()]
    );
}

/// The log has to stay a complete account of what this tool wrote, so an undo
/// is itself a change.
#[test]
fn an_undo_is_logged_like_any_other_write() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-undo-logged");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "genre apply", 100.0).expect("a run");
    tags::write_genres(&track, &["Touhou".to_owned()]).expect("a genre write");
    log.genre(&mut index, &track, &[], &["Touhou".to_owned()])
        .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, &[root]).expect("an undo");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].command, "changes undo");
    assert_ne!(changes[0].run_id, changes[1].run_id);
}

#[test]
fn a_file_edited_since_it_was_logged_blocks_its_undo() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-blocked");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "genre apply", 100.0).expect("a run");
    tags::write_genres(&track, &["Touhou".to_owned()]).expect("a genre write");
    log.genre(&mut index, &track, &[], &["Touhou".to_owned()])
        .expect("a logged change");

    tags::write_genres(&track, &["Fixed By Hand".to_owned()]).expect("a later edit");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");

    assert_eq!(plan.blocked, Some(Blocked::FileChanged));
    assert!(!plan.allowed(false));
    // A hand edit is the user's call to overrule, so --force may go ahead.
    assert!(plan.allowed(true));
}

/// Content addressing is what keeps a library-wide `art fetch` affordable: an
/// album's tracks share one replaced cover, and so one sidecar.
#[test]
fn one_cover_replaced_across_two_files_is_stored_once() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-sidecar");
    let root = library(&scratch);
    let mut index = indexed(&scratch, &root);
    let old = cover(1);
    let new = cover(2);

    let log = ChangeLog::open(&index, "art fetch", 100.0).expect("a run");
    for name in ["Heian Alien.flac", "Bad Apple.flac"] {
        let track = root.join("ZUN").join(name);
        tags::write_artwork(&track, &old).expect("a starting cover");
        tags::write_artwork(&track, &new).expect("a replacement cover");
        log.artwork(&mut index, &track, Some(&old), Some(&new))
            .expect("a logged change");
    }

    let sidecars: Vec<_> = std::fs::read_dir(scratch.join("changes"))
        .expect("the sidecar directory")
        .flatten()
        .collect();
    assert_eq!(sidecars.len(), 1);
    assert_eq!(
        sidecars[0].file_name().to_string_lossy(),
        format!("{}.bin", digest(&old.data))
    );
}

#[test]
fn undoing_an_artwork_write_restores_the_previous_cover() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-art-undo");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);
    let old = cover(1);
    let new = cover(2);
    tags::write_artwork(&track, &old).expect("a starting cover");

    let log = ChangeLog::open(&index, "art fetch", 100.0).expect("a run");
    tags::write_artwork(&track, &new).expect("a replacement cover");
    log.artwork(&mut index, &track, Some(&old), Some(&new))
        .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, &[root]).expect("an undo");

    let restored = tags::read_artwork(&track)
        .expect("readable artwork")
        .expect("a cover");
    assert_eq!(restored.data, old.data);
}

/// Undoing a first embed means removing the cover, not restoring one.
#[test]
fn undoing_a_first_embed_leaves_the_file_with_no_cover() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-art-clear");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);
    let new = cover(2);

    let log = ChangeLog::open(&index, "art fetch", 100.0).expect("a run");
    tags::write_artwork(&track, &new).expect("an embedded cover");
    log.artwork(&mut index, &track, None, Some(&new))
        .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, &[root]).expect("an undo");

    assert_eq!(tags::read_artwork(&track).expect("readable artwork"), None);
}

#[test]
fn a_change_whose_file_is_gone_cannot_be_undone_by_any_flag() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-file-gone");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "genre apply", 100.0).expect("a run");
    tags::write_genres(&track, &["Touhou".to_owned()]).expect("a genre write");
    log.genre(&mut index, &track, &[], &["Touhou".to_owned()])
        .expect("a logged change");
    std::fs::remove_file(&track).expect("a removed file");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");

    assert_eq!(plan.blocked, Some(Blocked::FileGone));
    assert!(!plan.allowed(true));
}

#[test]
fn every_write_in_one_command_shares_a_run() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-run");
    let root = library(&scratch);
    let mut index = indexed(&scratch, &root);

    let first = ChangeLog::open(&index, "genre apply", 100.0).expect("a run");
    for name in ["Heian Alien.flac", "Bad Apple.flac"] {
        let track = root.join("ZUN").join(name);
        first
            .genre(&mut index, &track, &[], &["Touhou".to_owned()])
            .expect("a logged change");
    }
    let second = ChangeLog::open(&index, "genre apply", 200.0).expect("a second run");

    assert_eq!(second.run_id(), first.run_id() + 1);
    assert_eq!(index.last_run_id().expect("a run"), Some(first.run_id()));
    assert_eq!(
        index
            .changes(&ChangeFilter {
                run: Some(first.run_id()),
                ..ChangeFilter::default()
            })
            .expect("the run's rows")
            .len(),
        2
    );
}

/// A v3 database already carries a `changes` table without the run column.
#[test]
fn a_v3_change_table_gains_its_run_column_in_place() {
    let scratch = Scratch::new("changes-migrate");
    let database = scratch.join("library.db");
    let connection = rusqlite::Connection::open(&database).expect("a database");
    connection
        .execute_batch(
            "CREATE TABLE changes (
                 id INTEGER PRIMARY KEY, applied_at REAL NOT NULL, command TEXT NOT NULL,
                 path TEXT NOT NULL, field TEXT NOT NULL, old_value BLOB, new_value BLOB);
             INSERT INTO changes (applied_at, command, path, field)
             VALUES (1.0, 'genre apply', 'E:\\Music\\a.flac', 'genre');",
        )
        .expect("a v3 changes table");
    drop(connection);

    let index = LibraryIndex::open(&database).expect("an upgraded index");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].run_id, 0);
    assert_eq!(index.schema_version().expect("a version"), 5);
}

/// Write one field and log it the way `tags apply` does.
fn write_field(
    index: &mut LibraryIndex,
    log: &ChangeLog,
    track: &Path,
    field: TagField,
    value: &str,
) {
    let before = tags::read_tags(track).expect("readable tags");
    let mut after = before.clone();
    field.set(&mut after, value);
    tags::write_tags(track, &after).expect("a tag write");
    log.tag(index, track, field, &before, &after)
        .expect("a logged change");
}

#[test]
fn a_tag_write_is_logged_once_per_field() {
    // One row per field, not one per file: an undo must never write wider than
    // the row it is undoing.
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-tag-rows");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &track, TagField::Title, "Native Faith");
    write_field(
        &mut index,
        &log,
        &track,
        TagField::Album,
        "Mountain of Faith",
    );

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    let fields: Vec<&str> = changes.iter().map(|row| row.field.as_str()).collect();
    assert_eq!(fields, vec!["album", "title"]);
    assert!(changes.iter().all(|row| row.command == "tags apply"));
    assert!(changes.iter().all(|row| row.run_id == changes[0].run_id));
}

#[test]
fn a_tag_write_records_the_value_it_replaced() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-tag-values");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &track, TagField::Title, "Native Faith");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes[0].old_value.as_deref(), Some("Heian Alien"));
    assert_eq!(changes[0].new_value.as_deref(), Some("Native Faith"));
}

#[test]
fn undoing_a_title_write_puts_the_old_title_back() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-tag-undo");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &track, TagField::Title, "Native Faith");

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");
    assert!(plan.ready(), "{:?}", plan.blocked);
    assert!(
        plan.description.contains("Heian Alien"),
        "{}",
        plan.description
    );

    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    assert_eq!(
        tags::read_tags(&track).expect("readable tags").title,
        "Heian Alien"
    );
}

#[test]
fn undoing_a_first_ever_album_tag_clears_it() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-tag-undo-clear");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(
        &mut index,
        &log,
        &track,
        TagField::Album,
        "Mountain of Faith",
    );

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    assert_eq!(tags::read_tags(&track).expect("readable tags").album, "");
}

#[test]
fn undoing_one_field_leaves_another_edited_since_alone() {
    // The reason the log keeps a row per field. A single row covering the whole
    // tag block would read the album edit as a reason to refuse -- and, forced,
    // would revert it.
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-tag-undo-narrow");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &track, TagField::Title, "Native Faith");

    // Someone fixes the album by hand afterwards.
    let mut edited = tags::read_tags(&track).expect("readable tags");
    edited.album = "Mountain of Faith".to_owned();
    tags::write_tags(&track, &edited).expect("a hand edit");

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");
    assert!(plan.ready(), "the title is untouched: {:?}", plan.blocked);

    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    let read = tags::read_tags(&track).expect("readable tags");
    assert_eq!(read.title, "Heian Alien");
    assert_eq!(read.album, "Mountain of Faith");
}

#[test]
fn a_title_edited_since_it_was_logged_blocks_its_own_undo() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-tag-undo-blocked");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &track, TagField::Title, "Native Faith");

    let mut edited = tags::read_tags(&track).expect("readable tags");
    edited.title = "Someone Fixed This".to_owned();
    tags::write_tags(&track, &edited).expect("a hand edit");

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");

    assert_eq!(plan.blocked, Some(Blocked::FileChanged));
    assert!(
        plan.allowed(true),
        "a hand edit is the user's call to overrule"
    );
}

#[test]
fn an_undone_tag_write_is_logged_in_its_turn() {
    needs_ffmpeg!();
    let scratch = Scratch::new("changes-tag-undo-logged");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &track, TagField::Title, "Native Faith");

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes[0].command, "changes undo");
    assert_eq!(changes[0].new_value.as_deref(), Some("Heian Alien"));
}
