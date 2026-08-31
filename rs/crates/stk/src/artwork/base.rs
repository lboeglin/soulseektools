//! Cover-art providers, and the guard against embedding the wrong artist's art.
//!
//! The script this replaces embedded the first search result without checking
//! who it belonged to, and pasted the wrong artist's cover onto files twice.
//! Search match quality is inconsistent for anything outside the mainstream, so
//! a result is only accepted here when the artist it names actually matches the
//! artist asked for -- and a file keeps no artwork rather than the wrong artwork.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::errors::Result;
use crate::matching::{similarity, NameForms};

/// The User-Agent sent to artwork providers.
///
/// MusicBrainz asks callers to identify themselves with a real contact -- a URL
/// or an email address -- and throttles or blocks those that do not. Nothing is
/// invented here: set `artwork.contact` and it is included, leave it unset and
/// the header simply names the tool and its version.
#[must_use]
pub fn user_agent(contact: &str) -> String {
    let version = crate::VERSION;
    if contact.trim().is_empty() {
        format!("soulseektools/{version}")
    } else {
        format!("soulseektools/{version} ( {contact} )")
    }
}

/// Holds a caller to a minimum interval between requests.
///
/// These APIs are free and publish limits; exceeding them earns 503s and then a
/// block. One limiter lives on each provider instance, so the pacing is per
/// service rather than global -- a slow provider must not slow a fast one.
#[derive(Debug)]
pub struct RateLimiter {
    pub min_interval_seconds: f64,
    next_allowed_at: std::sync::Mutex<Option<Instant>>,
}

impl RateLimiter {
    #[must_use]
    pub fn new(min_interval_seconds: f64) -> Self {
        Self {
            min_interval_seconds,
            next_allowed_at: std::sync::Mutex::new(None),
        }
    }

    /// Block until the next request is allowed.
    pub fn wait(&self) {
        if self.min_interval_seconds <= 0.0 {
            return;
        }
        let mut next = self
            .next_allowed_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(allowed_at) = *next {
            let now = Instant::now();
            if allowed_at > now {
                std::thread::sleep(allowed_at - now);
            }
        }
        *next = Some(Instant::now() + Duration::from_secs_f64(self.min_interval_seconds));
    }
}

/// A cover image a provider believes belongs to the requested track.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtCandidate {
    pub url: String,
    pub artist: String,
    pub title: String,
    pub source: String,
    pub album: String,
}

impl ArtCandidate {
    #[must_use]
    pub fn artist_similarity(&self, wanted: &str) -> f64 {
        similarity(&NameForms::of(&self.artist), &NameForms::of(wanted))
    }
}

/// A chosen candidate, or the reason nothing was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtSelection {
    pub candidate: Option<ArtCandidate>,
    pub reason: String,
}

impl ArtSelection {
    #[must_use]
    pub fn none(reason: &str) -> Self {
        Self {
            candidate: None,
            reason: reason.to_owned(),
        }
    }

    #[must_use]
    pub fn is_some(&self) -> bool {
        self.candidate.is_some()
    }
}

/// The HTTP a provider needs, so its wire format is testable without network.
pub trait ArtHttp {
    /// A JSON GET with query parameters.
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value>;

    /// A raw GET, for the image itself.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>>;
}

/// A source of cover art.
pub trait ArtProvider {
    fn name(&self) -> &'static str;

    /// The provider's published request limit, in seconds between calls.
    fn min_interval_seconds(&self) -> f64;

    /// Candidates for one track, best first.
    fn search(&self, http: &dyn ArtHttp, artist: &str, title: &str) -> Result<Vec<ArtCandidate>>;
}

/// Pick the best candidate whose artist matches the one asked for.
///
/// With `require_artist_match` disabled the first result wins, reproducing the
/// original behaviour; it is on by default for the reason in the module
/// docstring.
#[must_use]
pub fn select(
    candidates: &[ArtCandidate],
    artist: &str,
    require_artist_match: bool,
    artist_threshold: f64,
) -> ArtSelection {
    let Some(first) = candidates.first() else {
        return ArtSelection::none("no results");
    };

    if !require_artist_match {
        return ArtSelection {
            candidate: Some(first.clone()),
            reason: format!("unverified match from {}", first.source),
        };
    }

    let mut scored: Vec<(f64, &ArtCandidate)> = candidates
        .iter()
        .map(|candidate| (candidate.artist_similarity(artist), candidate))
        .collect();
    // A stable sort keeps provider order among equally good matches, which is
    // what the Python's `sorted` on a negated key does.
    scored.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (best_score, best) = scored[0];
    if best_score < artist_threshold {
        return ArtSelection::none(&format!(
            "best result is by '{}' ({best_score:.2} similarity), not '{artist}'",
            best.artist
        ));
    }
    ArtSelection {
        candidate: Some(best.clone()),
        reason: format!(
            "matched '{}' ({best_score:.2}) via {}",
            best.artist, best.source
        ),
    }
}
