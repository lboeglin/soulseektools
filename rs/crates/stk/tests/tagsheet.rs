//! Reading an edited `tags dump` back, behind `stk tags apply` (#3).

mod support;

use std::path::{Path, PathBuf};

use stk::library::index::describe;
use stk::library::tags::{self, TagField};
use stk::matching::NameForms;
use stk::models::{LibraryEntry, TrackTags};
use stk::tagsheet::{plan_changes, read_sheet, Blanks, PlannedEdit, Skipped, TagChange};
use support::{encode, Encoded, Scratch};

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

fn entry(path: &Path, tags: TrackTags) -> LibraryEntry {
    LibraryEntry {
        path: path.to_path_buf(),
        size: 1,
        mtime: 1.0,
        folder_artist: String::new(),
        artist: NameForms::of(&tags.artist),
        title: NameForms::of(&tags.title),
        duration_seconds: None,
        artwork_bytes: 0,
        tags,
        codec: String::new(),
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
    }
}

fn tagged() -> TrackTags {
    TrackTags {
        artist: "DECO*27".to_owned(),
        album_artist: "DECO*27".to_owned(),
        album: "Ghost".to_owned(),
        title: "Roki".to_owned(),
        genres: vec!["Vocaloid".to_owned()],
        track_number: Some(3),
    }
}

fn sheet(scratch: &Scratch, body: &str) -> PathBuf {
    let path = scratch.join("sheet.csv");
    std::fs::write(&path, body).expect("a written sheet");
    path
}

/// A sheet naming one file, whose path has to survive CSV quoting.
fn rows(path: &Path, header: &str, cells: &str) -> String {
    format!("path,{header}\n\"{}\",{cells}\n", path.display())
}

fn changed(planned: &[PlannedEdit]) -> Vec<&TagChange> {
    planned
        .iter()
        .filter_map(|edit| match edit {
            PlannedEdit::Change(change) => Some(change.as_ref()),
            PlannedEdit::Skip { .. } => None,
        })
        .collect()
}

fn skips(planned: &[PlannedEdit]) -> Vec<Skipped> {
    planned
        .iter()
        .filter_map(|edit| match edit {
            PlannedEdit::Skip { reason, .. } => Some(*reason),
            PlannedEdit::Change(_) => None,
        })
        .collect()
}

#[test]
fn a_dump_applied_unedited_proposes_nothing() {
    // The round-trip property the whole command rests on.
    let scratch = Scratch::new("sheet-round-trip");
    let path = scratch.join("a.flac");
    let body = rows(
        &path,
        "artist,album_artist,album,title,track_number,genres",
        "DECO*27,DECO*27,Ghost,Roki,3,Vocaloid",
    );

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, tagged())]);

    assert_eq!(skips(&planned), vec![Skipped::NoChange]);
}

#[test]
fn an_edited_cell_is_the_only_field_proposed() {
    let scratch = Scratch::new("sheet-one-field");
    let path = scratch.join("a.flac");
    let body = rows(&path, "artist,title", "DECO*27,Ghost Rule");

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, tagged())]);

    let changes = changed(&planned);
    assert_eq!(changes[0].fields, vec![TagField::Title]);
    assert_eq!(changes[0].proposed.title, "Ghost Rule");
    assert_eq!(changes[0].proposed.album, "Ghost");
}

#[test]
fn a_blank_cell_leaves_the_tag_alone() {
    // The commoner cause of an empty cell is a half-filled sheet, not an intent
    // to erase, and the wrong write costs more here than no write.
    let scratch = Scratch::new("sheet-blank-keeps");
    let path = scratch.join("a.flac");
    let body = rows(&path, "album", "");

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, tagged())]);

    assert_eq!(skips(&planned), vec![Skipped::NoChange]);
}

#[test]
fn a_blank_cell_clears_the_tag_when_the_run_asks_for_it() {
    let scratch = Scratch::new("sheet-blank-clears");
    let path = scratch.join("a.flac");
    let body = rows(&path, "album", "");

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::Clear).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, tagged())]);

    let changes = changed(&planned);
    assert_eq!(changes[0].fields, vec![TagField::Album]);
    assert_eq!(changes[0].proposed.album, "");
}

#[test]
fn a_column_absent_from_the_sheet_is_never_written() {
    // What makes a hand-written two-column sheet safe, even with --clear-blanks.
    let scratch = Scratch::new("sheet-absent-column");
    let path = scratch.join("a.flac");
    let body = rows(&path, "genres", "Touhou");

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::Clear).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, tagged())]);

    let changes = changed(&planned);
    assert_eq!(changes[0].fields, vec![TagField::Genres]);
    assert_eq!(changes[0].proposed.artist, "DECO*27");
    assert_eq!(changes[0].proposed.album, "Ghost");
    assert_eq!(changes[0].proposed.track_number, Some(3));
}

#[test]
fn a_row_whose_path_is_not_indexed_is_reported_rather_than_guessed_at() {
    let scratch = Scratch::new("sheet-unknown-path");
    let body = rows(&scratch.join("nowhere.flac"), "title", "Something");

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[]);

    assert_eq!(skips(&planned), vec![Skipped::NotIndexed]);
}

#[test]
fn a_container_that_cannot_carry_tags_is_reported() {
    let scratch = Scratch::new("sheet-untaggable");
    let path = scratch.join("a.wav");
    let body = rows(&path, "title", "Something");

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, TrackTags::default())]);

    assert_eq!(skips(&planned), vec![Skipped::NotTaggable]);
}

#[test]
fn a_derived_column_is_not_writable() {
    // `folder_artist` and `duration_seconds` are in the dump and are not tags.
    let scratch = Scratch::new("sheet-derived");
    let path = scratch.join("a.flac");
    let body = rows(
        &path,
        "folder_artist,duration_seconds",
        "Someone Else,999.9",
    );

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, tagged())]);

    assert_eq!(skips(&planned), vec![Skipped::NoChange]);
}

#[test]
fn a_sheet_without_a_path_column_is_rejected_by_name() {
    let scratch = Scratch::new("sheet-no-path");
    let path = sheet(&scratch, "artist,title\nZUN,Song\n");

    let error = read_sheet(&path, Blanks::LeaveAlone).expect_err("a rejected sheet");
    assert!(error.to_string().contains("no 'path' column"), "{error}");
    assert!(error.to_string().contains("artist, title"), "{error}");
}

#[test]
fn a_sheet_with_a_byte_order_mark_reads_the_same_as_one_without() {
    // `tags dump -o` writes a BOM, so without this the dump cannot be fed back
    // in: the first header would be named "\u{feff}path".
    let scratch = Scratch::new("sheet-bom");
    let path = scratch.join("a.flac");
    let body = rows(&path, "title", "Ghost Rule");

    let plain = sheet(&scratch, &body);
    let with_bom = scratch.join("bom.csv");
    std::fs::write(&with_bom, format!("\u{feff}{body}")).expect("a written sheet");

    assert_eq!(
        read_sheet(&plain, Blanks::LeaveAlone).expect("a sheet"),
        read_sheet(&with_bom, Blanks::LeaveAlone).expect("a sheet")
    );
}

#[test]
fn a_header_is_matched_whatever_its_case_and_spacing() {
    let scratch = Scratch::new("sheet-header-case");
    let path = scratch.join("a.flac");
    let body = format!("Path, TITLE \n\"{}\",Ghost Rule\n", path.display());

    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[entry(&path, tagged())]);

    assert_eq!(changed(&planned)[0].fields, vec![TagField::Title]);
}

#[test]
fn a_row_with_no_path_is_skipped_rather_than_read_as_one() {
    let scratch = Scratch::new("sheet-blank-row");
    let body = "path,title\n,Ghost Rule\n";

    let edits = read_sheet(&sheet(&scratch, body), Blanks::LeaveAlone).expect("a sheet");

    assert!(edits.is_empty());
}

#[test]
fn the_written_tags_are_built_from_the_file_and_not_from_the_index() {
    // The plan is diffed against a cache. Writing what the cache remembered
    // would silently revert every field the sheet never mentioned.
    needs_ffmpeg!();
    let scratch = Scratch::new("sheet-writes-from-file");
    let path = encode(
        &scratch.join("a.flac"),
        &Encoded {
            metadata: &[("artist", "DECO*27"), ("title", "Roki"), ("album", "Ghost")],
            ..Encoded::default()
        },
    );
    let stale = describe(&path, scratch.path()).expect("an entry");

    // The album moved on disk after the index last saw the file.
    let mut on_disk = tags::read_tags(&path).expect("readable tags");
    on_disk.album = "Ghost Rule".to_owned();
    tags::write_tags(&path, &on_disk).expect("an edited album");

    let body = rows(&path, "title", "Sanbika");
    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[stale]);

    stk::tagsheet::apply_change(changed(&planned)[0]).expect("a written title");

    let written = tags::read_tags(&path).expect("readable tags");
    assert_eq!(written.title, "Sanbika");
    assert_eq!(
        written.album, "Ghost Rule",
        "the album must not be reverted"
    );
}

#[test]
fn a_field_edited_on_disk_since_the_plan_blocks_that_row() {
    needs_ffmpeg!();
    let scratch = Scratch::new("sheet-stale-field");
    let path = encode(
        &scratch.join("a.flac"),
        &Encoded {
            metadata: &[("title", "Roki")],
            ..Encoded::default()
        },
    );
    let stale = describe(&path, scratch.path()).expect("an entry");

    let mut on_disk = tags::read_tags(&path).expect("readable tags");
    on_disk.title = "Someone Fixed This By Hand".to_owned();
    tags::write_tags(&path, &on_disk).expect("an edited title");

    let body = rows(&path, "title", "Sanbika");
    let edits = read_sheet(&sheet(&scratch, &body), Blanks::LeaveAlone).expect("a sheet");
    let planned = plan_changes(&edits, &[stale]);

    let error = stk::tagsheet::apply_change(changed(&planned)[0]).expect_err("a refused write");
    assert!(
        error.to_string().contains("changed since the plan"),
        "{error}"
    );
    assert_eq!(
        tags::read_tags(&path).expect("readable tags").title,
        "Someone Fixed This By Hand"
    );
}
