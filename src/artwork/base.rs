//! Cover-art providers, and the guard that rejects a cover whose artist doesn't match.

use crate::errors::Result;
use crate::matching::{similarity, NameForms};
use crate::services::ServiceHttp;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtCandidate {
    pub url: String,
    pub artist: String,
    pub title: String,
    pub source: String,
    pub album: String,
    /// The release's year, when the provider gives one.
    pub year: Option<u32>,
}

impl ArtCandidate {
    #[must_use]
    pub fn artist_similarity(&self, wanted: &str) -> f64 {
        similarity(&NameForms::of(&self.artist), &NameForms::of(wanted))
    }
}

/// The year a provider's date starts with: only the year, since a reissue
/// often carries the original's year with its own month and day.
#[must_use]
pub fn leading_year(value: Option<&serde_json::Value>) -> Option<u32> {
    let date = value?.as_str()?;
    let year = date.get(..4)?;
    let tail = date.get(4..).unwrap_or_default();
    (crate::library::tags::is_date(year) && (tail.is_empty() || tail.starts_with('-')))
        .then(|| year.parse().ok())
        .flatten()
}

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

pub trait ArtProvider {
    fn name(&self) -> &'static str;

    fn min_interval_seconds(&self) -> f64;

    fn search(
        &self,
        http: &dyn ServiceHttp,
        artist: &str,
        title: &str,
    ) -> Result<Vec<ArtCandidate>>;
}

/// With `require_artist_match` off, the first result wins.
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
    // Stable, so equally good matches keep provider order.
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
