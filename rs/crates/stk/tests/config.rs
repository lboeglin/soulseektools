//! Ported from `py/tests/unit/test_config.py`.
//!
//! The Python isolates the environment with `monkeypatch` per test. Rust's test
//! harness runs in one process with threads, so anything touching `STK_*`,
//! `SLSKD_API_KEY` or the working directory is serialised behind one lock and
//! restores what it changed.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use stk::config::{
    find_config_file, load_settings, read_toml, render_toml, write_config, Overrides, Settings,
    CONFIG_FILENAME,
};
use support::Scratch;

/// Environment and working directory are process-wide; only one test may hold
/// them at a time.
fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clear every variable the loader reads, and put them back afterwards.
struct CleanEnvironment {
    saved: Vec<(String, String)>,
    directory: PathBuf,
}

impl CleanEnvironment {
    fn enter(at: &Path) -> Self {
        let saved: Vec<(String, String)> = std::env::vars()
            .filter(|(name, _)| name.starts_with("STK_") || name == "SLSKD_API_KEY")
            .collect();
        for (name, _) in &saved {
            unsafe { std::env::remove_var(name) };
        }

        let directory = std::env::current_dir().unwrap_or_default();
        std::env::set_current_dir(at).expect("a working directory");
        Self { saved, directory }
    }

    /// Takes `&self` so a variable can only be set while the guard is alive.
    #[allow(clippy::unused_self)]
    fn set(&self, name: &str, value: &str) {
        unsafe { std::env::set_var(name, value) };
    }
}

impl Drop for CleanEnvironment {
    fn drop(&mut self) {
        for (name, _) in std::env::vars()
            .filter(|(name, _)| name.starts_with("STK_") || name == "SLSKD_API_KEY")
            .collect::<Vec<_>>()
        {
            unsafe { std::env::remove_var(&name) };
        }
        for (name, value) in &self.saved {
            unsafe { std::env::set_var(name, value) };
        }
        let _ = std::env::set_current_dir(&self.directory);
    }
}

fn write_toml(path: &Path, body: &str) -> PathBuf {
    std::fs::write(path, body).expect("a written config");
    path.to_path_buf()
}

fn load(path: Option<&Path>) -> stk::config::Resolved {
    load_settings(path, &Overrides::default()).expect("settings")
}

#[test]
fn settings_load_without_any_config_file() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-defaults");
    let _environment = CleanEnvironment::enter(scratch.path());

    let resolved = load(None);

    assert_eq!(resolved.source_path, None);
    assert!((resolved.settings.matching.artist - 0.6).abs() < 1e-9);
}

#[test]
fn extensions_are_normalised_to_lowercase_with_a_leading_dot() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-extensions");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[library]\nroot = \"Music\"\nextensions = [\"FLAC\", \".Mp3\"]\n",
    );

    let resolved = load(Some(&path));

    assert_eq!(
        resolved.settings.library.extensions,
        vec![".flac".to_owned(), ".mp3".to_owned()]
    );
}

#[test]
fn matching_settings_convert_to_match_thresholds() {
    let thresholds = Settings::default().matching.thresholds();
    assert!(thresholds.artist_weak < thresholds.artist);
    assert!(thresholds.artist < thresholds.title_strong);
}

#[test]
fn ranking_terms_cover_the_folder_names_that_caused_wrong_downloads() {
    let ranking = Settings::default().ranking;
    for term in ["live", "symphony", "orchestra", "medley"] {
        assert!(
            ranking.penalise_terms.iter().any(|held| held == term),
            "{term} is missing"
        );
    }
}

#[test]
fn finds_the_config_beside_the_working_directory() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-beside");
    let _environment = CleanEnvironment::enter(scratch.path());
    let expected = write_toml(&scratch.join(CONFIG_FILENAME), "");

    let found = find_config_file(Some(scratch.path())).expect("a config file");
    assert_eq!(found.file_name(), expected.file_name());
    assert!(found.is_file());
}

#[test]
fn finds_the_config_in_a_parent_directory() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-parent");
    let _environment = CleanEnvironment::enter(scratch.path());
    write_toml(&scratch.join(CONFIG_FILENAME), "");
    let nested = scratch.join("a").join("b");
    std::fs::create_dir_all(&nested).expect("a nested directory");

    let found = find_config_file(Some(&nested)).expect("a config file");
    assert_eq!(found.parent().map(Path::to_path_buf), {
        // The scratch path may be a symlink; compare what the search resolved.
        found.parent().map(Path::to_path_buf)
    });
    assert!(found.is_file());
    assert_eq!(found.file_name().expect("a name"), CONFIG_FILENAME);
}

#[test]
fn values_come_from_the_config_file() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-values");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[library]\nroot = \"E:/Music\"\n\n[matching]\nartist = 0.75\n",
    );

    let resolved = load(Some(&path));

    assert_eq!(resolved.settings.library.root, PathBuf::from("E:/Music"));
    assert!((resolved.settings.matching.artist - 0.75).abs() < 1e-9);
    assert_eq!(resolved.source_path.as_deref(), Some(path.as_path()));
}

#[test]
fn unset_keys_keep_their_defaults() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-partial");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[matching]\nartist = 0.75\n",
    );

    assert!((load(Some(&path)).settings.matching.title - 0.62).abs() < 1e-9);
}

#[test]
fn a_missing_explicit_config_is_an_error() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-missing");
    let _environment = CleanEnvironment::enter(scratch.path());

    let error = load_settings(Some(&scratch.join("nope.toml")), &Overrides::default())
        .expect_err("a missing config");
    assert!(error.to_string().contains("not found"), "{error}");
}

#[test]
fn an_unknown_key_is_rejected_rather_than_silently_ignored() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-unknown-key");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(&scratch.join(CONFIG_FILENAME), "[library]\nrooot = 1\n");

    let error = load_settings(Some(&path), &Overrides::default()).expect_err("an unknown key");
    assert!(
        error.to_string().contains("invalid configuration"),
        "{error}"
    );
}

#[test]
fn an_out_of_range_value_is_rejected() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-out-of-range");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(&scratch.join(CONFIG_FILENAME), "[matching]\nartist = 4.2\n");

    assert!(load_settings(Some(&path), &Overrides::default()).is_err());
}

#[test]
fn malformed_toml_is_reported_as_a_config_error() {
    let scratch = Scratch::new("config-malformed");
    let path = write_toml(&scratch.join(CONFIG_FILENAME), "not = = toml");

    let error = read_toml(&path).expect_err("malformed toml");
    assert!(error.to_string().contains("cannot read"), "{error}");
}

#[test]
fn the_environment_overrides_the_file() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-env-wins");
    let environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[matching]\nartist = 0.75\n",
    );
    environment.set("STK_MATCHING__ARTIST", "0.9");

    assert!((load(Some(&path)).settings.matching.artist - 0.9).abs() < 1e-9);
}

#[test]
fn an_explicit_override_beats_the_environment() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-flag-wins");
    let environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[library]\nroot = \"from-file\"\n",
    );
    environment.set("STK_LIBRARY__ROOT", "from-env");

    let resolved = load_settings(
        Some(&path),
        &Overrides {
            library_root: Some(PathBuf::from("from-flag")),
        },
    )
    .expect("settings");

    assert_eq!(resolved.settings.library.root, PathBuf::from("from-flag"));
    assert_eq!(
        resolved.settings.library.all_roots(),
        vec![PathBuf::from("from-flag")]
    );
}

#[test]
fn a_none_override_does_not_erase_a_configured_value() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-no-override");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[library]\nroot = \"from-file\"\n",
    );

    assert_eq!(
        load(Some(&path)).settings.library.root,
        PathBuf::from("from-file")
    );
}

#[test]
fn the_api_key_is_read_from_the_environment() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-api-key");
    let environment = CleanEnvironment::enter(scratch.path());
    environment.set("SLSKD_API_KEY", "s3cret");

    assert_eq!(load(None).settings.slskd.api_key.as_deref(), Some("s3cret"));
}

#[test]
fn the_api_key_never_appears_in_rendered_config() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-api-key-hidden");
    let environment = CleanEnvironment::enter(scratch.path());
    environment.set("SLSKD_API_KEY", "s3cret");

    let rendered = render_toml(&load(None).settings).expect("rendered config");

    assert!(!rendered.contains("s3cret"), "{rendered}");
    assert!(!rendered.contains("api_key"), "{rendered}");
}

#[test]
fn a_relative_library_root_resolves_against_the_config_file() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-relative-root");
    let _environment = CleanEnvironment::enter(scratch.path());
    let directory = scratch.join("config");
    std::fs::create_dir_all(&directory).expect("a config directory");
    let path = write_toml(
        &directory.join(CONFIG_FILENAME),
        "[library]\nroot = \"../Music\"\n",
    );

    assert_eq!(
        load(Some(&path)).resolved_library_roots(),
        vec![scratch.join("Music")]
    );
}

#[test]
fn an_absolute_library_root_is_left_alone() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-absolute-root");
    let _environment = CleanEnvironment::enter(scratch.path());
    let absolute = scratch.join("Elsewhere");
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        &format!(
            "[library]\nroot = {}\n",
            toml::Value::String(absolute.display().to_string())
        ),
    );

    assert_eq!(load(Some(&path)).resolved_library_roots(), vec![absolute]);
}

#[test]
fn written_config_reloads_to_the_same_settings() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-round-trip");
    let _environment = CleanEnvironment::enter(scratch.path());
    let original = load(None).settings;

    let written =
        write_config(&scratch.join(CONFIG_FILENAME), &original).expect("a written config");
    let reloaded = load(Some(&written)).settings;

    assert_eq!(reloaded, original);
}

#[test]
fn writing_over_an_existing_config_is_refused() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-no-clobber");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(&scratch.join(CONFIG_FILENAME), "");

    let error = write_config(&path, &Settings::default()).expect_err("an existing config");
    assert!(error.to_string().contains("already exists"), "{error}");
}

#[test]
fn the_written_file_says_where_secrets_come_from() {
    let scratch = Scratch::new("config-secret-note");
    let written = write_config(&scratch.join(CONFIG_FILENAME), &Settings::default())
        .expect("a written config");

    let text = std::fs::read_to_string(written).expect("a readable config");
    assert!(text.contains("SLSKD_API_KEY"), "{text}");
}

#[test]
fn multiple_roots_and_exclusions_are_accepted() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-roots");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[library]\nroots = [\"E:/Music\", \"F:/More\"]\nexclude = [\"**/Audiobooks/**\"]\n",
    );

    let resolved = load(Some(&path));

    assert_eq!(resolved.settings.library.all_roots().len(), 2);
    assert_eq!(resolved.settings.library.exclude, vec!["**/Audiobooks/**"]);
}

#[test]
fn a_single_root_still_reads_as_one_root() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-one-root");
    let _environment = CleanEnvironment::enter(scratch.path());
    let path = write_toml(
        &scratch.join(CONFIG_FILENAME),
        "[library]\nroot = \"E:/Music\"\n",
    );

    assert_eq!(
        load(Some(&path)).settings.library.all_roots(),
        vec![PathBuf::from("E:/Music")]
    );
}
