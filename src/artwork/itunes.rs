//! Cover art from the iTunes Search API.
//!
//! No authentication, direct image URLs and good coverage of commercial
//! releases, which makes it the first provider tried. Its match quality outside
//! the mainstream is poor, which is why every result goes through the artist
//! check in [`crate::artwork::base::select`].

use serde_json::Value;

use crate::artwork::base::{ArtCandidate, ArtHttp, ArtProvider, RateLimiter};
use crate::errors::Result;

pub const SEARCH_URL: &str = "https://itunes.apple.com/search";

/// Apple documents the Search API at roughly 20 calls per minute.
pub const MIN_INTERVAL_SECONDS: f64 = 3.0;

const THUMBNAIL_SIZE: &str = "100x100bb";

pub struct ITunesProvider {
    pub limit: u32,
    pub preferred_size: u32,
    limiter: RateLimiter,
}

impl ITunesProvider {
    #[must_use]
    pub fn new(preferred_size: u32, min_interval_seconds: f64) -> Self {
        Self {
            limit: 5,
            preferred_size,
            limiter: RateLimiter::new(min_interval_seconds),
        }
    }

    /// Ask for a full-size image instead of the 100px thumbnail in the response.
    fn upscale(&self, thumbnail_url: &str) -> String {
        let size = format!("{0}x{0}bb", self.preferred_size);
        thumbnail_url.replace(THUMBNAIL_SIZE, &size)
    }
}

impl ArtProvider for ITunesProvider {
    fn name(&self) -> &'static str {
        "itunes"
    }

    fn min_interval_seconds(&self) -> f64 {
        self.limiter.min_interval_seconds
    }

    fn search(&self, http: &dyn ArtHttp, artist: &str, title: &str) -> Result<Vec<ArtCandidate>> {
        self.limiter.wait();
        let payload = http.get_json(
            SEARCH_URL,
            &[
                ("term", format!("{artist} {title}")),
                ("media", "music".to_owned()),
                ("limit", self.limit.to_string()),
            ],
        )?;

        Ok(payload
            .get("results")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|result| {
                let thumbnail = result.get("artworkUrl100")?.as_str()?;
                if thumbnail.is_empty() {
                    return None;
                }
                Some(ArtCandidate {
                    url: self.upscale(thumbnail),
                    artist: text(result.get("artistName")),
                    title: text(result.get("trackName")),
                    album: text(result.get("collectionName")),
                    source: self.name().to_owned(),
                })
            })
            .collect())
    }
}

fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_owned()
}
