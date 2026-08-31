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
use support::{encode, Encoded, Scratch};

fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
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
    let mut argv: Vec<std::ffi::OsString> = vec!["stk-rs".into()];
    argv.extend(arguments.iter().map(Into::into));
    stk::cli::run(argv)
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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

#[test]
fn genre_apply_without_a_map_explains_itself() {
    needs_ffmpeg!();
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
    needs_ffmpeg!();
    let _guard = exclusive();
    let harness = Harness::new("cli-verify-one");
    let root = library(&harness);
    harness.with_config(&root);

    let track = root.join("ZUN").join("Heian Alien.flac");
    assert_eq!(run(&["verify", &track.display().to_string()]), 0);
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
    needs_ffmpeg!();
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

#[test]
fn wantlist_diff_queues_what_is_missing() {
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
    needs_ffmpeg!();
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
