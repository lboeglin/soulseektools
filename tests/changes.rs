//! The `--apply` write log, and undoing from it.

mod support;

use std::path::{Path, PathBuf};

use stk::library::changes::{apply_undo, digest, plan_undo, Blocked, ChangeFilter, ChangeLog};
use stk::library::index::LibraryIndex;
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
            stk::library::scan::AUDIO_EXTENSIONS,
            &Excludes::default(),
            false,
            None,
        )
        .expect("a refresh");
    index
}

fn cover(byte: u8) -> Artwork {
    Artwork {
        data: vec![byte; 2048],
        mime: "image/jpeg".to_owned(),
    }
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
    assert!(plan.allowed(true));
}

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
fn undoing_one_field_leaves_another_edited_since_alone() {
    // Why the log keeps a row per field: a whole-block row would refuse, or revert, the album edit.
    let scratch = Scratch::new("changes-tag-undo-narrow");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &track, TagField::Title, "Native Faith");

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
fn undoing_a_move_is_blocked_when_something_took_the_old_place() {
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
fn undoing_dropped_extras_puts_the_comment_back() {
    let scratch = Scratch::new("changes-extras-undo");
    let root = library(&scratch);
    let track = commented(&scratch);
    let mut index = indexed(&scratch, &root);

    let log = ChangeLog::open(&index, "fetch download", 100.0).expect("a run");
    strip_extras(&mut index, &log, &track);
    assert_eq!(tags::read_extras(&track).expect("readable extras"), []);

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
fn undoing_a_year_puts_the_old_one_back_or_none() {
    let scratch = Scratch::new("changes-year-undo");
    let root = library(&scratch);
    let dated = root.join("ZUN").join("Heian Alien.flac");
    let undated = root.join("ZUN").join("Bad Apple.flac");
    let mut index = indexed(&scratch, &root);
    let mut start = tags::read_tags(&dated).expect("readable tags");
    start.year = "1998-09-09".to_owned();
    tags::write_tags(&dated, &start).expect("a starting year");

    let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
    write_field(&mut index, &log, &dated, TagField::Year, "2000");
    write_field(&mut index, &log, &undated, TagField::Year, "2000");

    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    let mut descriptions = Vec::new();
    for change in index
        .changes(&ChangeFilter::default())
        .expect("the logged rows")
    {
        assert_eq!(change.field, "year");
        let plan = plan_undo(&index, change).expect("a plan");
        assert!(plan.ready(), "{:?}", plan.blocked);
        descriptions.push(plan.description.clone());
        apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");
    }

    descriptions.sort();
    assert_eq!(
        descriptions,
        vec!["remove the year tag", "set the year back to 1998-09-09"]
    );
    assert_eq!(
        tags::read_tags(&dated).expect("readable tags").year,
        "1998-09-09"
    );
    assert_eq!(tags::read_tags(&undated).expect("readable tags").year, "");
}

#[test]
fn an_extras_row_logged_with_the_date_among_them_still_undoes() {
    // Before the year was managed, a dropped comment was logged alongside the
    // kept DATE; the date now reads as a field, not an extra.
    let scratch = Scratch::new("changes-extras-dated");
    let root = library(&scratch);
    let track = commented(&scratch);
    let mut start = tags::read_tags(&track).expect("readable tags");
    start.year = "2016".to_owned();
    tags::write_tags(&track, &start).expect("a year");
    let mut index = indexed(&scratch, &root);

    let date = tags::Extra {
        key: "DATE".to_owned(),
        value: "2016".to_owned(),
        locator: false,
    };
    let comment = tags::read_extras(&track).expect("readable extras");
    tags::write_extras(&track, &[]).expect("stripped extras");
    let log = ChangeLog::open(&index, "fetch download", 100.0).expect("a run");
    log.extras(
        &mut index,
        &track,
        &[vec![date.clone()], comment.clone()].concat(),
        &[date],
    )
    .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    assert!(plan.ready(), "{:?}", plan.blocked);
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

    assert_eq!(tags::read_extras(&track).expect("readable extras"), comment);
    assert_eq!(date_comments(&track), vec!["2016".to_owned()]);
}

fn date_comments(track: &Path) -> Vec<String> {
    use lofty::file::AudioFile;
    let mut file = std::fs::File::open(track).expect("a readable file");
    lofty::flac::FlacFile::read_from(&mut file, lofty::config::ParseOptions::new())
        .expect("a flac")
        .vorbis_comments()
        .map(|comments| comments.get_all("DATE").map(str::to_owned).collect())
        .unwrap_or_default()
}

#[test]
fn a_png_cover_is_restored_as_a_png() {
    let scratch = Scratch::new("changes-art-png");
    let root = library(&scratch);
    let track = root.join("ZUN").join("Heian Alien.flac");
    let mut index = indexed(&scratch, &root);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(2048, 7);
    let old = Artwork {
        data: png,
        mime: "image/png".to_owned(),
    };
    tags::write_artwork(&track, &old).expect("a starting cover");

    let log = ChangeLog::open(&index, "art set", 100.0).expect("a run");
    tags::write_artwork(&track, &cover(2)).expect("a replacement cover");
    log.artwork(&mut index, &track, Some(&old), Some(&cover(2)))
        .expect("a logged change");

    let change = index.change(1).expect("a lookup").expect("the row");
    let plan = plan_undo(&index, change).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, &[root]).expect("an undo");

    assert_eq!(
        tags::read_artwork(&track).expect("readable artwork"),
        Some(old)
    );
}
