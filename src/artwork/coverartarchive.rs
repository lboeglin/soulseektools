//! Cover art from MusicBrainz and the Cover Art Archive.
//!
//! The fallback when iTunes has nothing, which is common for doujin, Vocaloid
//! and game-soundtrack releases. MusicBrainz answers which release a recording
//! belongs to; the Cover Art Archive serves that release's front cover.

use serde_json::Value;

use crate::artwork::base::{ArtCandidate, ArtHttp, ArtProvider, RateLimiter};
use crate::errors::Result;

pub const SEARCH_URL: &str = "https://musicbrainz.org/ws/2/recording";
pub const ARCHIVE_URL: &str = "https://coverartarchive.org/release";

/// MusicBrainz enforces one request per second per address, then blocks.
///
/// Only the lookup is paced. The cover itself comes from the Internet Archive,
/// which does not share that budget.
pub const MIN_INTERVAL_SECONDS: f64 = 1.0;

pub struct CoverArtArchiveProvider {
    pub limit: u32,
    size_segment: &'static str,
    limiter: RateLimiter,
}

impl CoverArtArchiveProvider {
    #[must_use]
    pub fn new(preferred_size: u32, min_interval_seconds: f64) -> Self {
        Self {
            limit: 5,
            size_segment: if preferred_size > 500 { "1200" } else { "500" },
            limiter: RateLimiter::new(min_interval_seconds),
        }
    }
}

impl ArtProvider for CoverArtArchiveProvider {
    fn name(&self) -> &'static str {
        "coverartarchive"
    }

    fn min_interval_seconds(&self) -> f64 {
        self.limiter.min_interval_seconds
    }

    fn search(&self, http: &dyn ArtHttp, artist: &str, title: &str) -> Result<Vec<ArtCandidate>> {
        self.limiter.wait();
        let payload = http.get_json(
            SEARCH_URL,
            &[
                (
                    "query",
                    format!("recording:\"{title}\" AND artist:\"{artist}\""),
                ),
                ("fmt", "json".to_owned()),
                ("limit", self.limit.to_string()),
            ],
        )?;

        Ok(payload
            .get("recordings")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|recording| {
                let release_id = first_release_id(recording)?;
                Some(ArtCandidate {
                    url: format!("{ARCHIVE_URL}/{release_id}/front-{}", self.size_segment),
                    artist: credited_artist(recording),
                    title: text(recording.get("title")),
                    album: release_title(recording),
                    source: self.name().to_owned(),
                })
            })
            .collect())
    }
}

fn first_release_id(recording: &Value) -> Option<String> {
    let first = recording.get("releases")?.as_array()?.first()?;
    let id = first.get("id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_owned())
}

fn release_title(recording: &Value) -> String {
    recording
        .get("releases")
        .and_then(Value::as_array)
        .and_then(|releases| releases.first())
        .map(|release| text(release.get("title")))
        .unwrap_or_default()
}

/// A recording's artist credit is a list of parts, which join into one name.
fn credited_artist(recording: &Value) -> String {
    recording
        .get("artist-credit")
        .and_then(Value::as_array)
        .map(|credits| {
            credits
                .iter()
                .filter_map(|credit| credit.get("name").and_then(Value::as_str))
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_owned()
}
