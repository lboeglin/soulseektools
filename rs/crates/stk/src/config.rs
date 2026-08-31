//! Layered configuration.
//!
//! Every path, threshold and term list that used to be a literal in a script is
//! a field here. Resolution across the CLI flags, the `STK_` environment, the
//! TOML file and these defaults arrives with the CLI in step 5 of the port; the
//! sections themselves are needed sooner, because the matcher and the ranker
//! take them.

use serde::{Deserialize, Serialize};

use crate::audio::spectral;
use crate::matching::MatchThresholds;

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
    pub download_directory: Option<std::path::PathBuf>,
    /// How long a queued transfer may sit before it counts as stalled (#2).
    pub stall_after_seconds: f64,
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
        }
    }
}

/// Where the music lives and what counts as part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LibrarySettings {
    /// The single root the Python build knows about.
    ///
    /// Kept alongside `roots` so one `stk.toml` serves both trees while `py/`
    /// exists: pydantic's `extra="forbid"` would reject a `roots` key outright.
    /// It folds into [`Self::roots`] on read.
    pub root: std::path::PathBuf,
    /// Every root to scan (#2). Empty means "just `root`".
    pub roots: Vec<std::path::PathBuf>,
    /// Globs to skip, so audiobooks and sample packs stay out of the index (#2).
    pub exclude: Vec<String>,
    pub extensions: Vec<String>,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            root: std::path::PathBuf::from("Music"),
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
    pub fn all_roots(&self) -> Vec<std::path::PathBuf> {
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
    pub path: std::path::PathBuf,
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
    pub path: std::path::PathBuf,
    pub candidates_per_track: usize,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            path: default_data_path("review.db"),
            candidates_per_track: 3,
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
    /// A URL or email address identifying you to artwork providers.
    ///
    /// MusicBrainz asks callers to identify themselves and throttles those that
    /// do not. Optional, and empty by default: nothing is sent that you did not
    /// set.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GenreSettings {
    pub map_path: std::path::PathBuf,
}

impl Default for GenreSettings {
    fn default() -> Self {
        Self {
            map_path: std::path::PathBuf::from("genres.toml"),
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
    pub ranking: RankingSettings,
    pub verify: VerifySettings,
    pub artwork: ArtworkSettings,
    pub genres: GenreSettings,
}

/// Where `platformdirs` puts application data, reproduced exactly.
///
/// The `directories` crate appends its own component on Windows and would point
/// at a different file from the one the Python build writes; both have to open
/// the same database.
#[must_use]
pub fn default_data_path(name: &str) -> std::path::PathBuf {
    app_directory().join(name)
}

#[must_use]
pub fn default_config_path() -> std::path::PathBuf {
    app_directory().join("config.toml")
}

fn app_directory() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("soulseektools")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Library/Application Support/soulseektools")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_default()
                    .join(".local/share")
            })
            .join("soulseektools")
    }
}
