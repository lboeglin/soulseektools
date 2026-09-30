//! Loading, layering, validating and writing the configuration.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use stk::config::{
    find_config_file, load_settings, render_toml, write_config, Overrides, CONFIG_FILENAME,
};
use support::Scratch;

const SECRETS: &[&str] = &["SLSKD_API_KEY", "DISCOGS_TOKEN", "LASTFM_API_KEY"];

fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

struct CleanEnvironment {
    saved: Vec<(String, String)>,
    directory: PathBuf,
}

impl CleanEnvironment {
    fn enter(at: &Path) -> Self {
        let saved: Vec<(String, String)> = std::env::vars()
            .filter(|(name, _)| name.starts_with("STK_") || SECRETS.contains(&name.as_str()))
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
            .filter(|(name, _)| name.starts_with("STK_") || SECRETS.contains(&name.as_str()))
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
fn finds_the_config_in_a_parent_directory() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-parent");
    let _environment = CleanEnvironment::enter(scratch.path());
    write_toml(&scratch.join(CONFIG_FILENAME), "");
    let nested = scratch.join("a").join("b");
    std::fs::create_dir_all(&nested).expect("a nested directory");

    let found = find_config_file(Some(&nested)).expect("a config file");
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
        "[library]\nroot = \"E:/Music\"\n\n[matching]\nartist = 0.75\n\n\
         [wantlist]\nrequest_timeout_seconds = 5\ndiscogs_cache = \"releases.json\"\n\n\
         [dupes]\nresolve_min_score = 0.99\n\n[fetch]\ntags = \"fill\"\n",
    );

    let resolved = load(Some(&path));
    let settings = &resolved.settings;

    assert_eq!(settings.library.root, PathBuf::from("E:/Music"));
    assert_eq!(
        settings.library.all_roots(),
        vec![PathBuf::from("E:/Music")]
    );
    assert!((settings.matching.artist - 0.75).abs() < 1e-9);
    assert!((settings.wantlist.request_timeout_seconds - 5.0).abs() < f64::EPSILON);
    assert!(settings.wantlist.discogs_cache.ends_with("releases.json"));
    assert!((settings.dupes.resolve_min_score - 0.99).abs() < f64::EPSILON);
    assert_eq!(settings.fetch.tags, stk::retag::TagPolicy::Fill);
    assert_eq!(resolved.source_path.as_deref(), Some(path.as_path()));
}

#[test]
fn an_invalid_value_is_rejected_naming_the_key() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-invalid");
    let _environment = CleanEnvironment::enter(scratch.path());

    for (body, named) in [
        ("[matching]\nartist = 4.2\n", "matching"),
        (
            "[review]\nalbum_minimum_tracks = 1\n",
            "album_minimum_tracks",
        ),
        ("[identify]\nmin_score = 1.5\n", "identify.min_score"),
        (
            "[wantlist]\nrequest_timeout_seconds = 0\n",
            "wantlist.request_timeout_seconds",
        ),
        (
            "[dupes]\nresolve_min_score = 1.5\n",
            "dupes.resolve_min_score",
        ),
        (
            "[uploader]\nscore_window = 5000000\n",
            "uploader.score_window",
        ),
        (
            "[organise]\npattern = \"{nonsense}/{title}\"\n",
            "{nonsense}",
        ),
        ("[organise]\npattern = \"../{title}\"\n", "organise"),
        ("[fetch]\ntags = \"overwrite\"\n", "overwrite"),
        (
            "[slskd]\nsearch_interval_seconds = -1\n",
            "slskd.search_interval_seconds",
        ),
    ] {
        let path = write_toml(&scratch.join(CONFIG_FILENAME), body);
        let error = load_settings(Some(&path), &Overrides::default()).expect_err(body);
        assert!(error.to_string().contains(named), "{body}: {error}");
    }
}

#[test]
fn secrets_are_read_from_the_environment_and_never_rendered() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-secrets");
    let environment = CleanEnvironment::enter(scratch.path());
    let empty = load(None).settings;
    assert_eq!(empty.slskd.api_key, None);
    assert_eq!(empty.wantlist.discogs_token, None);
    assert_eq!(empty.wantlist.lastfm_api_key, None);

    environment.set("SLSKD_API_KEY", "s3cret-slskd");
    environment.set("DISCOGS_TOKEN", "s3cret-discogs");
    environment.set("LASTFM_API_KEY", "s3cret-lastfm");
    let settings = load(None).settings;

    assert_eq!(settings.slskd.api_key.as_deref(), Some("s3cret-slskd"));
    assert_eq!(
        settings.wantlist.discogs_token.as_deref(),
        Some("s3cret-discogs")
    );
    assert_eq!(
        settings.wantlist.lastfm_api_key.as_deref(),
        Some("s3cret-lastfm")
    );
    let rendered = render_toml(&settings).expect("rendered config");
    for leaked in ["s3cret", "api_key", "token"] {
        assert!(!rendered.contains(leaked), "{leaked} in {rendered}");
    }
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

    let flag = std::env::current_dir()
        .expect("a working directory")
        .join("from-flag");
    assert_eq!(resolved.settings.library.root, flag);
    assert_eq!(resolved.settings.library.all_roots(), vec![flag]);
}

#[test]
fn every_relative_path_resolves_against_the_config_file_not_the_working_directory() {
    let _guard = exclusive();
    let scratch = Scratch::new("config-relative-paths");
    let _environment = CleanEnvironment::enter(scratch.path());
    let directory = scratch.join("config");
    std::fs::create_dir_all(&directory).expect("a config directory");
    let path = write_toml(
        &directory.join(CONFIG_FILENAME),
        "[index]
path = \"library.db\"
[review]
path = \"review.db\"
         [slskd]
download_directory = \"downloads\"
         [wantlist]
discogs_cache = \"discogs.json\"
",
    );

    let settings = load(Some(&path)).settings;
    assert_eq!(settings.index.path, directory.join("library.db"));
    assert_eq!(settings.review.path, directory.join("review.db"));
    assert_eq!(
        settings.slskd.download_directory,
        Some(directory.join("downloads"))
    );
    assert_eq!(
        settings.wantlist.discogs_cache,
        directory.join("discogs.json")
    );
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
