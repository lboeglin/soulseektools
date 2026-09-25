//! Laying files out from their tags, behind `stk organise`.

mod support;

use std::path::{Path, PathBuf};

use stk::audio::fingerprint::Fingerprint;
use stk::config::OrganiseSettings;
use stk::library::changes::ChangeFilter;
use stk::library::index::{default_extensions, describe, LibraryIndex};
use stk::library::scan::Excludes;
use stk::matching::NameForms;
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags};
use stk::organise::{
    apply_moves, loose_entries, move_file, plan_moves, prune_empty, render, spelled_on_disk,
    Obstacle, Pattern, PlannedMove,
};
use support::{encode, Encoded, Scratch};

fn settings() -> OrganiseSettings {
    OrganiseSettings::default()
}

fn pattern(text: &str) -> Pattern {
    Pattern::parse(text).expect("a valid pattern")
}

fn entry(path: &str, tags: TrackTags) -> LibraryEntry {
    let artist = if tags.artist.is_empty() {
        String::new()
    } else {
        tags.artist.clone()
    };
    let title = if tags.title.is_empty() {
        Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        tags.title.clone()
    };
    LibraryEntry {
        path: PathBuf::from(path),
        size: 1,
        mtime: 1.0,
        folder_artist: String::new(),
        artist: NameForms::of(&artist),
        title: NameForms::of(&title),
        duration_seconds: None,
        artwork_bytes: 0,
        tags,
        codec: String::new(),
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
        replay_gain: ReplayGainTags::default(),
    }
}

fn tagged(artist: &str, album: &str, title: &str, track: Option<i64>) -> TrackTags {
    TrackTags {
        artist: artist.to_owned(),
        album: album.to_owned(),
        title: title.to_owned(),
        track_number: track,
        ..TrackTags::default()
    }
}

fn rendered(path: &str, tags: TrackTags) -> String {
    render(
        &pattern("{artist}/{album}/{track} - {title}"),
        &entry(path, tags),
        &settings(),
    )
    .expect("a destination")
    .display()
    .to_string()
    .replace('\\', "/")
}

#[test]
fn a_tagged_file_lands_under_its_artist_album_and_number() {
    assert_eq!(
        rendered(
            "/d/a.flac",
            tagged("ZUN", "Mountain of Faith", "Native Faith", Some(5))
        ),
        "ZUN/Mountain of Faith/05 - Native Faith.flac"
    );
}

#[test]
fn a_track_number_is_padded_to_the_configured_width() {
    let settings = OrganiseSettings {
        track_number_width: 3,
        ..settings()
    };
    let destination = render(
        &pattern("{track} - {title}"),
        &entry("/d/a.flac", tagged("ZUN", "", "Native Faith", Some(5))),
        &settings,
    )
    .expect("a destination");

    assert_eq!(destination, PathBuf::from("005 - Native Faith.flac"));
}

#[test]
fn a_file_with_no_album_loses_the_album_directory() {
    // Dropped rather than filled in: inventing "Unknown Album" folders is the
    // mess this command exists to remove.
    assert_eq!(
        rendered("/d/a.flac", tagged("ZUN", "", "Native Faith", Some(5))),
        "ZUN/05 - Native Faith.flac"
    );
}

#[test]
fn a_file_with_no_track_number_loses_the_separator_with_it() {
    assert_eq!(
        rendered("/d/a.flac", tagged("ZUN", "Faith", "Native Faith", None)),
        "ZUN/Faith/Native Faith.flac"
    );
}

#[test]
fn a_trailing_placeholder_takes_the_literal_before_it() {
    let destination = render(
        &pattern("{title} ({album})"),
        &entry("/d/a.flac", tagged("ZUN", "", "Native Faith", None)),
        &settings(),
    )
    .expect("a destination");

    assert_eq!(destination, PathBuf::from("Native Faith.flac"));
}

#[test]
fn a_file_with_no_artist_at_all_lands_under_unknown_artist() {
    // The same fallback `fetch` already files an unnamed download under.
    assert_eq!(
        rendered("/d/a.flac", tagged("", "", "Native Faith", None)),
        "Unknown Artist/Native Faith.flac"
    );
}

#[test]
fn a_slash_in_an_artist_name_does_not_create_a_directory() {
    // `sanitize_folder_name` runs per component, so a
    // separator inside a name cannot become one on disk.
    assert_eq!(
        rendered("/d/a.flac", tagged("AC/DC", "", "Back in Black", None)),
        "AC_DC/Back in Black.flac"
    );
}

#[test]
fn the_extension_comes_from_the_file_and_not_the_pattern() {
    assert_eq!(
        rendered("/d/a.MP3", tagged("ZUN", "", "Song", None)),
        "ZUN/Song.MP3"
    );
}

#[test]
fn a_file_with_nothing_to_name_it_after_is_left_alone() {
    let mut nameless = entry("/d/a.flac", TrackTags::default());
    nameless.title = NameForms::of("");

    assert_eq!(
        render(&pattern("{artist}/{title}"), &nameless, &settings()),
        None
    );
}

#[test]
fn an_unknown_placeholder_is_rejected_when_the_pattern_is_parsed() {
    let error = Pattern::parse("{artist}/{bitrate} - {title}").expect_err("a rejected pattern");
    assert!(error.to_string().contains("{bitrate}"), "{error}");
    assert!(error.to_string().contains("{album}"), "{error}");
}

#[test]
fn a_pattern_without_a_title_is_rejected() {
    // Every track of an album would render to one name, and the collision rule
    // would then refuse the lot -- better said at load than per file.
    let error = Pattern::parse("{artist}/{album}").expect_err("a rejected pattern");
    assert!(error.to_string().contains("{title}"), "{error}");
}

#[test]
fn a_pattern_may_not_escape_the_root() {
    for bad in ["../{title}", "/{title}", "C:/{title}"] {
        assert!(Pattern::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_pattern_separates_directories_with_a_forward_slash_everywhere() {
    let error = Pattern::parse(r"{artist}\{title}").expect_err("a rejected pattern");
    assert!(error.to_string().contains('/'), "{error}");
}

#[test]
fn an_unclosed_placeholder_is_rejected() {
    assert!(Pattern::parse("{artist/{title}").is_err());
}

fn plan(entries: &[LibraryEntry], root: &Path) -> Vec<PlannedMove> {
    plan_moves(
        entries,
        &pattern("{artist}/{album}/{track} - {title}"),
        &settings(),
        std::slice::from_ref(&root.to_path_buf()),
        None,
    )
}

#[test]
fn a_file_already_in_place_is_not_moved() {
    let root = Path::new("/music");
    let planned = plan(
        &[entry(
            "/music/ZUN/Faith/05 - Native Faith.flac",
            tagged("ZUN", "Faith", "Native Faith", Some(5)),
        )],
        root,
    );

    assert_eq!(planned[0].blocked, Some(Obstacle::AlreadyInPlace));
}

#[test]
fn a_file_outside_every_root_is_left_alone() {
    let planned = plan(
        &[entry("/elsewhere/a.flac", tagged("ZUN", "", "Song", None))],
        Path::new("/music"),
    );

    assert_eq!(planned[0].blocked, Some(Obstacle::NoRoot));
}

#[test]
fn two_files_rendering_to_one_name_are_reported_before_anything_moves() {
    // Caught during planning, so the plan is truthful rather than the second
    // move discovering it.
    let same = tagged("ZUN", "Faith", "Native Faith", Some(5));
    let planned = plan(
        &[
            entry("/music/one.flac", same.clone()),
            entry("/music/two.flac", same),
        ],
        Path::new("/music"),
    );

    assert!(planned[0].ready());
    assert!(matches!(planned[1].blocked, Some(Obstacle::Claimed(_))));
}

#[test]
fn an_occupied_destination_is_never_overwritten() {
    let scratch = Scratch::new("organise-occupied");
    let root = scratch.join("Music");
    let source = encode(&root.join("loose.flac"), &Encoded::default());
    encode(&root.join("ZUN").join("Song.flac"), &Encoded::default());

    let planned = plan_moves(
        &[entry(
            &source.display().to_string(),
            tagged("ZUN", "", "Song", None),
        )],
        &pattern("{artist}/{title}"),
        &settings(),
        std::slice::from_ref(&root),
        None,
    );

    assert!(matches!(planned[0].blocked, Some(Obstacle::Occupied(_))));
}

#[test]
fn a_case_only_rename_is_a_move_and_not_a_collision() {
    // On NTFS the destination "exists" because it is the same file, and a plain
    // exists() check would refuse the one rename most worth making.
    let scratch = Scratch::new("organise-case");
    let root = scratch.join("Music");
    let source = encode(&root.join("ZUN").join("song.flac"), &Encoded::default());

    let planned = plan_moves(
        &[entry(
            &source.display().to_string(),
            tagged("ZUN", "", "Song", None),
        )],
        &pattern("{artist}/{title}"),
        &settings(),
        std::slice::from_ref(&root),
        None,
    );

    assert!(planned[0].ready(), "{:?}", planned[0].blocked);
    move_file(&source, &planned[0].destination).expect("a case-only rename");

    // `exists` cannot see the difference on Windows; the directory listing can.
    assert_eq!(
        spelled_on_disk(&planned[0].destination)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        Some("Song.flac".to_owned())
    );
}

/// Whether the scratch volume treats names differing only in case as one file.
fn folds_case(scratch: &Scratch) -> bool {
    let probe = scratch.join("case-probe");
    std::fs::write(&probe, b"").expect("a probe file");
    scratch.join("CASE-PROBE").exists()
}

#[test]
fn a_different_file_differing_only_in_case_is_occupied_where_case_matters() {
    // Folding case wherever the platform usually does would take this for a
    // case-only rename and overwrite a second file on a case-sensitive volume.
    let scratch = Scratch::new("organise-case-sensitive");
    if folds_case(&scratch) {
        return;
    }
    let root = scratch.join("Music");
    let source = encode(&root.join("ZUN").join("song.flac"), &Encoded::default());
    encode(&root.join("ZUN").join("Song.flac"), &Encoded::default());

    let planned = plan_moves(
        &[entry(
            &source.display().to_string(),
            tagged("ZUN", "", "Song", None),
        )],
        &pattern("{artist}/{title}"),
        &settings(),
        std::slice::from_ref(&root),
        None,
    );

    assert!(matches!(planned[0].blocked, Some(Obstacle::Occupied(_))));
}

#[test]
fn two_moves_differing_only_in_case_collide_where_the_volume_folds_case() {
    // On macOS the claim check compared exact spellings, so both moves were
    // ready and the second rename replaced the first file.
    let scratch = Scratch::new("organise-case-claimed");
    if !folds_case(&scratch) {
        return;
    }
    let root = scratch.join("Music");
    let planned = plan_moves(
        &[
            entry(
                &root.join("one.flac").display().to_string(),
                tagged("ZUN", "", "Song", None),
            ),
            entry(
                &root.join("two.flac").display().to_string(),
                tagged("zun", "", "song", None),
            ),
        ],
        &pattern("{artist}/{title}"),
        &settings(),
        std::slice::from_ref(&root),
        None,
    );

    assert!(planned[0].ready(), "{:?}", planned[0].blocked);
    assert!(matches!(planned[1].blocked, Some(Obstacle::Claimed(_))));
}

#[test]
fn a_move_carries_the_row_and_its_fingerprint_to_the_new_path() {
    let scratch = Scratch::new("organise-relocate");
    let root = scratch.join("Music");
    let source = encode(
        &root.join("loose.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("title", "Song")],
            ..Encoded::default()
        },
    );

    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");
    let described = describe(&source, &root).expect("an entry");
    index
        .reindex(&source, std::slice::from_ref(&root))
        .expect("a row");
    let shape = Fingerprint {
        values: vec![1, 2, 3],
        duration_seconds: Some(1.0),
    };
    index
        .store_fingerprint(&source, described.mtime, &shape)
        .expect("a fingerprint");

    let destination = root.join("ZUN").join("Song.flac");
    move_file(&source, &destination).expect("a move");
    index
        .relocate(&source, &destination, std::slice::from_ref(&root))
        .expect("a relocated row");

    assert!(index.entry(&source).expect("a lookup").is_none());
    let moved = index
        .entry(&destination)
        .expect("a lookup")
        .expect("the row at its new home");
    // Carried, not recomputed: the audio did not change because the file did.
    assert_eq!(
        index
            .fingerprint(&destination, moved.mtime)
            .expect("a lookup"),
        Some(shape)
    );
}

#[test]
fn an_emptied_source_directory_is_removed_up_to_the_root() {
    let scratch = Scratch::new("organise-prune");
    let root = scratch.join("Music");
    let nested = root.join("Artist").join("Album");
    std::fs::create_dir_all(&nested).expect("a nested directory");

    let removed = prune_empty(&nested.join("gone.flac"), &root);

    assert_eq!(removed.len(), 2);
    assert!(!root.join("Artist").exists());
    assert!(root.exists(), "the root itself is never removed");
}

#[test]
fn a_source_directory_still_holding_a_cover_is_kept() {
    // `--apply` on a command that moves files did not consent to deleting art.
    let scratch = Scratch::new("organise-prune-cover");
    let root = scratch.join("Music");
    let folder = root.join("Artist");
    std::fs::create_dir_all(&folder).expect("a directory");
    std::fs::write(folder.join("cover.jpg"), b"not empty").expect("a cover");

    let removed = prune_empty(&folder.join("gone.flac"), &root);

    assert!(removed.is_empty());
    assert!(folder.join("cover.jpg").exists());
}

fn download(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let downloads = scratch.join("downloads");
    let file = encode(
        &downloads.join("ZUN").join("01 peer name.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("album", "Lotus"), ("title", "Song")],
            ..Encoded::default()
        },
    );
    (downloads, file)
}

#[test]
fn loose_files_are_described_from_where_they_sit_rather_than_the_index() {
    let scratch = Scratch::new("organise-loose");
    let (downloads, file) = download(&scratch);

    let entries =
        loose_entries(&downloads, default_extensions(), &Excludes::default()).expect("entries");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, file);
    assert_eq!(entries[0].tags.title, "Song");
}

#[test]
fn applying_a_plan_moves_indexes_and_logs_each_file() {
    let scratch = Scratch::new("organise-apply");
    let (downloads, file) = download(&scratch);
    let root = scratch.join("Music");
    std::fs::create_dir_all(&root).expect("a root");
    let roots = vec![root.clone()];
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");

    let entries =
        loose_entries(&downloads, default_extensions(), &Excludes::default()).expect("entries");
    let planned = plan_moves(
        &entries,
        &pattern("{artist}/{album}/{title}"),
        &settings(),
        &roots,
        Some(&root),
    );
    let ready: Vec<&PlannedMove> = planned.iter().filter(|move_| move_.ready()).collect();
    let applied = apply_moves(&mut index, &ready, &roots, &settings(), 1.0).expect("applied");

    let landed = root.join("ZUN").join("Lotus").join("Song.flac");
    assert_eq!(applied.moved, 1);
    assert!(applied.failures.is_empty());
    assert!(landed.is_file());
    assert!(!file.exists());
    assert!(index.entry(&landed).expect("a lookup").is_some());
    let logged = index
        .changes(&ChangeFilter::default())
        .expect("the change log");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].command, "organise");
}

#[test]
fn a_move_that_fails_is_reported_and_the_rest_carry_on() {
    let scratch = Scratch::new("organise-apply-failed");
    let (downloads, _) = download(&scratch);
    let root = scratch.join("Music");
    let roots = vec![root.clone()];
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");

    let vanished = encode(
        &downloads.join("ZUN").join("02 peer name.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("album", "Lotus"), ("title", "Gone")],
            ..Encoded::default()
        },
    );

    let entries =
        loose_entries(&downloads, default_extensions(), &Excludes::default()).expect("entries");
    let planned = plan_moves(
        &entries,
        &pattern("{artist}/{title}"),
        &settings(),
        &roots,
        Some(&root),
    );
    std::fs::remove_file(&vanished).expect("the file gone before the move");
    let ready: Vec<&PlannedMove> = planned.iter().filter(|move_| move_.ready()).collect();
    let applied = apply_moves(&mut index, &ready, &roots, &settings(), 1.0).expect("applied");

    assert_eq!(applied.moved, 1);
    assert_eq!(applied.failures.len(), 1);
    assert!(root.join("ZUN").join("Song.flac").is_file());
}

#[test]
fn an_empty_placeholder_only_eats_the_literal_that_follows_it() {
    // With two placeholders side by side, an empty first one must not carry its
    // "skip the next literal" over the second and swallow a separator that
    // belongs to someone else.
    let destination = render(
        &pattern("{track}{album} - {title}"),
        &entry("/d/a.flac", tagged("ZUN", "Album", "Title", None)),
        &settings(),
    )
    .expect("a destination");

    assert_eq!(destination, PathBuf::from("Album - Title.flac"));
}

#[test]
fn an_empty_placeholder_between_two_others_leaves_them_both() {
    let destination = render(
        &pattern("{track}{album}{title}"),
        &entry("/d/a.flac", tagged("ZUN", "Album", "Title", None)),
        &settings(),
    )
    .expect("a destination");

    assert_eq!(destination, PathBuf::from("AlbumTitle.flac"));
}
