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
