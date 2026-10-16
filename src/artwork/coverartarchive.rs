//! Cover art from MusicBrainz (which release a recording is on) and the Cover
//! Art Archive (that release's front cover).

use serde_json::Value;

use crate::artwork::base::{leading_year, ArtCandidate, ArtProvider};
use crate::errors::Result;
use crate::services::{RateLimiter, ServiceHttp};

pub const SEARCH_URL: &str = "https://musicbrainz.org/ws/2/recording";
pub const ARCHIVE_URL: &str = "https://coverartarchive.org/release";

/// Only the MusicBrainz lookup is paced; the cover comes from the Internet Archive.
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

    fn search(
        &self,
        http: &dyn ServiceHttp,
        artist: &str,
        title: &str,
    ) -> Result<Vec<ArtCandidate>> {
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
                    year: first_release(recording)
                        .and_then(|release| leading_year(release.get("date"))),
                    source: self.name().to_owned(),
                })
            })
            .collect())
    }
}

fn first_release(recording: &Value) -> Option<&Value> {
    recording.get("releases")?.as_array()?.first()
}

fn first_release_id(recording: &Value) -> Option<String> {
    let id = first_release(recording)?.get("id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_owned())
}

fn release_title(recording: &Value) -> String {
    first_release(recording)
        .map(|release| text(release.get("title")))
        .unwrap_or_default()
}

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
