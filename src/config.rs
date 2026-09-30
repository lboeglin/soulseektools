//! Layered configuration: CLI flag, `STK_` environment, TOML file, defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

use crate::audio::spectral;
use crate::matching::MatchThresholds;
use crate::retag::TagPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MatchingSettings {
    pub artist: f64,
    pub artist_weak: f64,
    pub title: f64,
    pub title_strong: f64,
    pub artist_weight: f64,
}

impl Default for MatchingSettings {
    fn default() -> Self {
        let thresholds = MatchThresholds::default();
        Self {
            artist: thresholds.artist,
            artist_weak: thresholds.artist_weak,
            title: thresholds.title,
            title_strong: thresholds.title_strong,
            artist_weight: thresholds.artist_weight,
        }
    }
}

impl MatchingSettings {
    #[must_use]
    pub fn thresholds(&self) -> MatchThresholds {
        MatchThresholds {
            artist: self.artist,
            artist_weak: self.artist_weak,
            title: self.title,
            title_strong: self.title_strong,
            artist_weight: self.artist_weight,
        }
    }
}

/// How Soulseek search results are scored.
///
/// `reject_terms` and `penalise_terms` are matched against the whole remote
/// path, because a live recording or a cover often only says so in its folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RankingSettings {
    pub min_title_ratio: f64,

    /// Rank lossless above lossy, whatever the lossy file's bitrate.
    pub prefer_lossless: bool,

    /// Discard lossy results outright rather than ranking them lower.
    pub require_lossless: bool,

    /// Discard lossy results below this bitrate. Lossless is never discarded.
    pub min_bitrate: Option<u32>,

    pub reject_terms: Vec<String>,
    pub penalise_terms: Vec<String>,
    pub penalty_per_term: i64,
    pub duration_tolerance_seconds: f64,

    /// Bracketed parts of a name that don't make it another version, left out
    /// when flagging a qualifier a result lacks or adds. Matched at a word start.
    pub ignore_qualifiers: Vec<String>,
}

impl Default for RankingSettings {
    fn default() -> Self {
        Self {
            min_title_ratio: 0.45,
            prefer_lossless: true,
            require_lossless: false,
            min_bitrate: None,
            reject_terms: ["karaoke", "instrumental", "off vocal", "nightcore"]
                .map(str::to_owned)
                .to_vec(),
            penalise_terms: [
                "live",
                "remix",
                "cover",
                "symphony",
                "orchestra",
                "orchestral",
                "medley",
                "acoustic",
                "sped up",
                "8 bit",
            ]
            .map(str::to_owned)
            .to_vec(),
            penalty_per_term: 2_000_000,
            duration_tolerance_seconds: 12.0,
            ignore_qualifiers: [
                "feat",
                "ft",
                "featuring",
                "with",
                "remaster",
                "from",
                "original mix",
                "album version",
                "single version",
                "explicit",
                "mono",
                "stereo",
                "bonus track",
                "flac",
                "mp3",
                "320",
                "16bit",
                "24bit",
                "hi res",
                "web",
                "cd",
            ]
            .map(str::to_owned)
            .to_vec(),
        }
    }
}

/// Preferring one uploader across a batch when the per-track choice is close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UploaderSettings {
    pub prefer_one: bool,

    /// How far below a track's best a candidate may score and still be taken
    /// from the preferred peer.
    ///
    /// 2000 spans availability and name match (free slot 1000, queue up to 500,
    /// name up to 500) and nothing structural: a content penalty is 2,000,000
    /// and a quality tier 10,000,000.
    pub score_window: i64,

    /// How many of the batch's tracks a peer must cover to be preferred. A count,
    /// not a fraction, so large batches don't switch the feature off.
    pub minimum_coverage: usize,

    /// How many candidates per track the batch pass looks at. Deeper than the
    /// review cap, so a peer ranked fourth on every track is still seen.
    pub pool_per_track: usize,
}

impl Default for UploaderSettings {
    fn default() -> Self {
        Self {
            prefer_one: true,
            score_window: 2000,
            minimum_coverage: 3,
            pool_per_track: 25,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VerifySettings {
    pub steep_db_per_khz: f64,
    pub noise_floor_db: f64,
    pub near_nyquist_margin_hz: u32,
    pub probe_seconds: f64,
    pub probe_count: usize,
}

impl Default for VerifySettings {
    fn default() -> Self {
        Self {
            steep_db_per_khz: spectral::DEFAULT_STEEP_DB_PER_KHZ,
            noise_floor_db: spectral::DEFAULT_NOISE_FLOOR_DB,
            near_nyquist_margin_hz: spectral::DEFAULT_NEAR_NYQUIST_MARGIN_HZ,
            probe_seconds: 20.0,
            probe_count: 4,
        }
    }
}

impl VerifySettings {
    #[must_use]
    pub fn tuning(&self) -> spectral::Tuning {
        spectral::Tuning {
            noise_floor_db: self.noise_floor_db,
            steep_db_per_khz: self.steep_db_per_khz,
            near_nyquist_margin_hz: self.near_nyquist_margin_hz,
            fft_size: spectral::DEFAULT_FFT_SIZE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoudnessSettings {
    /// What a scanned file is levelled to. -18 LUFS is ReplayGain 2.0, as
    /// `loudgain`, `rsgain` and foobar2000 write it.
    pub reference_lufs: f64,

    /// Measure the oversampled true peak rather than the loudest sample, to
    /// catch inter-sample peaks above 0 dBFS.
    pub true_peak: bool,
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self {
            reference_lufs: -18.0,
            true_peak: true,
        }
    }
}

/// Connection details for slskd.
///
/// `api_key` is never serialised: it comes from `SLSKD_API_KEY` only, so a
/// config file is safe to commit and `stk config show` safe to paste.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SlskdSettings {
    pub url: String,
    #[serde(skip)]
    pub api_key: Option<String>,
    pub search_timeout_seconds: u32,
    pub search_response_limit: u32,
    pub request_timeout_seconds: f64,
    pub download_directory: Option<PathBuf>,
    /// How long a queued transfer may sit before it counts as stalled.
    pub stall_after_seconds: f64,
    /// How many searches to run at once. Bounded because slskd relays each one
    /// to the Soulseek network, which throttles floods.
    pub search_concurrency: usize,
    /// The least time between two searches starting, across workers, so a
    /// large sync doesn't flood the Soulseek server.
    pub search_interval_seconds: f64,
}

impl Default for SlskdSettings {
    fn default() -> Self {
        Self {
            url: "http://localhost:5030".to_owned(),
            api_key: None,
            search_timeout_seconds: 15,
            search_response_limit: 200,
            request_timeout_seconds: 30.0,
            download_directory: None,
            stall_after_seconds: 3600.0,
            search_concurrency: 4,
            search_interval_seconds: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LibrarySettings {
    /// The single-root spelling. [`Self::all_roots`] folds it into `roots`.
    pub root: PathBuf,
    /// Every root to scan. Empty means "just `root`".
    pub roots: Vec<PathBuf>,
    /// Globs to skip, so audiobooks and sample packs stay out of the index.
    pub exclude: Vec<String>,
    pub extensions: Vec<String>,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            root: PathBuf::from("Music"),
            roots: Vec::new(),
            exclude: Vec::new(),
            extensions: crate::library::scan::AUDIO_EXTENSIONS
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
        }
    }
}

impl LibrarySettings {
    #[must_use]
    pub fn all_roots(&self) -> Vec<PathBuf> {
        if self.roots.is_empty() {
            vec![self.root.clone()]
        } else {
            self.roots.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IndexSettings {
    pub path: PathBuf,
}

impl Default for IndexSettings {
    fn default() -> Self {
        Self {
            path: default_data_path("library.db"),
        }
    }
}

/// How many candidates a search keeps per track, so the picker can show more
/// on request without searching again.
pub const STORED_CANDIDATES: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReviewSettings {
    pub path: PathBuf,
    /// How many candidates the picker shows, when no `--candidates` is given.
    pub candidates_per_track: usize,

    /// How many of the queue's tracks one peer's folder must hold to be offered
    /// as an album.
    pub album_minimum_tracks: usize,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            path: default_data_path("review.db"),
            candidates_per_track: 5,
            album_minimum_tracks: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ArtworkSettings {
    pub providers: Vec<String>,
    pub min_bytes: usize,
    pub require_artist_match: bool,
    pub artist_match_threshold: f64,
    pub request_timeout_seconds: f64,
    pub preferred_size: u32,
    /// Override the pacing between requests to every provider. Unset leaves
    /// each on its published limit; 0 disables pacing.
    pub min_request_interval_seconds: Option<f64>,
    /// A URL or email address sent to every web service in the User-Agent.
    /// MusicBrainz and AcoustID throttle anonymous callers.
    pub contact: String,
}

impl Default for ArtworkSettings {
    fn default() -> Self {
        Self {
            providers: vec!["itunes".to_owned(), "coverartarchive".to_owned()],
            min_bytes: 3000,
            require_artist_match: true,
            artist_match_threshold: 0.8,
            request_timeout_seconds: 15.0,
            preferred_size: 1200,
            min_request_interval_seconds: None,
            contact: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IdentifySettings {
    /// How sure AcoustID has to be before a match is proposed. A re-encode of
    /// the same recording scores near 1; a live take or remix well under.
    pub min_score: f64,

    /// How alike a release title and the file's album or folder, or a credit
    /// and the file's artist, must be to count as the same. Above the score for
    /// one name containing another, so an artist's name inside a release title
    /// doesn't match.
    pub name_match_threshold: f64,

    pub request_timeout_seconds: f64,
}

impl Default for IdentifySettings {
    fn default() -> Self {
        Self {
            min_score: 0.85,
            name_match_threshold: 0.9,
            request_timeout_seconds: 15.0,
        }
    }
}

/// Reading a wantlist from a web service. The Discogs token and Last.fm key
/// come from the environment only, like the slskd key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WantlistSettings {
    pub request_timeout_seconds: f64,
    /// Every Discogs tracklist fetched so far, by release ID. A tracklist
    /// doesn't change, so each is fetched once.
    pub discogs_cache: PathBuf,
    /// Optional: Discogs answers without it, at 25 requests a minute rather than 60.
    #[serde(skip)]
    pub discogs_token: Option<String>,
    /// Required for `lastfm:<user>`: Last.fm refuses any request without one,
    /// public profile or not.
    #[serde(skip)]
    pub lastfm_api_key: Option<String>,
}

impl Default for WantlistSettings {
    fn default() -> Self {
        Self {
            request_timeout_seconds: 15.0,
            discogs_cache: default_data_path("discogs.json"),
            discogs_token: None,
            lastfm_api_key: None,
        }
    }
}

/// What joins a lead artist to the rest of a credit, lowercased:
/// `PinocchioP; Hatsune Miku`, `SAWTOWNE ft. 初音ミク`.
pub const CREDIT_SEPARATORS: &[&str] = &[
    ";", ",", "、", "，", " & ", " × ", " x ", " feat", " ft.", " ft ", " vo.", " with ",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OrganiseSettings {
    /// Where a file belongs, relative to its library root, with `/` between
    /// directories. The extension always comes from the file.
    pub pattern: String,

    pub track_number_width: usize,

    pub remove_empty_directories: bool,

    /// Where organised files go. Unset means each stays under its own root.
    pub destination: Option<PathBuf>,

    /// Artist spellings to file under an existing folder, such as
    /// `"Inaba Cumori" = "Inabakumori"`. Names that read the same in romaji
    /// already share a folder without one.
    pub artist_aliases: BTreeMap<String, String>,
}

impl Default for OrganiseSettings {
    fn default() -> Self {
        Self {
            pattern: "{artist}/{album}/{track} - {title}".to_owned(),
            track_number_width: 2,
            remove_empty_directories: true,
            destination: None,
            artist_aliases: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DupesSettings {
    /// The lowest score a pair in a group may have for the group to be resolved.
    /// Stricter than the finders' thresholds: a doubtful group is only shown.
    pub resolve_min_score: f64,
}

impl Default for DupesSettings {
    fn default() -> Self {
        Self {
            resolve_min_score: 0.95,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FetchSettings {
    /// `replace` writes the review row over the peer's tags and drops the
    /// rest of the block, `fill` writes only what the peer left empty, `keep`
    /// leaves the file as it arrived. Embedded artwork survives all three.
    pub tags: TagPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GenreSettings {
    pub map_path: PathBuf,
}

impl Default for GenreSettings {
    fn default() -> Self {
        Self {
            map_path: PathBuf::from("genres.toml"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    pub library: LibrarySettings,
    pub index: IndexSettings,
    pub review: ReviewSettings,
    pub matching: MatchingSettings,
    pub slskd: SlskdSettings,
    pub fetch: FetchSettings,
    pub ranking: RankingSettings,
    pub uploader: UploaderSettings,
    pub verify: VerifySettings,
    pub loudness: LoudnessSettings,
    pub artwork: ArtworkSettings,
    pub identify: IdentifySettings,
    pub genres: GenreSettings,
    pub organise: OrganiseSettings,
    pub dupes: DupesSettings,
    pub wantlist: WantlistSettings,
}

/// The per-user data directory. Not the `directories` crate, which adds its own
/// component on Windows and would miss existing databases.
#[must_use]
pub fn default_data_path(name: &str) -> PathBuf {
    app_directory().join(name)
}

#[must_use]
pub fn default_config_path() -> PathBuf {
    app_directory().join("config.toml")
}

fn app_directory() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("soulseektools")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library/Application Support/soulseektools")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let fallback = || {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".local/share")
        };
        std::env::var_os("XDG_DATA_HOME")
            .map_or_else(fallback, PathBuf::from)
            .join("soulseektools")
    }
}

pub const APP_NAME: &str = "soulseektools";
pub const CONFIG_FILENAME: &str = "stk.toml";
pub const API_KEY_ENV_VAR: &str = "SLSKD_API_KEY";
pub const DISCOGS_TOKEN_ENV_VAR: &str = "DISCOGS_TOKEN";
pub const LASTFM_API_KEY_ENV_VAR: &str = "LASTFM_API_KEY";
const ENV_PREFIX: &str = "STK_";
const ENV_NESTED_DELIMITER: &str = "__";

/// Locate the config file: nearest `stk.toml` upwards, else the user's.
#[must_use]
pub fn find_config_file(start: Option<&Path>) -> Option<PathBuf> {
    let directory = start
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())?;
    let directory = strip_verbatim(&directory.canonicalize().unwrap_or(directory));

    for candidate in std::iter::successors(Some(directory.as_path()), |path| path.parent()) {
        let local = candidate.join(CONFIG_FILENAME);
        if local.is_file() {
            return Some(local);
        }
    }

    let user = default_config_path();
    user.is_file().then_some(user)
}

const VERBATIM_PREFIX: &str = r"\\?\";
const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";

/// Drop the extended-length prefix a canonicalised Windows path carries, so
/// printed paths look like what the user typed.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(VERBATIM_UNC_PREFIX) {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    PathBuf::from(text.strip_prefix(VERBATIM_PREFIX).unwrap_or(&text))
}

/// Overrides a CLI flag supplies, which beat every other layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub library_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub settings: Settings,
    pub source_path: Option<PathBuf>,
}

impl Resolved {
    /// Every configured root, each made absolute relative to the config file.
    #[must_use]
    pub fn resolved_library_roots(&self) -> Vec<PathBuf> {
        self.settings
            .library
            .all_roots()
            .iter()
            .map(|root| self.resolve(root))
            .collect()
    }

    #[must_use]
    pub fn resolved_genre_map_path(&self) -> PathBuf {
        self.resolve(&self.settings.genres.map_path)
    }

    #[must_use]
    pub fn resolved_organise_destination(&self) -> Option<PathBuf> {
        self.settings
            .organise
            .destination
            .as_ref()
            .map(|path| self.resolve(path))
    }

    fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        let base = self
            .source_path
            .as_ref()
            .and_then(|source| source.parent().map(Path::to_path_buf))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        // `canonicalize` needs the path to exist; a configured root that has not
        // been created yet must still resolve.
        normalise(&base.join(path))
    }
}

/// Collapse `.` and `..` without touching the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut parts: Vec<std::path::Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(parts.last(), Some(std::path::Component::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            other => parts.push(other),
        }
    }
    parts.iter().collect()
}

/// Build the settings from the config file, the environment and `overrides`.
///
/// First match wins: CLI override, `STK_` variable, TOML file, defaults.
/// Secrets come from the environment only. An explicit `config_path` that
/// doesn't exist is an error.
pub fn load_settings(config_path: Option<&Path>, overrides: &Overrides) -> Result<Resolved> {
    if let Some(path) = config_path {
        if !path.is_file() {
            return Err(Error::Config(format!(
                "config file not found: {}",
                path.display()
            )));
        }
    }

    let path = config_path
        .map(Path::to_path_buf)
        .or_else(|| find_config_file(None));

    let mut table = match &path {
        Some(path) => read_toml(path)?,
        None => toml::Table::new(),
    };
    apply_environment(&mut table);
    apply_overrides(&mut table, overrides);

    let location = path.as_ref().map_or_else(
        || "<defaults>".to_owned(),
        |path| path.display().to_string(),
    );
    let mut settings: Settings = toml::Value::Table(table)
        .try_into()
        .map_err(|error| Error::Config(format!("invalid configuration ({location}): {error}")))?;

    settings.library.normalise_extensions();
    settings.validate(&location)?;
    settings.slskd.api_key = secret(API_KEY_ENV_VAR);
    settings.wantlist.discogs_token = secret(DISCOGS_TOKEN_ENV_VAR);
    settings.wantlist.lastfm_api_key = secret(LASTFM_API_KEY_ENV_VAR);

    let mut resolved = Resolved {
        settings,
        source_path: path,
    };
    let settings = &resolved.settings;
    let index = resolved.resolve(&settings.index.path);
    let review = resolved.resolve(&settings.review.path);
    let discogs_cache = resolved.resolve(&settings.wantlist.discogs_cache);
    let downloads = settings
        .slskd
        .download_directory
        .as_deref()
        .map(|path| resolved.resolve(path));
    resolved.settings.index.path = index;
    resolved.settings.review.path = review;
    resolved.settings.wantlist.discogs_cache = discogs_cache;
    resolved.settings.slskd.download_directory = downloads;
    Ok(resolved)
}

fn secret(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub fn read_toml(path: &Path) -> Result<toml::Table> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))?;
    text.parse::<toml::Table>()
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))
}

/// Overlay `STK_SECTION__KEY` variables onto the parsed file. A bare `STK_FOO`
/// names no section and is ignored.
fn apply_environment(table: &mut toml::Table) {
    for (name, value) in std::env::vars() {
        let Some(rest) = name.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        let Some((section, key)) = rest.split_once(ENV_NESTED_DELIMITER) else {
            continue;
        };

        let entry = table
            .entry(section.to_lowercase())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(entry) = entry.as_table_mut() {
            entry.insert(key.to_lowercase(), parse_scalar(&value));
        }
    }
}

fn parse_scalar(value: &str) -> toml::Value {
    if let Ok(parsed) = value.parse::<i64>() {
        return toml::Value::Integer(parsed);
    }
    if let Ok(parsed) = value.parse::<f64>() {
        return toml::Value::Float(parsed);
    }
    match value {
        "true" => toml::Value::Boolean(true),
        "false" => toml::Value::Boolean(false),
        other => toml::Value::String(other.to_owned()),
    }
}

/// `--library` names one root, so it has to overwrite both spellings: leaving a
/// `roots` list from the file in place would make the flag do nothing.
fn apply_overrides(table: &mut toml::Table, overrides: &Overrides) {
    let Some(root) = &overrides.library_root else {
        return;
    };
    let entry = table
        .entry("library".to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(entry) = entry.as_table_mut() {
        // Typed on the command line, so relative to where stk runs.
        let root = std::path::absolute(root).unwrap_or_else(|_| root.clone());
        let root = toml::Value::String(root.display().to_string());
        entry.insert("roots".to_owned(), toml::Value::Array(vec![root.clone()]));
        entry.insert("root".to_owned(), root);
    }
}

pub fn render_toml(settings: &Settings) -> Result<String> {
    toml::to_string_pretty(settings)
        .map_err(|error| Error::Config(format!("cannot render configuration: {error}")))
}

pub fn write_config(path: &Path, settings: &Settings) -> Result<PathBuf> {
    if path.exists() {
        return Err(Error::Config(format!(
            "config file already exists: {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::Config(format!("{}: {error}", parent.display())))?;
        }
    }

    let header = "# SoulseekTools configuration.\n\
                  # Secrets are never read from this file: export SLSKD_API_KEY,\n\
                  # DISCOGS_TOKEN and LASTFM_API_KEY instead.\n\n";
    std::fs::write(path, format!("{header}{}", render_toml(settings)?))
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;
    Ok(path.to_path_buf())
}

impl LibrarySettings {
    fn normalise_extensions(&mut self) {
        self.extensions = self
            .extensions
            .iter()
            .map(|extension| {
                let lowered = extension.to_lowercase();
                if lowered.starts_with('.') {
                    lowered
                } else {
                    format!(".{lowered}")
                }
            })
            .collect();
    }
}

impl Settings {
    /// Range checks serde can't express. A threshold outside `[0, 1]` would
    /// silently disable a guard.
    fn validate(&self, location: &str) -> Result<()> {
        let bad = |field: &str, detail: &str| {
            Err(Error::Config(format!(
                "invalid configuration ({location}): {field} {detail}"
            )))
        };

        for (field, value) in [
            ("matching.artist", self.matching.artist),
            ("matching.artist_weak", self.matching.artist_weak),
            ("matching.title", self.matching.title),
            ("matching.title_strong", self.matching.title_strong),
            ("matching.artist_weight", self.matching.artist_weight),
            ("ranking.min_title_ratio", self.ranking.min_title_ratio),
            (
                "artwork.artist_match_threshold",
                self.artwork.artist_match_threshold,
            ),
            ("identify.min_score", self.identify.min_score),
            ("dupes.resolve_min_score", self.dupes.resolve_min_score),
            (
                "identify.name_match_threshold",
                self.identify.name_match_threshold,
            ),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return bad(field, "must be between 0 and 1");
            }
        }

        if self.verify.steep_db_per_khz <= 0.0 {
            return bad("verify.steep_db_per_khz", "must be greater than 0");
        }
        if self.verify.noise_floor_db >= 0.0 {
            return bad("verify.noise_floor_db", "must be less than 0");
        }
        if self.verify.probe_seconds <= 0.0 {
            return bad("verify.probe_seconds", "must be greater than 0");
        }
        if self.verify.probe_count == 0 {
            return bad("verify.probe_count", "must be at least 1");
        }
        if !(1..=STORED_CANDIDATES).contains(&self.review.candidates_per_track) {
            return bad(
                "review.candidates_per_track",
                &format!("must be between 1 and {STORED_CANDIDATES}"),
            );
        }
        if self.review.album_minimum_tracks < 2 {
            return bad("review.album_minimum_tracks", "must be at least 2");
        }
        if self.slskd.search_timeout_seconds == 0 {
            return bad("slskd.search_timeout_seconds", "must be at least 1");
        }
        if !(self.slskd.search_interval_seconds >= 0.0
            && self.slskd.search_interval_seconds.is_finite())
        {
            return bad("slskd.search_interval_seconds", "must be 0 or more");
        }
        if self.slskd.request_timeout_seconds <= 0.0 {
            return bad("slskd.request_timeout_seconds", "must be greater than 0");
        }
        if self.identify.request_timeout_seconds <= 0.0 {
            return bad("identify.request_timeout_seconds", "must be greater than 0");
        }
        if self.wantlist.request_timeout_seconds <= 0.0 {
            return bad("wantlist.request_timeout_seconds", "must be greater than 0");
        }
        if self.uploader.minimum_coverage < 2 {
            return bad("uploader.minimum_coverage", "must be at least 2");
        }
        if self.uploader.pool_per_track == 0 {
            return bad("uploader.pool_per_track", "must be at least 1");
        }
        // Bounded against the scale it is compared on: a window wide enough to
        // cross a content penalty or a quality tier would let batch coherence
        // buy a live recording or a lossy file.
        if self.uploader.score_window < 0
            || self.uploader.score_window >= self.ranking.penalty_per_term
            || self.uploader.score_window >= crate::soulseek::ranking::TIER_WEIGHT
        {
            return bad(
                "uploader.score_window",
                "must be between 0 and the smaller of ranking.penalty_per_term and a quality tier",
            );
        }
        if !(1..=4).contains(&self.organise.track_number_width) {
            return bad("organise.track_number_width", "must be between 1 and 4");
        }
        // Fail at load rather than halfway through moving files.
        if let Err(error) = crate::organise::Pattern::parse(&self.organise.pattern) {
            return Err(Error::Config(format!(
                "invalid configuration ({location}): {error}"
            )));
        }
        Ok(())
    }
}
