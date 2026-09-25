//! The `--apply` write log and its undo: every write is recorded, and the
//! record is enough to put the file back.

mod support;

use std::path::{Path, PathBuf};

use stk::library::changes::{
    apply_undo, decode_extras, decode_genres, decode_replay_gain, digest, plan_undo, Blocked,
    ChangeFilter, ChangeLog,
};
use stk::library::index::{default_extensions, LibraryIndex};
use stk::library::scan::Excludes;
use stk::library::tags::{self, TagField};
use stk::models::{Artwork, ReplayGainTags};
use support::{encode, Encoded, Scratch};

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
    assert_eq!(index.schema_version().expect("a version"), 6);
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
fn a_move_is_logged_against_the_files_new_home() {
    // Keyed on the destination, because `plan_undo` opens by asking whether the
    // file it is about to put back is still where the log left it.
    let scratch = Scratch::new("changes-move-row");
    let root = library(&scratch);
    let from = root.join("ZUN").join("Heian Alien.flac");
    let to = root.join("ZUN").join("Faith").join("01 - Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "organise", 100.0).expect("a run");
    log.moved(&mut index, &from, &to).expect("a logged move");
    stk::organise::move_file(&from, &to).expect("a move");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes[0].field, "move");
    assert_eq!(changes[0].path, to);
    assert_eq!(
        changes[0].old_value.as_deref(),
        Some(from.display().to_string().as_str())
    );
}

#[test]
fn undoing_a_move_puts_the_file_back_where_it_came_from() {
    let scratch = Scratch::new("changes-move-undo");
    let root = library(&scratch);
    let from = root.join("ZUN").join("Heian Alien.flac");
    let to = root.join("ZUN").join("Faith").join("01 - Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "organise", 100.0).expect("a run");
    log.moved(&mut index, &from, &to).expect("a logged move");
    stk::organise::move_file(&from, &to).expect("a move");
    index
        .relocate(&from, &to, std::slice::from_ref(&root))
        .expect("a relocated row");

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");
    assert!(plan.ready(), "{:?}", plan.blocked);

    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    assert!(from.is_file());
    assert!(!to.is_file());
    assert!(index.entry(&from).expect("a lookup").is_some());
    assert!(index.entry(&to).expect("a lookup").is_none());
}

#[test]
fn undoing_a_move_is_blocked_when_something_took_the_old_place() {
    // Putting the file back would overwrite a different file, which is not
    // something --force should be able to buy.
    let scratch = Scratch::new("changes-move-taken");
    let root = library(&scratch);
    let from = root.join("ZUN").join("Heian Alien.flac");
    let to = root.join("ZUN").join("Faith").join("01 - Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "organise", 100.0).expect("a run");
    log.moved(&mut index, &from, &to).expect("a logged move");
    stk::organise::move_file(&from, &to).expect("a move");

    encode(
        &from,
        &Encoded {
            metadata: &[("title", "Something Else")],
            ..Encoded::default()
        },
    );

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");

    assert!(matches!(plan.blocked, Some(Blocked::SourceOccupied(_))));
    assert!(!plan.allowed(true), "no flag should destroy another file");
}

#[test]
fn a_move_that_never_happened_cannot_be_undone() {
    // The row is written before the rename, so a rename that then failed leaves
    // a row for a file that is not there. Detectable and inert.
    let scratch = Scratch::new("changes-move-phantom");
    let root = library(&scratch);
    let from = root.join("ZUN").join("Heian Alien.flac");
    let to = root.join("ZUN").join("Faith").join("01 - Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "organise", 100.0).expect("a run");
    log.moved(&mut index, &from, &to).expect("a logged move");

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");

    assert_eq!(plan.blocked, Some(Blocked::FileGone));
    assert!(from.is_file(), "the source is untouched");
}

fn measurement(track_gain: &str) -> ReplayGainTags {
    ReplayGainTags {
        track_gain: Some(track_gain.to_owned()),
        track_peak: Some("0.988556".to_owned()),
        album_gain: Some("-6.25 dB".to_owned()),
        album_peak: Some("0.995000".to_owned()),
        r128_track_gain: None,
        r128_album_gain: None,
    }
}

/// The four figures are one measurement, so they share one row: a row apiece
/// would let an undo restore a gain without the peak that bounds it.
#[test]
fn a_loudness_write_is_logged_as_one_row_carrying_every_figure() {
    let scratch = Scratch::new("changes-loudness");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let written = measurement("-7.29 dB");
    let log = ChangeLog::open(&index, "loudness", 100.0).expect("a run");
    tags::write_replay_gain(&track, &written).expect("a loudness write");
    log.replay_gain(&mut index, &track, &ReplayGainTags::default(), &written)
        .expect("a logged change");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].command, "loudness");
    assert_eq!(changes[0].field, "replay_gain");
    assert_eq!(
        decode_replay_gain(changes[0].old_value.as_deref()),
        ReplayGainTags::default()
    );
    assert_eq!(decode_replay_gain(changes[0].new_value.as_deref()), written);
}

/// A library scanned elsewhere carries figures somebody chose, and `--force`
/// replaces them. The undo has to give them back exactly as they were spelled.
#[test]
fn undoing_a_loudness_write_puts_the_previous_scan_back() {
    let scratch = Scratch::new("changes-loudness-undo");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let before = measurement("-11.11 dB");
    tags::write_replay_gain(&track, &before).expect("an earlier scan");

    let after = measurement("-7.29 dB");
    let log = ChangeLog::open(&index, "loudness", 100.0).expect("a run");
    tags::write_replay_gain(&track, &after).expect("a loudness write");
    log.replay_gain(&mut index, &track, &before, &after)
        .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    assert!(plan.ready(), "{:?}", plan.blocked);

    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    assert_eq!(
        tags::read_replay_gain(&track).expect("readable loudness tags"),
        before
    );
}

#[test]
fn undoing_a_first_scan_removes_the_loudness_tags() {
    let scratch = Scratch::new("changes-loudness-first");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let written = measurement("-7.29 dB");
    let log = ChangeLog::open(&index, "loudness", 100.0).expect("a run");
    tags::write_replay_gain(&track, &written).expect("a loudness write");
    log.replay_gain(&mut index, &track, &ReplayGainTags::default(), &written)
        .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    assert!(tags::read_replay_gain(&track)
        .expect("readable loudness tags")
        .is_empty());
}

/// A gain someone has since corrected by hand is not overwritten, the same
/// rule every other field is undone under.
#[test]
fn a_scan_changed_since_it_was_logged_blocks_its_undo() {
    let scratch = Scratch::new("changes-loudness-blocked");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let written = measurement("-7.29 dB");
    let log = ChangeLog::open(&index, "loudness", 100.0).expect("a run");
    tags::write_replay_gain(&track, &written).expect("a loudness write");
    log.replay_gain(&mut index, &track, &ReplayGainTags::default(), &written)
        .expect("a logged change");

    tags::write_replay_gain(&track, &measurement("-3.00 dB")).expect("a later edit");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");

    assert_eq!(plan.blocked, Some(Blocked::FileChanged));
    assert!(plan.allowed(true));
}

/// A file as a peer leaves it: a comment beside the fields that matter.
fn commented(scratch: &Scratch) -> PathBuf {
    let track = scratch.join("Music").join("ZUN").join("Commented.flac");
    encode(
        &track,
        &Encoded {
            metadata: &[
                ("artist", "ZUN"),
                ("title", "Heian Alien"),
                ("comment", "ripped by someone"),
            ],
            ..Encoded::default()
        },
    );
    track
}

fn strip_extras(index: &mut LibraryIndex, log: &ChangeLog, track: &Path) {
    let before = tags::read_extras(track).expect("readable extras");
    tags::write_extras(track, &[]).expect("stripped extras");
    log.extras(index, track, &before, &[])
        .expect("a logged change");
}

#[test]
fn dropped_extras_are_logged_as_one_row_holding_every_item() {
    let scratch = Scratch::new("changes-extras-row");
    let root = library(&scratch);
    let track = commented(&scratch);
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "fetch download", 100.0).expect("a run");
    strip_extras(&mut index, &log, &track);

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field, "extras");
    assert_eq!(
        decode_extras(changes[0].old_value.as_deref()),
        vec![tags::Extra {
            key: "COMMENT".to_owned(),
            value: "ripped by someone".to_owned(),
            locator: false,
        }]
    );
    assert_eq!(changes[0].new_value.as_deref(), Some("[]"));
}

#[test]
fn undoing_dropped_extras_puts_the_comment_back() {
    let scratch = Scratch::new("changes-extras-undo");
    let root = library(&scratch);
    let track = commented(&scratch);
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "fetch download", 100.0).expect("a run");
    strip_extras(&mut index, &log, &track);
    assert!(tags::read_extras(&track)
        .expect("readable extras")
        .is_empty());

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");
    assert!(plan.ready(), "{:?}", plan.blocked);
    assert_eq!(plan.description, "put back the 1 dropped tag(s)");

    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    let extras = tags::read_extras(&track).expect("readable extras");
    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].value, "ripped by someone");
    assert_eq!(
        tags::read_tags(&track).expect("readable tags").title,
        "Heian Alien"
    );
    let undone = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    assert_eq!(undone.command, "changes undo");
    assert_eq!(undone.old_value.as_deref(), Some("[]"));
}

#[test]
fn an_extra_added_since_the_drop_blocks_its_undo() {
    let scratch = Scratch::new("changes-extras-blocked");
    let root = library(&scratch);
    let track = commented(&scratch);
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "fetch download", 100.0).expect("a run");
    strip_extras(&mut index, &log, &track);
    tags::write_extras(
        &track,
        &[tags::Extra {
            key: "COMMENT".to_owned(),
            value: "edited by hand".to_owned(),
            locator: false,
        }],
    )
    .expect("a later edit");

    let change = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
        .remove(0);
    let plan = plan_undo(&index, change).expect("a plan");

    assert_eq!(plan.blocked, Some(Blocked::FileChanged));
}
