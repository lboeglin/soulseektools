//! Layered configuration.
//!
//! Every path, threshold and term list that used to be a literal in a script is
//! a field here. Resolution across the CLI flags, the `STK_` environment, the
//! TOML file and these defaults arrives with the CLI in step 5 of the port; the
//! sections themselves are needed sooner, because the matcher and the ranker
//! take them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

use crate::audio::spectral;
use crate::matching::MatchThresholds;
use crate::retag::TagPolicy;

/// Thresholds deciding when two names describe the same track.
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
/// path, folder included. Scoring the filename alone was the root cause of most
/// wrong downloads: a file named exactly right still turns out to be a live
/// recording or an orchestral cover, and only its folder says so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RankingSettings {
    pub min_title_ratio: f64,

    /// Rank lossless above lossy, whatever the lossy file's bitrate.
    ///
    /// Turn this off to judge results on availability and name match alone,
    /// which is what you want when a smaller file beats a better one.
    pub prefer_lossless: bool,

    /// Discard lossy results outright rather than ranking them lower.
    pub require_lossless: bool,

    /// Discard lossy results below this bitrate. Lossless is never discarded.
    pub min_bitrate: Option<u32>,

    pub reject_terms: Vec<String>,
    pub penalise_terms: Vec<String>,
    pub penalty_per_term: i64,
    pub duration_tolerance_seconds: f64,
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
        }
    }
}

/// Preferring one uploader across a batch (#3).
///
/// Soulseek's cost is per queue position, not per file, so eight tracks from
/// one peer beats eight tracks from eight peers whenever the per-track choice
/// is close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UploaderSettings {
    pub prefer_one: bool,

    /// How far below a track's best a candidate may score and still be taken
    /// from the preferred peer.
    ///
    /// 2000 is the exact span of the availability part of the score: a free
    /// slot is worth 1000, a full queue costs up to 500, and the name match up
    /// to 500 more. Everything structural is further away than that -- a
    /// content penalty is 2,000,000, a quality tier 10,000,000, one sample-rate
    /// step within lossless 624,000 -- so a preference can trade availability
    /// and name match and can never trade a codec, a master or a live version.
    pub score_window: i64,

    /// How many of the batch's tracks a peer must cover to be preferred.
    ///
    /// A count rather than a fraction, so it behaves the same over ten tracks
    /// and over four hundred: a fraction would demand forty tracks from one
    /// peer on a large batch, which essentially never happens, and would
    /// silently switch the feature off on exactly the runs it helps most.
    pub minimum_coverage: usize,

    /// How many candidates per track the batch pass may look at.
    ///
    /// Deeper than the review cap, or a peer holding most of the batch is
    /// invisible on every track where they placed fourth.
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

/// Tuning for the transcoded-lossless check.
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

/// Tuning for the ReplayGain scan (#7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoudnessSettings {
    /// What a scanned file is levelled to.
    ///
    /// -18 LUFS is ReplayGain 2.0, and what `loudgain`, `rsgain` and
    /// foobar2000 write today. Moving it makes this tool's figures disagree
    /// with theirs, which is a reason to leave it alone and the reason it is
    /// settable at all.
    pub reference_lufs: f64,

    /// Measure the true peak rather than the loudest sample.
    ///
    /// A true-peak scan oversamples and catches the inter-sample peaks above
    /// 0 dBFS that a player applying positive gain clips on; the sample peak
    /// misses them and costs less to find.
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

/// Connection details for the headless Soulseek daemon.
///
/// `api_key` is deliberately absent from the serialised form: it is read from
/// the bare `SLSKD_API_KEY` variable only, never from the TOML file, so a config
/// file stays safe to commit and `stk config show` stays safe to paste.
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
    /// How long a queued transfer may sit before it counts as stalled (#2).
    pub stall_after_seconds: f64,
    /// How many searches to run at once.
    ///
    /// Every search is spent waiting on slskd rather than computing, so this is
    /// the one number that decides how long a batch takes. Bounded rather than
    /// unlimited: slskd relays each search to the Soulseek network, and a
    /// hundred at once is a good way to be throttled.
    pub search_concurrency: usize,
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
        }
    }
}

/// Where the music lives and what counts as part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LibrarySettings {
    /// The single-root spelling, kept because most libraries have one.
    ///
    /// It folds into [`Self::roots`] on read, so nothing downstream has to know
    /// which of the two a config used.
    pub root: PathBuf,
    /// Every root to scan (#2). Empty means "just `root`".
    pub roots: Vec<PathBuf>,
    /// Globs to skip, so audiobooks and sample packs stay out of the index (#2).
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
    /// Every configured root, however it was spelled.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReviewSettings {
    pub path: PathBuf,
    pub candidates_per_track: usize,

    /// How many of the queue's tracks one peer's folder must hold to be offered
    /// as an album (#6).
    ///
    /// A folder holding a single wanted track is not an album -- approving it is
    /// `review approve` with more typing.
    pub album_minimum_tracks: usize,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            path: default_data_path("review.db"),
            candidates_per_track: 3,
            album_minimum_tracks: 2,
        }
    }
}

/// Cover-art lookup and the guard against embedding the wrong artist's art.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ArtworkSettings {
    pub providers: Vec<String>,
    pub min_bytes: usize,
    pub require_artist_match: bool,
    pub artist_match_threshold: f64,
    pub request_timeout_seconds: f64,
    pub preferred_size: u32,
    /// Override the pacing between requests to every provider.
    ///
    /// Unset by default, which leaves each provider on its own published limit
    /// -- one second for MusicBrainz, three for iTunes. Setting this to 0
    /// disables pacing entirely, which will get you throttled on a large run.
    pub min_request_interval_seconds: Option<f64>,
    /// A URL or email address identifying you to every web service.
    ///
    /// MusicBrainz and AcoustID ask callers to identify themselves and throttle
    /// those that do not; it is sent to all of them, artwork or not, because a
    /// contact is a fact about you rather than about a service. Optional, and
    /// empty by default: nothing is sent that you did not set.
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

/// Identifying a file from its audio through AcoustID (#12).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IdentifySettings {
    /// How sure AcoustID has to be before a match is proposed at all.
    ///
    /// The service scores a lookup on how much of the print it recognised.
    /// A re-encode of the same recording scores near 1; a live take, a remix
    /// or a different recording of the same song scores well under this and
    /// is reported as no confident match rather than written, since a
    /// wrongly tagged file is worse than an untagged one.
    pub min_score: f64,

    /// How alike two names must be to count as the same one: a release title
    /// and the file's album tag or folder name, a recording's credit and the
    /// artist the file is filed under.
    ///
    /// Above the score the matcher gives one name contained in another,
    /// deliberately: a file filed under `Dave Rodgers` must not have every
    /// release with his name in its title claimed as the album it came from.
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

/// How files are laid out on disk (#3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OrganiseSettings {
    /// Where a file belongs, relative to its library root.
    ///
    /// Directories are separated with `/` on every platform. The extension is
    /// never part of it: a pattern that could change one would let a rename lie
    /// about the container.
    pub pattern: String,

    /// How many digits a track number is padded to.
    ///
    /// A setting rather than a `{track:02}` format spec: nobody varies the
    /// padding within one pattern, and a mini-language is a parser and a
    /// validation surface for something that never changes.
    pub track_number_width: usize,

    /// Remove the directories a move emptied.
    pub remove_empty_directories: bool,

    /// Where organised files go. Unset means each stays under its own root.
    pub destination: Option<PathBuf>,
}

impl Default for OrganiseSettings {
    fn default() -> Self {
        Self {
            pattern: "{artist}/{album}/{track} - {title}".to_owned(),
            track_number_width: 2,
            remove_empty_directories: true,
            destination: None,
        }
    }
}

/// What `stk dupes resolve` is sure enough about to act on (#15).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DupesSettings {
    /// The lowest score a pair in a group may have for the group to be resolved.
    ///
    /// Stricter than the finders' own thresholds on purpose. A pair is a
    /// judgement, not a fact -- a name match, or a fingerprint similar enough --
    /// and reporting a doubtful one costs a glance where moving it costs a
    /// track. A group with any pair below this is shown and left alone.
    pub resolve_min_score: f64,
}

impl Default for DupesSettings {
    fn default() -> Self {
        Self {
            resolve_min_score: 0.95,
        }
    }
}

/// What happens to a download's tags once it has arrived (#11).
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

/// The complete resolved configuration.
///
/// The five-layer resolution -- CLI flag, `STK_` environment, `stk.toml`, the
/// per-user file, these defaults -- arrives with the CLI in step 5.
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
}

/// Where `platformdirs` puts application data, reproduced exactly.
///
/// The `directories` crate appends its own component on Windows, which would
/// point at a different file from the one an existing install already has its
/// databases in.
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

/// Windows' extended-length path prefixes, which `canonicalize` introduces.
const VERBATIM_PREFIX: &str = r"\\?\";
const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";

/// Drop the extended-length prefix a canonicalised Windows path carries.
///
/// Every later `display()` would otherwise put it into output the Python never
/// produces: `config path` exists to be pasted into another command, and that
/// form is not what anyone typed.
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

/// The settings, plus where they came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub settings: Settings,
    /// The config file the values came from, if there was one.
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
/// Values resolve in this order, first match winning:
///
/// 1. an explicit override passed by a CLI flag
/// 2. a `STK_`-prefixed environment variable (`STK_LIBRARY__ROOT`)
/// 3. the TOML file -- `stk.toml` beside the project, else the per-user config
/// 4. the defaults
///
/// Secrets are the exception: the API key is read from the environment only,
/// never from the TOML file, so a config file stays safe to commit.
///
/// `config_path` is discovered when not given; passing a path that does not
/// exist is an error, because it means a `--config` flag pointed at nothing.
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
    settings.slskd.api_key = std::env::var(API_KEY_ENV_VAR)
        .ok()
        .filter(|key| !key.is_empty());

    Ok(Resolved {
        settings,
        source_path: path,
    })
}

/// Parse a TOML file, reporting a malformed one as a configuration error.
pub fn read_toml(path: &Path) -> Result<toml::Table> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))?;
    text.parse::<toml::Table>()
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", path.display())))
}

/// Overlay `STK_SECTION__KEY` variables onto the parsed file.
///
/// Only the nested form is honoured, matching pydantic-settings' `env_prefix`
/// plus `env_nested_delimiter`. A bare `STK_FOO` names no section and is ignored.
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

/// An environment variable is a string; TOML wants the typed value.
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
        let root = toml::Value::String(root.display().to_string());
        entry.insert("roots".to_owned(), toml::Value::Array(vec![root.clone()]));
        entry.insert("root".to_owned(), root);
    }
}

/// Serialise settings back to TOML, with secrets omitted.
///
/// `api_key` is `#[serde(skip)]`, so it cannot reach the file even by accident:
/// `stk config show` stays safe to paste.
pub fn render_toml(settings: &Settings) -> Result<String> {
    toml::to_string_pretty(settings)
        .map_err(|error| Error::Config(format!("cannot render configuration: {error}")))
}

/// Write a config file, refusing to clobber one that already exists.
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
                  # Secrets are never read from this file: export SLSKD_API_KEY instead.\n\n";
    std::fs::write(path, format!("{header}{}", render_toml(settings)?))
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;
    Ok(path.to_path_buf())
}

impl LibrarySettings {
    /// Accept `FLAC` and `.Mp3` alike, as the Python's validator does.
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
    /// The range checks pydantic does with `Field(ge=..., le=...)`.
    ///
    /// serde has no equivalent, and a threshold outside `[0, 1]` silently
    /// disables a guard rather than failing, so they are checked by hand.
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
        if !(1..=10).contains(&self.review.candidates_per_track) {
            return bad("review.candidates_per_track", "must be between 1 and 10");
        }
        if self.review.album_minimum_tracks < 2 {
            return bad("review.album_minimum_tracks", "must be at least 2");
        }
        if self.slskd.search_timeout_seconds == 0 {
            return bad("slskd.search_timeout_seconds", "must be at least 1");
        }
        if self.slskd.request_timeout_seconds <= 0.0 {
            return bad("slskd.request_timeout_seconds", "must be greater than 0");
        }
        if self.identify.request_timeout_seconds <= 0.0 {
            return bad("identify.request_timeout_seconds", "must be greater than 0");
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
        // Parsed here so a pattern that will not work fails at load, like every
        // other invalid setting, rather than at the moment of moving files.
        if let Err(error) = crate::organise::Pattern::parse(&self.organise.pattern) {
            return Err(Error::Config(format!(
                "invalid configuration ({location}): {error}"
            )));
        }
        Ok(())
    }
}
