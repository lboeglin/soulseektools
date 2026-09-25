//! The CLI end to end, in-process. Tests touching the environment or cwd hold one lock.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use support::{committed, encode, noise_source, Encoded, Scratch};

const SECRETS: &[&str] = &["SLSKD_API_KEY", "DISCOGS_TOKEN", "LASTFM_API_KEY"];

fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Harness {
    scratch: Scratch,
    saved: Vec<(String, String)>,
    directory: PathBuf,
}

impl Harness {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let saved: Vec<(String, String)> = std::env::vars()
            .filter(|(name, _)| name.starts_with("STK_") || SECRETS.contains(&name.as_str()))
            .collect();
        for (name, _) in &saved {
            unsafe { std::env::remove_var(name) };
        }

        let directory = std::env::current_dir().unwrap_or_default();
        std::env::set_current_dir(scratch.path()).expect("a working directory");

        Self {
            scratch,
            saved,
            directory,
        }
    }

    fn with_config(&self, root: &Path) {
        let body = format!(
            "[library]\nroot = {}\n\n[index]\npath = {}\n\n[review]\npath = {}\n\n\
             [genres]\nmap_path = {}\n",
            quoted(root),
            quoted(&self.scratch.join("library.db")),
            quoted(&self.scratch.join("review.db")),
            quoted(&self.scratch.join("genres.toml")),
        );
        std::fs::write(self.scratch.join("stk.toml"), body).expect("a written config");
    }

    fn join(&self, name: &str) -> PathBuf {
        self.scratch.join(name)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            unsafe { std::env::set_var(name, value) };
        }
        let _ = std::env::set_current_dir(&self.directory);
    }
}

fn quoted(path: &Path) -> String {
    toml::Value::String(path.display().to_string()).to_string()
}

/// stdout isn't captured, so tests assert on exit codes and on the files a command leaves.
fn run(arguments: &[&str]) -> i32 {
    let mut argv: Vec<std::ffi::OsString> = vec!["stk".into()];
    argv.extend(arguments.iter().map(Into::into));
    stk::cli::run(argv)
}

fn logged(harness: &Harness) -> Vec<stk::library::changes::Change> {
    stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index")
        .changes(&stk::library::changes::ChangeFilter::default())
        .expect("the logged rows")
}

fn genres_of(path: &Path) -> Vec<String> {
    stk::library::tags::read_tags(path)
        .expect("readable tags")
        .genres
}

fn library(harness: &Harness) -> PathBuf {
    let root = harness.join("Music");
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
        &root.join("ZUN").join("Heian Alien.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("title", "Heian Alien")],
            ..Encoded::default()
        },
    );
    root
}

#[test]
fn completions_are_printed_whatever_the_config_holds() {
    let _guard = exclusive();
    let harness = Harness::new("cli-completions-broken-config");
    std::fs::write(harness.join("stk.toml"), "not = [valid").expect("a written config");

    assert_eq!(run(&["completions", "bash"]), 0);
    assert_eq!(run(&["doctor"]), 1);
    drop(harness);
}

#[test]
fn a_missing_config_is_reported_rather_than_ignored() {
    let _guard = exclusive();
    let harness = Harness::new("cli-missing-config");
    assert_eq!(
        run(&[
            "--config",
            &harness.join("absent.toml").display().to_string(),
            "doctor"
        ]),
        1
    );
}

#[test]
fn config_init_writes_a_file_and_refuses_to_clobber_it() {
    let _guard = exclusive();
    let harness = Harness::new("cli-config-init");
    let target = harness.join("written.toml");
    let path = target.display().to_string();

    assert_eq!(run(&["config", "init", "--path", &path]), 0);
    assert!(target.is_file());
    let text = std::fs::read_to_string(&target).expect("a readable config");
    assert!(text.contains("SLSKD_API_KEY"), "{text}");

    assert_eq!(run(&["config", "init", "--path", &path]), 1);
}

#[test]
fn doctor_fails_when_the_library_root_is_absent() {
    let _guard = exclusive();
    let harness = Harness::new("cli-doctor-missing");
    harness.with_config(Path::new("Z:/nowhere-at-all"));
    assert_eq!(run(&["doctor"]), 1);
    drop(harness);
}

#[test]
fn library_scan_then_stats() {
    let _guard = exclusive();
    let harness = Harness::new("cli-scan");
    let root = library(&harness);
    harness.with_config(&root);

    assert_eq!(run(&["library", "scan"]), 0);
    assert_eq!(run(&["library", "stats"]), 0);
    assert_eq!(run(&["--json", "library", "stats"]), 0);

    let index = stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index the scan wrote");
    assert_eq!(index.count().expect("a count"), 2);
    drop(harness);
}

#[test]
fn tags_dump_writes_a_csv_with_a_bom() {
    let _guard = exclusive();
    let harness = Harness::new("cli-tags-dump");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let output = harness.join("tags.csv");
    assert_eq!(
        run(&["tags", "dump", "--output", &output.display().to_string()]),
        0
    );

    let bytes = std::fs::read(&output).expect("a written CSV");
    assert_eq!(&bytes[..3], b"\xef\xbb\xbf", "the BOM Excel needs");
    let text = String::from_utf8_lossy(&bytes[3..]);
    assert!(text.starts_with("path,folder_artist,artist"), "{text}");
    assert!(text.contains("DECO*27"), "{text}");
    drop(harness);
}

fn duplicated(harness: &Harness) -> (PathBuf, PathBuf, PathBuf) {
    let root = harness.join("Music");
    let tags: &[(&str, &str)] = &[("artist", "DECO*27"), ("title", "Roki")];
    let flac = encode(
        &root.join("DECO_27").join("Roki.flac"),
        &Encoded {
            metadata: tags,
            ..Encoded::default()
        },
    );
    let mp3 = encode(
        &root.join("Loose").join("Roki.mp3"),
        &Encoded {
            metadata: tags,
            ..Encoded::default()
        },
    );
    encode(
        &root.join("DECO_27").join("Ghost.flac"),
        &Encoded {
            seconds: 6.0,
            metadata: &[("artist", "DECO*27"), ("title", "Ghost Rule")],
            ..Encoded::default()
        },
    );
    (root, flac, mp3)
}

fn indexed(harness: &Harness) -> usize {
    stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index")
        .count()
        .expect("a count")
}

#[test]
fn dupes_resolve_without_the_flag_moves_nothing() {
    let _guard = exclusive();
    let harness = Harness::new("cli-dupes-resolve-dry");
    let (root, flac, mp3) = duplicated(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["dupes", "resolve"]), 0);
    assert_eq!(run(&["--json", "dupes", "resolve", "--keep", "largest"]), 0);

    assert!(flac.is_file());
    assert!(mp3.is_file());
    assert!(!harness.join("quarantine").exists());
    assert!(logged(&harness).is_empty());
    assert_eq!(indexed(&harness), 3);
    drop(harness);
}

#[test]
fn dupes_resolve_quarantines_the_lossy_copy_and_undo_puts_it_back() {
    let _guard = exclusive();
    let harness = Harness::new("cli-dupes-resolve-apply");
    let (root, flac, mp3) = duplicated(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["dupes", "resolve", "--apply"]), 0);

    let parked = harness
        .join("quarantine")
        .join("1")
        .join("Loose")
        .join("Roki.mp3");
    assert!(flac.is_file(), "the lossless copy is kept");
    assert!(parked.is_file(), "the lossy copy is quarantined");
    assert!(
        !root.join("Loose").exists(),
        "its emptied folder is removed"
    );
    assert_eq!(indexed(&harness), 2, "the index forgets the moved copy");
    let rows = logged(&harness);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].command, "dupes resolve");
    assert_eq!(rows[0].field, "move");

    assert_eq!(run(&["changes", "undo", "--last", "--apply"]), 0);

    assert!(mp3.is_file());
    assert!(!parked.exists());
    assert_eq!(indexed(&harness), 3);
    drop(harness);
}

#[test]
fn genre_apply_writes_and_logs_only_with_the_flag() {
    let _guard = exclusive();
    let harness = Harness::new("cli-genre-log");
    let root = library(&harness);
    harness.with_config(&root);
    std::fs::write(
        harness.join("genres.toml"),
        "ZUN = \"Touhou\"
",
    )
    .expect("a genre map");
    run(&["library", "scan"]);

    let track = root.join("ZUN").join("Heian Alien.flac");
    assert_eq!(run(&["genre", "apply"]), 0);
    assert!(genres_of(&track).is_empty());
    assert!(logged(&harness).is_empty());

    assert_eq!(run(&["genre", "apply", "--apply"]), 0);

    assert_eq!(genres_of(&track), vec!["Touhou".to_owned()]);
    let changes = logged(&harness);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].command, "genre apply");
    assert_eq!(changes[0].field, "genre");
    drop(harness);
}

/// The table and JSON renderers are otherwise only reached with nothing to show.
#[test]
fn every_report_renders_over_a_populated_library() {
    let _guard = exclusive();
    let harness = Harness::new("cli-reports");
    let root = library(&harness);
    harness.with_config(&root);
    std::fs::write(harness.join("genres.toml"), "ZUN = \"Touhou\"\n").expect("a genre map");
    run(&["library", "scan"]);
    run(&["genre", "apply", "--apply"]);

    for command in [
        &["config", "show"][..],
        &["config", "path"],
        &["library", "stats"],
        &["dupes", "titles"],
        &["art", "check"],
        &["genre", "check"],
        &["changes", "list"],
        &["changes", "list", "--command", "genre apply"],
        &["loudness"],
        &["review", "list"],
    ] {
        assert_eq!(run(command), 0, "{command:?}");
        let mut json = vec!["--json"];
        json.extend_from_slice(command);
        assert_eq!(run(&json), 0, "--json {command:?}");
    }
    drop(harness);
}

#[test]
fn changes_undo_puts_a_genre_back_only_with_apply() {
    let _guard = exclusive();
    let harness = Harness::new("cli-changes-undo");
    let root = library(&harness);
    harness.with_config(&root);
    std::fs::write(
        harness.join("genres.toml"),
        "ZUN = \"Touhou\"
",
    )
    .expect("a genre map");
    run(&["library", "scan"]);
    run(&["genre", "apply", "--apply"]);

    let track = root.join("ZUN").join("Heian Alien.flac");
    assert_eq!(genres_of(&track), vec!["Touhou".to_owned()]);

    assert_eq!(run(&["changes", "undo", "--last"]), 0);
    assert_eq!(genres_of(&track), vec!["Touhou".to_owned()]);

    assert_eq!(run(&["changes", "undo", "--last", "--apply"]), 0);
    assert!(genres_of(&track).is_empty());
    drop(harness);
}

/// Noise, because the short tone in `library` is too narrow to classify.
fn verifiable_library(harness: &Harness) -> PathBuf {
    let root = harness.join("Music");
    noise_source(&root.join("ZUN").join("Noise.flac"), 8, 44100);
    root
}

#[test]
fn verify_changed_skips_a_file_that_still_carries_its_verdict() {
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-changed");
    let root = verifiable_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);
    run(&["verify"]);

    let report = harness.join("second.csv");
    assert_eq!(
        run(&[
            "verify",
            "--changed",
            "--report",
            &report.display().to_string()
        ]),
        0
    );

    assert!(!report.exists());
    drop(harness);
}

fn library_with_an_upsampled_mp3(harness: &Harness) -> PathBuf {
    let root = verifiable_library(harness);
    let mp3 = root.join("ZUN").join("Upsampled.mp3");
    std::fs::copy(committed("noise-128k-as-320k.mp3"), &mp3).expect("a copied fixture");
    root
}

fn reported(harness: &Harness, arguments: &[&str]) -> (i32, String) {
    let report = harness.join("verify.csv");
    let _ = std::fs::remove_file(&report);
    let mut full = arguments.to_vec();
    let destination = report.display().to_string();
    full.extend(["--report", &destination]);
    let code = run(&full);
    (code, std::fs::read_to_string(&report).unwrap_or_default())
}

#[test]
fn a_library_sweep_checks_mp3s_only_when_asked() {
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-lossy");
    let root = library_with_an_upsampled_mp3(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let (_, lossless) = reported(&harness, &["verify"]);
    assert!(!lossless.contains("Upsampled.mp3"), "{lossless}");
    assert!(lossless.contains("Noise.flac"), "{lossless}");

    let (code, everything) = reported(&harness, &["verify", "--lossy"]);
    assert_eq!(code, 0);
    assert!(
        everything.contains("Upsampled.mp3,UPSAMPLED,"),
        "{everything}"
    );

    let index = stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index the scan wrote");
    assert_eq!(
        index
            .verdict(&root.join("ZUN").join("Upsampled.mp3"))
            .expect("a lookup"),
        Some(stk::models::Verdict::Upsampled)
    );
    drop(harness);
}

/// `from` and `to` include their commas, so only the cell is replaced, not the path.
fn edited_dump(harness: &Harness, from: &str, to: &str) -> PathBuf {
    let sheet = round_trip_dump(harness);
    let text = String::from_utf8(std::fs::read(&sheet).expect("a written dump")).expect("UTF-8");
    std::fs::write(&sheet, text.replace(from, to)).expect("an edited sheet");
    sheet
}

fn round_trip_dump(harness: &Harness) -> PathBuf {
    let sheet = harness.join("sheet.csv");
    assert_eq!(
        run(&[
            "tags",
            "dump",
            "--round-trip",
            "-o",
            &sheet.display().to_string(),
        ]),
        0
    );
    sheet
}

fn title_of(track: &Path) -> String {
    stk::library::tags::read_tags(track)
        .expect("readable tags")
        .title
}

#[test]
fn tags_apply_without_the_flag_writes_nothing() {
    let _guard = exclusive();
    let harness = Harness::new("cli-tags-apply-dry");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let sheet = edited_dump(&harness, ",Heian Alien,", ",Native Faith,");
    assert_eq!(run(&["tags", "apply", &sheet.display().to_string()]), 0);

    assert_eq!(
        title_of(&root.join("ZUN").join("Heian Alien.flac")),
        "Heian Alien"
    );
    assert!(logged(&harness).is_empty());
    drop(harness);
}

#[test]
fn tags_apply_writes_the_edited_field_and_logs_it() {
    let _guard = exclusive();
    let harness = Harness::new("cli-tags-apply");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let sheet = edited_dump(&harness, ",Heian Alien,", ",Native Faith,");
    assert_eq!(
        run(&["tags", "apply", &sheet.display().to_string(), "--apply"]),
        0
    );

    let track = root.join("ZUN").join("Heian Alien.flac");
    let read = stk::library::tags::read_tags(&track).expect("readable tags");
    assert_eq!(read.title, "Native Faith");
    assert_eq!(read.artist, "ZUN", "no other field moved");

    let changes = logged(&harness);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].command, "tags apply");
    assert_eq!(changes[0].field, "title");

    let index =
        stk::library::index::LibraryIndex::open(&harness.join("library.db")).expect("the index");
    assert_eq!(
        index
            .entry(&track)
            .expect("a lookup")
            .expect("the row")
            .tags
            .title,
        "Native Faith"
    );
    drop(harness);
}

#[test]
fn tags_identify_reports_a_file_it_cannot_fingerprint() {
    // Opus can be tagged but not decoded, so it can't be looked up.
    let _guard = exclusive();
    let harness = Harness::new("cli-identify-opus");
    let root = library(&harness);
    let track = encode(
        &root.join("Unknown").join("01 track.opus"),
        &Encoded::default(),
    );
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["tags", "identify", "--apply"]), 0);
    assert_eq!(run(&["--json", "tags", "identify"]), 0);

    assert_eq!(title_of(&track), "");
    assert!(logged(&harness).is_empty());
    drop(harness);
}

#[test]
fn organise_moves_the_file_and_carries_its_index_row_along() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["organise", "--apply"]), 0);

    let moved = root.join("DECO_27").join("Ghost").join("Roki.flac");
    assert!(moved.is_file(), "{}", moved.display());
    assert!(!root.join("DECO_27").join("01 - Roki.flac").exists());

    let index =
        stk::library::index::LibraryIndex::open(&harness.join("library.db")).expect("the index");
    assert!(index.entry(&moved).expect("a lookup").is_some());
    assert!(index
        .entry(&root.join("DECO_27").join("01 - Roki.flac"))
        .expect("a lookup")
        .is_none());

    let changes = logged(&harness);
    assert!(changes.iter().any(|row| row.field == "move"));
    assert!(changes.iter().all(|row| row.command == "organise"));
    drop(harness);
}

#[test]
fn organise_refuses_a_destination_outside_every_root() {
    // Otherwise the moved files leave the library and the next scan evicts them.
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-outside");
    let root = library(&harness);
    let elsewhere = harness.join("Elsewhere");
    std::fs::write(
        harness.join("stk.toml"),
        format!(
            "[library]
root = {}

[index]
path = {}

[review]
path = {}

             [organise]
destination = {}
",
            quoted(&root),
            quoted(&harness.join("library.db")),
            quoted(&harness.join("review.db")),
            quoted(&elsewhere),
        ),
    )
    .expect("a written config");
    run(&["library", "scan"]);

    assert_eq!(run(&["organise"]), 1);
    assert!(root.join("DECO_27").join("01 - Roki.flac").is_file());
    drop(harness);
}

#[test]
fn organise_from_moves_a_download_into_the_library() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-from");
    let root = library(&harness);
    harness.with_config(&root);
    let downloads = harness.join("downloads");
    let fetched = encode(
        &downloads.join("ZUN").join("01 peer name.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("album", "Lotus"), ("title", "Song")],
            ..Encoded::default()
        },
    );
    let from = downloads.display().to_string();

    assert_eq!(run(&["organise", "--from", &from]), 0);
    assert!(fetched.is_file(), "nothing moves without --apply");

    assert_eq!(run(&["organise", "--from", &from, "--apply"]), 0);
    assert!(!fetched.exists());
    assert!(root.join("ZUN").join("Lotus").join("Song.flac").is_file());
    drop(harness);
}

#[test]
fn an_exported_playlist_keeps_the_export_row_order() {
    // The index reads back sorted by path, which would put DECO_27 before ZUN.
    let _guard = exclusive();
    let harness = Harness::new("cli-playlist-export");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let export = harness.join("export.csv");
    std::fs::write(
        &export,
        "Track name,Artist name
Heian Alien,ZUN
Roki,DECO*27
",
    )
    .expect("a written export");

    let list = harness.join("export.m3u8");
    assert_eq!(
        run(&[
            "playlist",
            "export",
            &export.display().to_string(),
            "-o",
            &list.display().to_string(),
        ]),
        0
    );

    let text = std::fs::read_to_string(&list).expect("a written playlist");
    let heian = text.find("Heian Alien").expect("the first export row");
    let roki = text.find("Roki").expect("the second export row");
    assert!(heian < roki, "{text}");
    drop(harness);
}

#[test]
fn wantlist_diff_queues_what_is_missing() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let export = harness.join("export.csv");
    std::fs::write(
        &export,
        "Track name,Artist name\nRoki,DECO*27\nSenbonzakura,Hatsune Miku\n",
    )
    .expect("a written export");

    assert_eq!(run(&["wantlist", "diff", &export.display().to_string()]), 0);

    let store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue");
    let tracks = store.tracks(&[], None).expect("tracks");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].track.title, "Senbonzakura");
    drop(harness);
}

#[test]
fn a_lastfm_profile_without_an_api_key_is_refused_before_any_request() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-lastfm-no-key");
    let root = library(&harness);
    harness.with_config(&root);

    assert_eq!(run(&["wantlist", "diff", "lastfm:rj"]), 1);
    assert_eq!(run(&["sync", "lastfm:rj"]), 1);
    assert_eq!(run(&["playlist", "export", "lastfm:rj"]), 1);
    drop(harness);
}

#[test]
fn a_track_the_queue_gave_up_on_is_not_reported_as_new_again() {
    // A FAILED track is still missing, so a plain diff reports it on every run.
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-new-failed");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let export = harness.join("export.csv");
    std::fs::write(
        &export,
        "Track name,Artist name
Senbonzakura,Hatsune Miku
",
    )
    .expect("a written export");
    run(&["wantlist", "diff", &export.display().to_string()]);

    {
        let mut store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
            .expect("the review queue");
        let tracks = store.tracks(&[], None).expect("tracks");
        store
            .mark(
                tracks[0].id,
                stk::models::TrackStatus::Failed,
                "nothing found",
            )
            .expect("a marked track");
    }

    let report = harness.join("missing.csv");
    assert_eq!(
        run(&[
            "wantlist",
            "diff",
            &export.display().to_string(),
            "--new",
            "--missing",
            &report.display().to_string(),
        ]),
        0
    );

    let text = std::fs::read_to_string(&report).expect("a missing report");
    assert!(!text.contains("Senbonzakura"), "{text}");
    drop(harness);
}

#[test]
fn review_pick_refuses_to_run_as_json() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-pick-json");
    harness.with_config(&harness.join("Music"));

    // In JSON mode the prompt would be invisible, hanging a pipeline.
    assert_eq!(run(&["--json", "review", "pick"]), 1);
    drop(harness);
}

#[test]
fn review_pick_on_an_empty_queue_says_so() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-pick-empty");
    harness.with_config(&harness.join("Music"));

    // No input reads as quit, not agreement, so this returns instead of blocking.
    assert_eq!(run(&["review", "pick"]), 0);
    drop(harness);
}

fn album_in_review(harness: &Harness) {
    let mut store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue");
    for title in ["Roki", "Ghost Rule"] {
        store
            .add(&[stk::models::Track::new("DECO*27", title)])
            .expect("an added track");
        let track_id = store
            .tracks(&[], None)
            .expect("tracks")
            .into_iter()
            .find(|entry| entry.track.title == title)
            .expect("the track just added")
            .id;
        store
            .set_candidates(
                track_id,
                &[stk::models::ScoredCandidate {
                    candidate: stk::models::Candidate {
                        username: "bulk".to_owned(),
                        remote_path: format!(r"@@bulk\Music\Best Of\{title}.flac"),
                        size: 30_000_000,
                        bit_depth: Some(16),
                        sample_rate: Some(44100),
                        free_upload_slots: true,
                        ..stk::models::Candidate::default()
                    },
                    score: 100,
                    title_ratio: 0.99,
                    flags: Vec::new(),
                }],
            )
            .expect("recorded candidates");
    }
}

#[test]
fn approving_an_album_takes_every_track_its_folder_covers() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-albums-approve");
    harness.with_config(&harness.join("Music"));
    album_in_review(&harness);

    assert_eq!(run(&["review", "albums", "--tracks"]), 0);
    assert_eq!(run(&["review", "approve-album", "1"]), 0);

    let store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue");
    let tracks = store.tracks(&[], None).expect("tracks");
    assert_eq!(tracks.len(), 2);
    assert!(tracks
        .iter()
        .all(|entry| entry.status == stk::models::TrackStatus::Approved));
    assert_eq!(run(&["review", "approve-album", "1"]), 1);
    drop(harness);
}

#[test]
fn get_accepts_one_track_several_tracks_and_a_list() {
    use stk::cli::commands::get::{wanted_tracks, GetArgs};

    let scratch = Scratch::new("get-input");
    let args = |query: Vec<&str>, from: Option<PathBuf>| GetArgs {
        query: query.into_iter().map(str::to_owned).collect(),
        from,
        artist: None,
        album: false,
        lossless: false,
        min_bitrate: None,
        yes: false,
        queue_only: false,
        limit: 5,
    };

    let one = wanted_tracks(&args(vec!["DECO*27 - Roki"], None)).expect("one track");
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].query(), "DECO*27 - Roki");

    let several = wanted_tracks(&args(
        vec!["DECO*27 - Roki", "ZUN - Heian Alien", "Ado - Odo"],
        None,
    ))
    .expect("several tracks");
    assert_eq!(several.len(), 3);
    assert_eq!(several[2].query(), "Ado - Odo");

    let list = scratch.join("wanted.txt");
    std::fs::write(
        &list,
        "# tracks to chase down\nDECO*27 - Roki\n\n  ZUN - Heian Alien  \n# not this one\n",
    )
    .expect("a written list");

    let from_file = wanted_tracks(&args(Vec::new(), Some(list.clone()))).expect("a list");
    assert_eq!(from_file.len(), 2);
    assert_eq!(from_file[0].query(), "DECO*27 - Roki");
    assert_eq!(from_file[1].query(), "ZUN - Heian Alien");

    // Arguments and a list combine.
    let both = wanted_tracks(&args(vec!["Ado - Odo"], Some(list))).expect("both");
    assert_eq!(both.len(), 3);
}

fn gain_of(path: &Path) -> Option<String> {
    stk::library::tags::read_replay_gain(path)
        .expect("readable loudness tags")
        .track_gain
}

fn levelled_library(harness: &Harness) -> PathBuf {
    let root = harness.join("Music");
    for (name, title) in [
        ("01 - Roki.flac", "Roki"),
        ("02 - Ghost Rule.flac", "Ghost Rule"),
    ] {
        encode(
            &root.join("DECO_27").join(name),
            &Encoded {
                seconds: 2.0,
                modulation_hz: 0.5,
                metadata: &[("artist", "DECO*27"), ("title", title), ("album", "Ghost")],
                ..Encoded::default()
            },
        );
    }
    encode(
        &root.join("ZUN").join("Heian Alien.flac"),
        &Encoded {
            seconds: 2.0,
            modulation_hz: 0.5,
            metadata: &[("artist", "ZUN"), ("title", "Heian Alien")],
            ..Encoded::default()
        },
    );
    root
}

#[test]
fn loudness_with_the_flag_writes_a_gain_and_logs_it() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-apply");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["loudness", "--apply"]), 0);

    assert!(gain_of(&root.join("ZUN").join("Heian Alien.flac")).is_some());
    let changes = logged(&harness);
    assert_eq!(changes.len(), 3);
    assert!(changes.iter().all(|change| change.command == "loudness"));
    assert!(changes.iter().all(|change| change.field == "replay_gain"));
    drop(harness);
}

#[test]
fn loudness_leaves_a_file_that_already_carries_a_gain_alone() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-skip");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);
    let track = root.join("ZUN").join("Heian Alien.flac");
    stk::library::tags::write_replay_gain(
        &track,
        &stk::models::ReplayGainTags {
            track_gain: Some("-99.00 dB".to_owned()),
            ..stk::models::ReplayGainTags::default()
        },
    )
    .expect("an earlier scan");

    assert_eq!(
        run(&["loudness", "--apply", &track.display().to_string()]),
        0
    );

    assert_eq!(gain_of(&track), Some("-99.00 dB".to_owned()));
    assert!(logged(&harness).is_empty());
    drop(harness);
}

/// Regression: failed files were subtracted from the plan twice and the count underflowed.
#[test]
fn loudness_reports_what_it_wrote_when_most_files_could_not_be_read() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-mostly-broken");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    for name in ["x.flac", "y.flac", "z.flac", "w.flac"] {
        std::fs::write(root.join("ZUN").join(name), b"not audio").expect("a written file");
    }

    assert_eq!(run(&["loudness", "--apply"]), 0);

    let changes = logged(&harness);
    assert_eq!(changes.len(), 3);
    drop(harness);
}

#[test]
fn loudness_gives_no_album_gain_to_one_track_of_a_record() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-partial");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let track = root.join("DECO_27").join("01 - Roki.flac");
    assert_eq!(
        run(&["loudness", "--apply", &track.display().to_string()]),
        0
    );

    let written = stk::library::tags::read_replay_gain(&track).expect("readable loudness tags");
    assert!(written.track_gain.is_some());
    assert_eq!(written.album_gain, None);
    assert_eq!(written.album_peak, None);
    drop(harness);
}

#[test]
fn get_album_takes_one_album_and_not_a_list() {
    let _guard = exclusive();
    let harness = Harness::new("cli-get-album-one");
    harness.with_config(&harness.join("Music"));

    let list = harness.join("albums.txt");
    std::fs::write(&list, "DECO*27 - Ghost Rule EP\n").expect("a written list");

    assert_eq!(
        run(&[
            "get",
            "--album",
            "DECO*27 - Ghost Rule EP",
            "ZUN - Heian Alien"
        ]),
        1
    );
    assert_eq!(
        run(&["get", "--album", "--from", &list.display().to_string()]),
        1
    );
    assert_eq!(run(&["get", "--album"]), 1);
    drop(harness);
}

fn sync_harness(name: &str) -> (Harness, PathBuf, PathBuf) {
    let harness = Harness::new(name);
    let root = library(&harness);
    harness.with_config(&root);
    let downloads = harness.join("downloads");
    let config = std::fs::read_to_string(harness.join("stk.toml")).expect("the config");
    std::fs::write(
        harness.join("stk.toml"),
        format!(
            "{config}\n[slskd]\ndownload_directory = {}\n",
            quoted(&downloads)
        ),
    )
    .expect("a written config");

    let export = harness.join("export.csv");
    std::fs::write(
        &export,
        "Track name,Artist name\nRoki,DECO*27\nSenbonzakura,Hatsune Miku\n",
    )
    .expect("a written export");
    (harness, downloads, export)
}

fn fetched(downloads: &Path) -> PathBuf {
    encode(
        &downloads.join("ZUN").join("01 peer name.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("album", "Lotus"), ("title", "Song")],
            ..Encoded::default()
        },
    )
}

fn queued_titles(harness: &Harness) -> Vec<String> {
    stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue")
        .tracks(&[], None)
        .expect("tracks")
        .into_iter()
        .map(|entry| entry.track.title)
        .collect()
}

#[test]
fn sync_under_json_without_yes_is_refused_before_anything_runs() {
    let _guard = exclusive();
    let (harness, _, export) = sync_harness("cli-sync-json");

    assert_eq!(run(&["--json", "sync", &export.display().to_string()]), 1);
    assert!(!harness.join("review.db").exists());
    assert!(!harness.join("library.db").exists());
    drop(harness);
}

#[test]
fn sync_without_apply_queues_what_is_missing_and_moves_nothing() {
    let _guard = exclusive();
    let (harness, downloads, export) = sync_harness("cli-sync-dry");
    let download = fetched(&downloads);

    // `--limit 0` searches nothing, which keeps slskd out of the test.
    assert_eq!(
        run(&[
            "sync",
            "--yes",
            "--limit",
            "0",
            &export.display().to_string()
        ]),
        0
    );

    assert_eq!(queued_titles(&harness), vec!["Senbonzakura".to_owned()]);
    assert!(download.is_file(), "nothing moves without --apply");
    assert!(logged(&harness).is_empty());
    drop(harness);
}

#[test]
fn sync_apply_organises_what_arrived_and_scans_it_in() {
    let _guard = exclusive();
    let (harness, downloads, export) = sync_harness("cli-sync-apply");
    let download = fetched(&downloads);

    assert_eq!(
        run(&[
            "sync",
            "--yes",
            "--apply",
            "--limit",
            "0",
            &export.display().to_string()
        ]),
        0
    );

    let landed = harness
        .join("Music")
        .join("ZUN")
        .join("Lotus")
        .join("Song.flac");
    assert!(!download.exists());
    assert!(landed.is_file());
    let index =
        stk::library::index::LibraryIndex::open(&harness.join("library.db")).expect("the index");
    assert!(index.entry(&landed).expect("a lookup").is_some());
    assert_eq!(index.snapshots(None).expect("the series")[0].total, 3);
    assert!(logged(&harness).iter().all(|row| row.command == "organise"));
    drop(harness);
}

#[test]
fn trend_reports_the_measurements_it_recorded() {
    let _guard = exclusive();
    let harness = Harness::new("cli-trend-report");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    encode(
        &root.join("ZUN").join("Native Faith.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("title", "Native Faith")],
            ..Encoded::default()
        },
    );
    run(&["library", "scan"]);

    assert_eq!(run(&["library", "trend"]), 0);
    assert_eq!(run(&["--json", "library", "trend"]), 0);

    let index =
        stk::library::index::LibraryIndex::open(&harness.join("library.db")).expect("the index");
    let totals: Vec<usize> = index
        .snapshots(None)
        .expect("the series")
        .iter()
        .map(|row| row.total)
        .collect();
    assert_eq!(totals, vec![3, 2]);
    drop(harness);
}
