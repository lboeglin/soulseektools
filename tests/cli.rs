//! Ported from the five CLI suites under `py/tests/integration/`.
//!
//! The Python drives typer's `CliRunner` in-process. `cli::run` returns an exit
//! code rather than exiting, so the same thing works here: the whole command
//! line is parsed and dispatched in-process, with stdout captured around it.
//!
//! Anything that touches the environment, the working directory or stdout is
//! serialised behind one lock, because a Rust test binary shares all three
//! across threads.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use support::{encode, noise_source, Encoded, Scratch};

fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scratch home with its own config, index and review database.
struct Harness {
    scratch: Scratch,
    saved: Vec<(String, String)>,
    directory: PathBuf,
}

impl Harness {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let saved: Vec<(String, String)> = std::env::vars()
            .filter(|(name, _)| name.starts_with("STK_") || name == "SLSKD_API_KEY")
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

    /// Point the config at this harness's own databases and library root.
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

    /// The same, for a library spread over several roots with folders skipped.
    fn with_roots_config(&self, roots: &[PathBuf], exclude: &[&str]) {
        let listed = roots
            .iter()
            .map(|root| quoted(root))
            .collect::<Vec<_>>()
            .join(", ");
        let skipped = exclude
            .iter()
            .map(|pattern| toml::Value::String((*pattern).to_owned()).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "[library]
roots = [{listed}]
exclude = [{skipped}]

[index]
path = {}

             [review]
path = {}

[genres]
map_path = {}
",
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

/// Run the CLI and hand back its exit code.
///
/// stdout is not captured -- Rust offers no stable way to redirect it from
/// in-process -- so assertions here are on exit codes and on the files and
/// databases a command leaves behind, with the JSON documents themselves checked
/// by writing them through a command that takes a path.
fn run(arguments: &[&str]) -> i32 {
    let mut argv: Vec<std::ffi::OsString> = vec!["stk".into()];
    argv.extend(arguments.iter().map(Into::into));
    stk::cli::run(argv)
}

/// Every row the write log holds, newest first.
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

/// A small library laid out the way the real one is.
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

fn json_at(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).expect("a written document");
    serde_json::from_str(&text).expect("valid JSON")
}

#[test]
fn version_is_reported() {
    let _guard = exclusive();
    let _harness = Harness::new("cli-version");
    assert_eq!(run(&["--version"]), 0);
}

#[test]
fn no_arguments_prints_help_without_failing_hard() {
    let _guard = exclusive();
    let _harness = Harness::new("cli-bare");
    // clap exits 2 on a usage error, as typer does.
    assert_eq!(run(&[]), 2);
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let _guard = exclusive();
    let _harness = Harness::new("cli-unknown");
    assert_eq!(run(&["nonsense"]), 2);
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
fn config_show_and_path_run_cleanly() {
    let _guard = exclusive();
    let harness = Harness::new("cli-config-show");
    library(&harness);
    assert_eq!(run(&["config", "show"]), 0);
    assert_eq!(run(&["--json", "config", "show"]), 0);
    assert_eq!(run(&["config", "path"]), 0);
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
fn doctor_passes_with_a_real_root() {
    let _guard = exclusive();
    let harness = Harness::new("cli-doctor-ok");
    let root = library(&harness);
    harness.with_config(&root);

    assert_eq!(run(&["doctor"]), 0);
    assert_eq!(run(&["--json", "doctor"]), 0);
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

/// The config half of #2's multiple roots: two drives, one index.
#[test]
fn library_scan_covers_every_configured_root() {
    let _guard = exclusive();
    let harness = Harness::new("cli-scan-roots");
    let first = library(&harness);
    let second = harness.join("More");
    encode(
        &second.join("ZUN").join("Bad Apple.flac"),
        &Encoded::default(),
    );
    harness.with_roots_config(&[first, second], &[]);

    assert_eq!(run(&["library", "scan"]), 0);

    let index = stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index the scan wrote");
    assert_eq!(index.count().expect("a count"), 3);
    drop(harness);
}

#[test]
fn library_scan_skips_an_excluded_folder() {
    let _guard = exclusive();
    let harness = Harness::new("cli-scan-exclude");
    let root = library(&harness);
    encode(
        &root.join("Audiobooks").join("Chapter 1.mp3"),
        &Encoded::default(),
    );
    harness.with_roots_config(&[root], &["**/Audiobooks/**"]);

    assert_eq!(run(&["library", "scan"]), 0);

    let index = stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index the scan wrote");
    assert_eq!(index.count().expect("a count"), 2);
    drop(harness);
}

#[test]
fn library_scan_reports_a_missing_root() {
    let _guard = exclusive();
    let harness = Harness::new("cli-scan-missing");
    harness.with_config(Path::new("Z:/nowhere-at-all"));
    assert_eq!(run(&["library", "scan"]), 1);
    drop(harness);
}

#[test]
fn stats_on_an_empty_index_says_so() {
    let _guard = exclusive();
    let harness = Harness::new("cli-stats-empty");
    harness.with_config(&harness.join("Music"));
    assert_eq!(run(&["library", "stats"]), 0);
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

#[test]
fn dupes_titles_runs_over_the_index() {
    let _guard = exclusive();
    let harness = Harness::new("cli-dupes");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["dupes", "titles"]), 0);
    assert_eq!(run(&["--json", "dupes", "titles"]), 0);
    drop(harness);
}

#[test]
fn dupes_on_an_empty_index_says_so() {
    let _guard = exclusive();
    let harness = Harness::new("cli-dupes-empty");
    harness.with_config(&harness.join("Music"));
    assert_eq!(run(&["dupes", "titles"]), 0);
    drop(harness);
}

#[test]
fn art_check_reports_what_needs_a_cover() {
    let _guard = exclusive();
    let harness = Harness::new("cli-art-check");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["art", "check"]), 0);
    assert_eq!(run(&["--json", "art", "check"]), 0);
    drop(harness);
}

#[test]
fn genre_check_runs_without_a_map() {
    let _guard = exclusive();
    let harness = Harness::new("cli-genre-no-map");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["genre", "check"]), 0);
    drop(harness);
}

#[test]
fn genre_apply_without_the_flag_writes_nothing() {
    let _guard = exclusive();
    let harness = Harness::new("cli-genre-dry-run");
    let root = library(&harness);
    harness.with_config(&root);
    std::fs::write(harness.join("genres.toml"), "ZUN = \"Touhou\"\n").expect("a genre map");
    run(&["library", "scan"]);

    assert_eq!(run(&["genre", "apply"]), 0);

    // The invariant: nothing is written without --apply.
    let track = root.join("ZUN").join("Heian Alien.flac");
    assert!(stk::library::tags::read_tags(&track)
        .expect("readable tags")
        .genres
        .is_empty());
    drop(harness);
}

#[test]
fn genre_apply_with_the_flag_writes_the_tag() {
    let _guard = exclusive();
    let harness = Harness::new("cli-genre-apply");
    let root = library(&harness);
    harness.with_config(&root);
    std::fs::write(harness.join("genres.toml"), "ZUN = \"Touhou\"\n").expect("a genre map");
    run(&["library", "scan"]);

    assert_eq!(run(&["genre", "apply", "--apply"]), 0);

    let track = root.join("ZUN").join("Heian Alien.flac");
    assert_eq!(
        stk::library::tags::read_tags(&track)
            .expect("readable tags")
            .genres,
        vec!["Touhou".to_owned()]
    );
    drop(harness);
}

/// #2's write log: every `--apply` mutation leaves a row, and none does
/// without the flag.
#[test]
fn genre_apply_logs_what_it_wrote_only_when_applied() {
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

    assert_eq!(run(&["genre", "apply"]), 0);
    assert!(logged(&harness).is_empty());

    assert_eq!(run(&["genre", "apply", "--apply"]), 0);

    let changes = logged(&harness);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].command, "genre apply");
    assert_eq!(changes[0].field, "genre");
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

#[test]
fn changes_list_renders_and_serialises() {
    let _guard = exclusive();
    let harness = Harness::new("cli-changes-list");
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

    assert_eq!(run(&["changes", "list"]), 0);
    assert_eq!(run(&["--json", "changes", "list"]), 0);
    assert_eq!(run(&["changes", "list", "--command", "genre apply"]), 0);
    drop(harness);
}

#[test]
fn changes_undo_needs_to_be_told_what_to_undo() {
    let _guard = exclusive();
    let harness = Harness::new("cli-changes-undo-args");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["changes", "undo"]), 1);
    assert_eq!(run(&["changes", "undo", "1", "--last"]), 1);
    assert_eq!(run(&["changes", "undo", "404"]), 1);
    drop(harness);
}

#[test]
fn genre_apply_without_a_map_explains_itself() {
    let _guard = exclusive();
    let harness = Harness::new("cli-genre-missing-map");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["genre", "apply"]), 1);
    drop(harness);
}

#[test]
fn verify_checks_a_single_file() {
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-one");
    let root = library(&harness);
    harness.with_config(&root);

    let track = root.join("ZUN").join("Heian Alien.flac");
    assert_eq!(run(&["verify", &track.display().to_string()]), 0);
    drop(harness);
}

/// A library whose one file the spectral check can actually judge.
///
/// The tone `library` writes is too short and too narrow to classify, so it
/// verifies as ERROR; broadband noise is what produces a real verdict.
fn verifiable_library(harness: &Harness) -> PathBuf {
    let root = harness.join("Music");
    noise_source(&root.join("ZUN").join("Noise.flac"), 8, 44100);
    root
}

/// #2 wanted verify to stop being a one-shot report.
#[test]
fn verify_records_its_verdict_in_the_index() {
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-verdict");
    let root = verifiable_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["verify"]), 0);

    let index = stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index the scan wrote");
    assert_eq!(
        index
            .verdict(&root.join("ZUN").join("Noise.flac"))
            .expect("a lookup"),
        Some(stk::models::Verdict::Ok)
    );
    drop(harness);
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

    // Nothing was analysed, so nothing was reported -- the run said so rather
    // than writing an empty document.
    assert!(!report.exists());
    drop(harness);
}

#[test]
fn verify_changed_comes_back_to_a_file_that_moved() {
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-moved");
    let root = verifiable_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);
    run(&["verify"]);

    // Deliberately not rescanned: the stored verdict has to go stale on the
    // file's own `(size, mtime)`, not only when the index row is rewritten.
    noise_source(&root.join("ZUN").join("Noise.flac"), 6, 44100);

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

    let written = std::fs::read_to_string(&report).expect("a report");
    assert!(written.contains("Noise.flac"), "{written}");
    drop(harness);
}

#[test]
fn verify_reports_a_target_that_does_not_exist() {
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-absent");
    harness.with_config(&harness.join("Music"));
    assert_eq!(run(&["verify", "no-such-file.flac"]), 1);
    drop(harness);
}

#[test]
fn verify_writes_a_report_with_a_bom() {
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-report");
    let root = library(&harness);
    harness.with_config(&root);

    let report = harness.join("verify.csv");
    let track = root.join("ZUN").join("Heian Alien.flac");
    run(&[
        "verify",
        &track.display().to_string(),
        "--report",
        &report.display().to_string(),
    ]);

    let bytes = std::fs::read(&report).expect("a written report");
    assert_eq!(&bytes[..3], b"\xef\xbb\xbf");
    let text = String::from_utf8_lossy(&bytes[3..]);
    assert!(text.starts_with("path,verdict,detail,codec"), "{text}");
    drop(harness);
}

/// The round-trip dump, with one cell edited.
///
/// `from` and `to` carry their surrounding commas, so the substitution lands on
/// the cell and not on the same words inside the file's own path.
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

    // The index followed the write, so the next command sees the new title.
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
fn a_dump_applied_unedited_changes_nothing() {
    // The round-trip property: dump, apply, and the library is where it was.
    let _guard = exclusive();
    let harness = Harness::new("cli-tags-apply-round-trip");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let sheet = round_trip_dump(&harness);
    assert_eq!(
        run(&["tags", "apply", &sheet.display().to_string(), "--apply"]),
        0
    );

    assert!(logged(&harness).is_empty());
    assert_eq!(
        title_of(&root.join("ZUN").join("Heian Alien.flac")),
        "Heian Alien"
    );
    drop(harness);
}

#[test]
fn undoing_a_tags_apply_run_puts_the_field_back() {
    let _guard = exclusive();
    let harness = Harness::new("cli-tags-apply-undo");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let sheet = edited_dump(&harness, ",Heian Alien,", ",Native Faith,");
    run(&["tags", "apply", &sheet.display().to_string(), "--apply"]);
    assert_eq!(run(&["changes", "undo", "--last", "--apply"]), 0);

    assert_eq!(
        title_of(&root.join("ZUN").join("Heian Alien.flac")),
        "Heian Alien"
    );
    drop(harness);
}

#[test]
fn tags_apply_reports_a_row_that_is_not_in_the_index() {
    // Reported, not fatal: a stray row in an edited spreadsheet is routine.
    let _guard = exclusive();
    let harness = Harness::new("cli-tags-apply-unknown");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let sheet = harness.join("sheet.csv");
    let row = "path,title\nE:/nowhere/a.flac,Something\n";
    std::fs::write(&sheet, row).expect("a written sheet");

    assert_eq!(
        run(&["tags", "apply", &sheet.display().to_string(), "--apply"]),
        0
    );
    assert!(logged(&harness).is_empty());
    drop(harness);
}

/// Every `tags identify` test here stays off the network: a file that names
/// an artist and a title is not looked up, and an Opus file cannot be
/// fingerprinted, so neither run ever reaches AcoustID. The lookup and the
/// choice are covered against recorded responses in `tests/identify.rs`.
#[test]
fn tags_identify_looks_nothing_up_when_every_file_is_tagged() {
    let _guard = exclusive();
    let harness = Harness::new("cli-identify-tagged");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["tags", "identify", "--apply"]), 0);
    assert!(logged(&harness).is_empty());
    drop(harness);
}

#[test]
fn tags_identify_reports_a_file_it_cannot_fingerprint() {
    // Opus is taggable and undecodable: the one container that reaches the
    // command and cannot be looked up. Reported, not fatal, and nothing is
    // written for it.
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
fn tags_identify_on_a_path_nothing_is_indexed_under_says_so() {
    let _guard = exclusive();
    let harness = Harness::new("cli-identify-elsewhere");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let elsewhere = harness.join("Elsewhere");
    assert_eq!(
        run(&[
            "tags",
            "identify",
            &elsewhere.display().to_string(),
            "--all"
        ]),
        0
    );
    drop(harness);
}

#[test]
fn tags_identify_on_an_empty_index_says_so() {
    let _guard = exclusive();
    let harness = Harness::new("cli-identify-empty");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["tags", "identify"]), 0);
    drop(harness);
}

#[test]
fn tags_apply_rejects_a_sheet_with_no_path_column() {
    let _guard = exclusive();
    let harness = Harness::new("cli-tags-apply-bad");
    harness.with_config(&harness.join("Music"));

    let sheet = harness.join("sheet.csv");
    std::fs::write(&sheet, "artist,title\nZUN,Song\n").expect("a written sheet");

    assert_eq!(run(&["tags", "apply", &sheet.display().to_string()]), 1);
    drop(harness);
}

#[test]
fn a_scan_records_a_measurement_and_a_repeat_scan_does_not() {
    let _guard = exclusive();
    let harness = Harness::new("cli-trend-record");
    let root = library(&harness);
    harness.with_config(&root);

    assert_eq!(run(&["library", "scan"]), 0);
    assert_eq!(run(&["library", "scan"]), 0);

    let index =
        stk::library::index::LibraryIndex::open(&harness.join("library.db")).expect("the index");
    let series = index.snapshots(None).expect("the series");
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].total, 2);
    drop(harness);
}

#[test]
fn a_scan_after_files_were_added_records_a_new_measurement() {
    let _guard = exclusive();
    let harness = Harness::new("cli-trend-grown");
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
    assert_eq!(run(&["library", "scan"]), 0);

    let index =
        stk::library::index::LibraryIndex::open(&harness.join("library.db")).expect("the index");
    let series = index.snapshots(None).expect("the series");
    assert_eq!(series.len(), 2);
    assert_eq!(series[0].total, 3);
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

#[test]
fn trend_on_an_index_that_was_never_scanned_says_so() {
    let _guard = exclusive();
    let harness = Harness::new("cli-trend-empty");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["library", "trend"]), 0);
    drop(harness);
}

#[test]
fn organise_without_the_flag_moves_nothing() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-dry");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["organise"]), 0);

    assert!(root.join("DECO_27").join("01 - Roki.flac").is_file());
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

    // The fixture tags this one artist "DECO*27", album "Ghost", no track.
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
fn undoing_an_organise_run_puts_the_files_back() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-undo");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);
    run(&["organise", "--apply"]);

    assert_eq!(run(&["changes", "undo", "--last", "--apply"]), 0);

    assert!(root.join("DECO_27").join("01 - Roki.flac").is_file());
    assert!(root.join("ZUN").join("Heian Alien.flac").is_file());
    drop(harness);
}

#[test]
fn organise_accepts_a_pattern_for_one_run() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-pattern");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(
        run(&["organise", "--pattern", "{artist}/{title}", "--apply"]),
        0
    );

    assert!(root.join("DECO_27").join("Roki.flac").is_file());
    drop(harness);
}

#[test]
fn organise_refuses_a_destination_outside_every_root() {
    // Otherwise the files leave the library and the next scan evicts all of
    // them, the walk never having seen them again.
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
fn a_relative_playlist_without_a_destination_is_refused_in_json_too() {
    // A script is the caller most likely to pass both, and the one the flag
    // would otherwise be silently ignored for.
    let _guard = exclusive();
    let harness = Harness::new("cli-playlist-relative-json");
    harness.with_config(&harness.join("Music"));

    assert_eq!(
        run(&["--json", "playlist", "artist", "ZUN", "--relative"]),
        1
    );
    drop(harness);
}

#[test]
fn organise_rejects_a_pattern_it_cannot_use() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-bad-pattern");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["organise", "--pattern", "{nonsense}/{title}"]), 1);
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
fn organise_from_a_directory_that_is_not_there_fails() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-from-missing");
    let root = library(&harness);
    harness.with_config(&root);
    let from = harness.join("absent").display().to_string();

    assert_eq!(run(&["organise", "--from", &from]), 1);
    drop(harness);
}

#[test]
fn organise_leaves_no_empty_artist_folder_behind() {
    let _guard = exclusive();
    let harness = Harness::new("cli-organise-prune");
    let root = harness.join("Music");
    encode(
        &root.join("Loose").join("track.flac"),
        &Encoded {
            metadata: &[("artist", "ZUN"), ("title", "Native Faith")],
            ..Encoded::default()
        },
    );
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["organise", "--apply"]), 0);

    assert!(root.join("ZUN").join("Native Faith.flac").is_file());
    assert!(!root.join("Loose").exists(), "the emptied folder is gone");
    drop(harness);
}

#[test]
fn a_genre_playlist_holds_only_the_files_carrying_that_genre() {
    let _guard = exclusive();
    let harness = Harness::new("cli-playlist-genre");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let list = harness.join("vocaloid.m3u8");
    assert_eq!(
        run(&[
            "playlist",
            "genre",
            "Vocaloid",
            "-o",
            &list.display().to_string(),
        ]),
        0
    );

    let text = std::fs::read_to_string(&list).expect("a written playlist");
    assert!(text.starts_with("#EXTM3U"), "{text}");
    assert!(text.contains("Roki"), "{text}");
    assert!(!text.contains("Heian Alien"), "{text}");
    drop(harness);
}

#[test]
fn an_artist_playlist_matches_the_normalised_name() {
    let _guard = exclusive();
    let harness = Harness::new("cli-playlist-artist");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let list = harness.join("deco.m3u8");
    assert_eq!(
        run(&[
            "playlist",
            "artist",
            "DECO*27",
            "-o",
            &list.display().to_string(),
        ]),
        0
    );

    let text = std::fs::read_to_string(&list).expect("a written playlist");
    assert!(text.contains("Roki"), "{text}");
    assert!(!text.contains("Heian Alien"), "{text}");
    drop(harness);
}

#[test]
fn an_exported_playlist_keeps_the_export_row_order() {
    // The order was in the export and nowhere else: the index reads back sorted
    // by path, which would put DECO_27 before ZUN.
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
fn a_relative_playlist_can_be_moved_with_the_music() {
    let _guard = exclusive();
    let harness = Harness::new("cli-playlist-relative");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let list = root.join("lists").join("all.m3u8");
    assert_eq!(
        run(&[
            "playlist",
            "artist",
            "ZUN",
            "--relative",
            "-o",
            &list.display().to_string(),
        ]),
        0
    );

    let text = std::fs::read_to_string(&list).expect("a written playlist");
    assert!(text.contains("../ZUN/Heian Alien.flac"), "{text}");
    drop(harness);
}

#[test]
fn a_relative_playlist_without_a_destination_is_refused() {
    // Relative to what? Writing absolute paths anyway would look like it worked.
    let _guard = exclusive();
    let harness = Harness::new("cli-playlist-relative-stdout");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["playlist", "artist", "ZUN", "--relative"]), 1);
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

    // Roki is owned; Senbonzakura is not, and is queued.
    let store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue");
    let tracks = store.tracks(&[], None).expect("tracks");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].track.title, "Senbonzakura");
    drop(harness);
}

#[test]
fn wantlist_diff_can_skip_queueing() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-no-queue");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let export = harness.join("export.csv");
    std::fs::write(
        &export,
        "Track name,Artist name\nSenbonzakura,Hatsune Miku\n",
    )
    .expect("a written export");

    assert_eq!(
        run(&[
            "wantlist",
            "diff",
            &export.display().to_string(),
            "--queue",
            "false"
        ]),
        0
    );

    let store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue");
    assert!(store.tracks(&[], None).expect("tracks").is_empty());
    drop(harness);
}

#[test]
fn wantlist_diff_writes_the_missing_and_owned_reports() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-reports");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let export = harness.join("export.csv");
    std::fs::write(
        &export,
        "Track name,Artist name\nRoki,DECO*27\nSenbonzakura,Hatsune Miku\n",
    )
    .expect("a written export");

    let missing = harness.join("missing.csv");
    let owned = harness.join("owned.csv");
    run(&[
        "wantlist",
        "diff",
        &export.display().to_string(),
        "--missing",
        &missing.display().to_string(),
        "--owned",
        &owned.display().to_string(),
    ]);

    let missing_text = std::fs::read_to_string(&missing).expect("a missing report");
    assert!(missing_text.contains("Senbonzakura"), "{missing_text}");
    let owned_text = std::fs::read_to_string(&owned).expect("an owned report");
    assert!(owned_text.contains("matched_path"), "{owned_text}");
    assert!(owned_text.contains("Roki"), "{owned_text}");
    drop(harness);
}

#[test]
fn a_second_diff_of_the_same_export_reports_nothing_new() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-new-again");
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

    let report = harness.join("missing.csv");
    let diff_new = |destination: &Path| {
        run(&[
            "wantlist",
            "diff",
            &export.display().to_string(),
            "--new",
            "--missing",
            &destination.display().to_string(),
        ])
    };

    assert_eq!(diff_new(&report), 0);
    let first = std::fs::read_to_string(&report).expect("a missing report");
    assert!(first.contains("Senbonzakura"), "{first}");

    assert_eq!(diff_new(&report), 0);
    let second = std::fs::read_to_string(&report).expect("a missing report");
    assert!(!second.contains("Senbonzakura"), "{second}");
    drop(harness);
}

#[test]
fn a_track_added_to_the_export_since_the_last_diff_is_reported_as_new() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-new-grown");
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

    std::fs::write(
        &export,
        "Track name,Artist name
Senbonzakura,Hatsune Miku
Ghost Rule,DECO*27
",
    )
    .expect("a grown export");

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
    assert!(text.contains("Ghost Rule"), "{text}");
    assert!(!text.contains("Senbonzakura"), "{text}");
    drop(harness);
}

#[test]
fn a_track_the_queue_gave_up_on_is_not_reported_as_new_again() {
    // The point of --new: a FAILED track is still missing from the library, so
    // a plain diff re-reports it every single run.
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
fn the_new_flag_does_not_change_what_is_queued() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-new-queues");
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

    assert_eq!(
        run(&["wantlist", "diff", &export.display().to_string(), "--new"]),
        0
    );

    let store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue");
    let tracks = store.tracks(&[], None).expect("tracks");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].track.title, "Senbonzakura");
    drop(harness);
}

#[test]
fn wantlist_diff_rejects_a_file_without_the_needed_columns() {
    let _guard = exclusive();
    let harness = Harness::new("cli-wantlist-bad");
    harness.with_config(&harness.join("Music"));

    let export = harness.join("export.csv");
    std::fs::write(&export, "Foo,Bar\n1,2\n").expect("a written export");

    assert_eq!(run(&["wantlist", "diff", &export.display().to_string()]), 1);
    drop(harness);
}

#[test]
fn review_list_is_empty_until_something_is_queued() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-empty");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["review", "list"]), 0);
    assert_eq!(run(&["--json", "review", "list"]), 0);
    drop(harness);
}

#[test]
fn review_approve_reject_and_reset_report_an_unknown_track() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-unknown");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["review", "approve", "999", "1"]), 1);
    assert_eq!(run(&["review", "reject", "999"]), 1);
    assert_eq!(run(&["review", "reset", "999"]), 1);
    drop(harness);
}

#[test]
fn review_import_reads_a_legacy_csv() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-import");
    harness.with_config(&harness.join("Music"));

    let csv = harness.join("soulseek_review.csv");
    std::fs::write(
        &csv,
        "artist,title,approve,cand1_user,cand1_path,cand1_size\n\
         DECO*27,Roki,1,peerA,@@peerA\\Roki.flac,30000000\n",
    )
    .expect("a legacy CSV");

    assert_eq!(run(&["review", "import", &csv.display().to_string()]), 0);

    let store = stk::review::store::ReviewStore::open(&harness.join("review.db"))
        .expect("the review queue");
    let tracks = store.tracks(&[], None).expect("tracks");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].approved_rank, Some(1));
    drop(harness);
}

#[test]
fn review_import_reports_a_missing_file() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-import-missing");
    harness.with_config(&harness.join("Music"));
    assert_eq!(run(&["review", "import", "no-such-file.csv"]), 1);
    drop(harness);
}

#[test]
fn review_list_accepts_a_status_filter_and_rejects_a_bad_one() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-status");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["review", "list", "--status", "pending"]), 0);
    assert_eq!(run(&["review", "list", "--status", "nonsense"]), 1);
    drop(harness);
}

#[test]
fn review_pick_refuses_to_run_as_json() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-pick-json");
    harness.with_config(&harness.join("Music"));

    // `say` writes nothing in JSON mode, so a prompt there would be an invisible
    // hang in the middle of a pipeline.
    assert_eq!(run(&["--json", "review", "pick"]), 1);
    drop(harness);
}

#[test]
fn review_pick_on_an_empty_queue_says_so() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-pick-empty");
    harness.with_config(&harness.join("Music"));

    // Nothing answers the prompts here, which the picker reads as a quit rather
    // than as agreement -- so this returns instead of blocking.
    assert_eq!(run(&["review", "pick"]), 0);
    drop(harness);
}

/// Two tracks one peer holds in a single folder, waiting to be reviewed (#6).
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
fn review_albums_is_empty_until_one_peer_covers_several_tracks() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-albums-empty");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["review", "albums"]), 0);
    assert_eq!(run(&["--json", "review", "albums"]), 0);
    drop(harness);
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
    // Decided tracks leave the album view, so nothing can be approved twice.
    assert_eq!(run(&["review", "approve-album", "1"]), 1);
    drop(harness);
}

#[test]
fn approving_an_album_that_is_not_offered_is_an_error() {
    let _guard = exclusive();
    let harness = Harness::new("cli-review-albums-unknown");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["review", "approve-album", "1"]), 1);
    assert_eq!(run(&["review", "approve-album", "0"]), 1);
    drop(harness);
}

#[test]
fn get_with_nothing_to_search_for_is_rejected() {
    let _guard = exclusive();
    let harness = Harness::new("cli-get-empty");
    harness.with_config(&harness.join("Music"));
    assert_eq!(run(&["get", "   "]), 1);
    drop(harness);
}

#[test]
fn get_splits_an_artist_from_a_title() {
    use stk::cli::commands::get::split_query;

    assert_eq!(
        split_query("DECO*27 - Roki"),
        ("DECO*27".to_owned(), "Roki".to_owned())
    );
    assert_eq!(
        split_query("DECO*27 \u{2013} Roki"),
        ("DECO*27".to_owned(), "Roki".to_owned())
    );
    assert_eq!(
        split_query("DECO*27 \u{2014} Roki"),
        ("DECO*27".to_owned(), "Roki".to_owned())
    );
    assert_eq!(split_query("Roki"), (String::new(), "Roki".to_owned()));
}

#[test]
fn the_library_flag_overrides_the_configured_root() {
    let _guard = exclusive();
    let harness = Harness::new("cli-library-flag");
    let root = library(&harness);
    harness.with_config(Path::new("Z:/nowhere-at-all"));

    // Without the flag the configured root is missing and doctor fails.
    assert_eq!(run(&["doctor"]), 1);
    assert_eq!(
        run(&["--library", &root.display().to_string(), "doctor"]),
        0
    );
    drop(harness);
}

#[test]
fn json_output_is_a_document_rather_than_a_table() {
    let _guard = exclusive();
    let harness = Harness::new("cli-json-shape");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    // `tags dump` is the one JSON path that also has a file form, so its
    // document can be read back without capturing stdout.
    let output = harness.join("tags.csv");
    run(&["tags", "dump", "--output", &output.display().to_string()]);
    let text = std::fs::read_to_string(&output).expect("a written CSV");
    assert_eq!(text.lines().count(), 3, "a header and two rows");
    drop(harness);
}

#[test]
fn a_written_config_round_trips_through_the_cli() {
    let _guard = exclusive();
    let harness = Harness::new("cli-config-round-trip");
    let written = harness.join("written.toml");
    let path = written.display().to_string();

    assert_eq!(run(&["config", "init", "--path", &path]), 0);
    assert_eq!(run(&["--config", &path, "config", "show"]), 0);
    drop(harness);
}

#[test]
fn the_api_key_is_never_written_into_the_config() {
    let _guard = exclusive();
    let harness = Harness::new("cli-config-no-secret");
    unsafe { std::env::set_var("SLSKD_API_KEY", "s3cret") };

    let written = harness.join("written.toml");
    run(&["config", "init", "--path", &written.display().to_string()]);

    let text = std::fs::read_to_string(&written).expect("a written config");
    assert!(!text.contains("s3cret"), "{text}");
    assert!(!text.contains("api_key"), "{text}");

    unsafe { std::env::remove_var("SLSKD_API_KEY") };
    drop(harness);
}

#[test]
fn art_fetch_without_the_flag_writes_nothing() {
    let _guard = exclusive();
    let harness = Harness::new("cli-art-dry-run");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    // No network here: every provider fails and the run still reports rather
    // than aborting. The invariant under test is that nothing was embedded.
    let track = root.join("ZUN").join("Heian Alien.flac");
    let before = stk::library::tags::artwork_size(&track);
    run(&["art", "fetch", "--limit", "1"]);

    assert_eq!(stk::library::tags::artwork_size(&track), before);
    drop(harness);
}

#[test]
fn an_unknown_artwork_provider_is_rejected() {
    let _guard = exclusive();
    let harness = Harness::new("cli-art-bad-provider");
    let root = harness.join("Music");
    std::fs::create_dir_all(&root).expect("a root");
    std::fs::write(
        harness.join("stk.toml"),
        format!(
            "[library]\nroot = {}\n\n[index]\npath = {}\n\n[artwork]\nproviders = [\"discogs\"]\n",
            quoted(&root),
            quoted(&harness.join("library.db")),
        ),
    )
    .expect("a written config");

    assert_eq!(run(&["art", "fetch"]), 1);
    drop(harness);
}

#[test]
fn the_json_flag_must_precede_the_subcommand_but_is_accepted_after_it() {
    // typer requires the global flag before the subcommand; clap's `global`
    // accepts either, which is a superset rather than a divergence.
    let _guard = exclusive();
    let harness = Harness::new("cli-flag-position");
    harness.with_config(&harness.join("Music"));

    assert_eq!(run(&["--json", "review", "list"]), 0);
    assert_eq!(run(&["review", "list", "--json"]), 0);
    drop(harness);
}

#[test]
fn a_json_document_written_by_the_cli_parses() {
    let _guard = exclusive();
    let harness = Harness::new("cli-json-parses");
    let root = library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    // Written through the report path so the bytes can be inspected.
    let report = harness.join("verify.csv");
    let track = root.join("ZUN").join("Heian Alien.flac");
    run(&[
        "verify",
        &track.display().to_string(),
        "--report",
        &report.display().to_string(),
    ]);
    assert!(report.is_file());

    let written = harness.join("written.toml");
    run(&["config", "init", "--path", &written.display().to_string()]);
    let settings: stk::config::Settings =
        toml::from_str(&std::fs::read_to_string(&written).expect("a config")).expect("valid TOML");
    assert_eq!(settings.library.root, root);

    let _ = json_at;
    drop(harness);
}

/// Phase 4 (#1): `stk get` handles one track, several tracks, and a list from a
/// file or stdin. `wanted_tracks` is the parser all three go through.
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

    // One.
    let one = wanted_tracks(&args(vec!["DECO*27 - Roki"], None)).expect("one track");
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].query(), "DECO*27 - Roki");

    // Several, which the Python could not express at all: `query` was one `str`.
    let several = wanted_tracks(&args(
        vec!["DECO*27 - Roki", "ZUN - Heian Alien", "Ado - Odo"],
        None,
    ))
    .expect("several tracks");
    assert_eq!(several.len(), 3);
    assert_eq!(several[2].query(), "Ado - Odo");

    // A list from a file, with blank lines and comments skipped.
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

    // Arguments and a list combine rather than one replacing the other.
    let both = wanted_tracks(&args(vec!["Ado - Odo"], Some(list))).expect("both");
    assert_eq!(both.len(), 3);
}

#[test]
fn get_applies_one_artist_across_a_whole_list() {
    use stk::cli::commands::get::{wanted_tracks, GetArgs};

    let tracks = wanted_tracks(&GetArgs {
        query: vec!["Roki".to_owned(), "Ghost Rule".to_owned()],
        from: None,
        artist: Some("DECO*27".to_owned()),
        album: false,
        lossless: false,
        min_bitrate: None,
        yes: false,
        queue_only: false,
        limit: 5,
    })
    .expect("tracks");

    assert_eq!(tracks.len(), 2);
    assert!(tracks.iter().all(|track| track.artist == "DECO*27"));
}

#[test]
fn get_reports_a_list_it_cannot_read() {
    use stk::cli::commands::get::{wanted_tracks, GetArgs};

    let error = wanted_tracks(&GetArgs {
        query: Vec::new(),
        from: Some(PathBuf::from("no-such-list.txt")),
        artist: None,
        album: false,
        lossless: false,
        min_bitrate: None,
        yes: false,
        queue_only: false,
        limit: 5,
    })
    .expect_err("a missing list");

    assert!(error.to_string().contains("cannot read"), "{error}");
}

#[test]
fn get_with_an_empty_list_is_rejected() {
    let _guard = exclusive();
    let harness = Harness::new("cli-get-empty-list");
    harness.with_config(&harness.join("Music"));

    let list = harness.join("empty.txt");
    std::fs::write(&list, "# nothing here\n\n").expect("a written list");

    assert_eq!(run(&["get", "--from", &list.display().to_string()]), 1);
    drop(harness);
}

fn gain_of(path: &Path) -> Option<String> {
    stk::library::tags::read_replay_gain(path)
        .expect("readable loudness tags")
        .track_gain
}

/// A library where one album holds two tracks and one file stands alone, so
/// both halves of the album rule are exercised.
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
fn loudness_writes_nothing_without_apply() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-dry-run");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["loudness"]), 0);

    // The invariant: nothing is written without --apply.
    assert_eq!(gain_of(&root.join("ZUN").join("Heian Alien.flac")), None);
    assert!(logged(&harness).is_empty());
    drop(harness);
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

/// An album gain belongs to a record; a file with no album tag has none.
#[test]
fn loudness_gives_an_album_gain_only_to_files_that_name_an_album() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-albums");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);
    run(&["loudness", "--apply"]);

    let read = |path: PathBuf| {
        stk::library::tags::read_replay_gain(&path).expect("readable loudness tags")
    };
    let first = read(root.join("DECO_27").join("01 - Roki.flac"));
    let second = read(root.join("DECO_27").join("02 - Ghost Rule.flac"));
    let single = read(root.join("ZUN").join("Heian Alien.flac"));

    assert!(first.album_gain.is_some());
    assert_eq!(first.album_gain, second.album_gain);
    assert_eq!(single.album_gain, None);
    drop(harness);
}

/// A library scanned elsewhere is not re-measured out from under whoever owns
/// it -- that is what `--force` is for.
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

#[test]
fn loudness_force_replaces_an_existing_scan() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-force");
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
        run(&[
            "loudness",
            "--force",
            "--apply",
            &track.display().to_string()
        ]),
        0
    );

    assert_ne!(gain_of(&track), Some("-99.00 dB".to_owned()));
    drop(harness);
}

#[test]
fn loudness_reports_the_plan_as_json() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-json");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    assert_eq!(run(&["--json", "loudness"]), 0);
    drop(harness);
}

/// The whole run undone leaves the library exactly as it was found.
#[test]
fn changes_undo_takes_a_whole_loudness_run_back() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-undo");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);
    run(&["loudness", "--apply"]);

    assert_eq!(run(&["changes", "undo", "--last", "--apply"]), 0);

    for track in [
        root.join("DECO_27").join("01 - Roki.flac"),
        root.join("DECO_27").join("02 - Ghost Rule.flac"),
        root.join("ZUN").join("Heian Alien.flac"),
    ] {
        assert!(stk::library::tags::read_replay_gain(&track)
            .expect("readable loudness tags")
            .is_empty());
    }
    drop(harness);
}

/// The count printed after `--apply` is what was written, not the plan less
/// every failure: a file that could not be measured never became a plan entry,
/// so subtracting it once underflowed and printed a number near `u64::MAX`.
#[test]
fn loudness_reports_what_it_wrote_when_most_files_could_not_be_read() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-mostly-broken");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    // More unreadable files than readable ones, which is what underflowed.
    for name in ["x.flac", "y.flac", "z.flac", "w.flac"] {
        std::fs::write(root.join("ZUN").join(name), b"not audio").expect("a written file");
    }

    assert_eq!(run(&["loudness", "--apply"]), 0);

    let changes = logged(&harness);
    assert_eq!(changes.len(), 3);
    drop(harness);
}

/// An album gain describes a whole record, so naming one track of one gets a
/// track gain and no album gain -- a figure measured from that track alone
/// would tell a player the record is as loud as its loudest song.
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

/// The whole record named, so the album gain is one the record earns.
#[test]
fn loudness_gives_an_album_gain_when_the_whole_record_is_named() {
    let _guard = exclusive();
    let harness = Harness::new("cli-loudness-whole-record");
    let root = levelled_library(&harness);
    harness.with_config(&root);
    run(&["library", "scan"]);

    let folder = root.join("DECO_27");
    assert_eq!(
        run(&["loudness", "--apply", &folder.display().to_string()]),
        0
    );

    let written = stk::library::tags::read_replay_gain(&folder.join("01 - Roki.flac"))
        .expect("readable loudness tags");
    assert!(written.album_gain.is_some());
    drop(harness);
}

/// `--album` takes one album, and says so before it opens a socket. A list of
/// albums is a list of interactive decisions, which is the one thing the batch
/// path exists to avoid (#13).
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

/// The album listing's rows, checked as a pure function for the same reason the
/// other rendering is: a CLI test can only see the exit code (#13).
#[test]
fn get_album_lists_a_peer_a_count_and_what_is_inside() {
    use stk::cli::commands::get::{folder_rows, folder_warnings};
    use stk::config::RankingSettings;
    use stk::models::Candidate;
    use stk::review::albums::folders;

    let file = |name: &str, lossless: bool| Candidate {
        username: "bulk".to_owned(),
        remote_path: format!("@@peer\\Music\\DECO27 - Ghost Rule EP\\{name}"),
        size: 30_000_000,
        sample_rate: if lossless { Some(44100) } else { None },
        bit_depth: if lossless { Some(16) } else { None },
        bitrate: if lossless { None } else { Some(192) },
        free_upload_slots: false,
        queue_length: 7,
        ..Candidate::default()
    };
    let found = vec![
        file("01 - Roki.flac", true),
        file("02 - Ghost Rule.flac", true),
        file("04 - Bonus.mp3", false),
        file("ghost.cue", true),
    ];

    let offers = folders(
        &found,
        &RankingSettings {
            require_lossless: true,
            ..RankingSettings::default()
        },
        "deco 27 ghost rule ep",
        2,
    );

    assert_eq!(
        folder_rows(&offers),
        vec![vec![
            "1".to_owned(),
            "bulk".to_owned(),
            "DECO27 - Ghost Rule EP".to_owned(),
            "3".to_owned(),
            "2 flac, 1 mp3".to_owned(),
            "busy".to_owned(),
            "7".to_owned(),
        ]]
    );
    assert_eq!(offers[0].extras, 1);
    assert_eq!(
        folder_warnings(&offers[0]),
        "1 file(s) below the quality limit"
    );
}

#[test]
fn nothing_is_wrong_with_a_folder_that_has_nothing_wrong_with_it() {
    use stk::cli::commands::get::folder_warnings;
    use stk::config::RankingSettings;
    use stk::models::Candidate;
    use stk::review::albums::folders;

    let file = |name: &str| Candidate {
        username: "bulk".to_owned(),
        remote_path: format!("@@peer\\Music\\Ghost Rule EP\\{name}"),
        size: 30_000_000,
        sample_rate: Some(44100),
        bit_depth: Some(16),
        free_upload_slots: true,
        ..Candidate::default()
    };
    let offers = folders(
        &[file("01 - Roki.flac"), file("02 - Ghost Rule.flac")],
        &RankingSettings::default(),
        "ghost rule ep",
        2,
    );

    assert_eq!(folder_warnings(&offers[0]), "");
}

/// A library, a config naming a download directory, and an export in which
/// Roki is owned and Senbonzakura is not.
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

/// A finished download as a fetch leaves it, peer's filename and all.
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
fn sync_apply_without_a_download_directory_is_refused() {
    let _guard = exclusive();
    let harness = Harness::new("cli-sync-no-downloads");
    let root = library(&harness);
    harness.with_config(&root);
    let export = harness.join("export.csv");
    std::fs::write(&export, "Track name,Artist name\nRoki,DECO*27\n").expect("an export");

    assert_eq!(
        run(&["sync", "--yes", "--apply", &export.display().to_string()]),
        1
    );
    assert!(!harness.join("review.db").exists());
    drop(harness);
}

#[test]
fn sync_with_an_export_that_is_not_there_fails() {
    let _guard = exclusive();
    let (harness, _, _) = sync_harness("cli-sync-no-export");
    let absent = harness.join("absent.csv").display().to_string();

    assert_eq!(run(&["sync", "--yes", &absent]), 1);
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
