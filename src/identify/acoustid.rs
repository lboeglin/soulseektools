//! The AcoustID lookup, with `compress`ed values filled back in from their parents.

use serde_json::Value;

use crate::audio::fingerprint::Fingerprint;
use crate::errors::{Error, Result};
use crate::services::{RateLimiter, ServiceHttp};

pub const LOOKUP_URL: &str = "https://api.acoustid.org/v2/lookup";

/// AcoustID allows three requests per second per application.
pub const MIN_INTERVAL_SECONDS: f64 = 1.0 / 3.0;

/// Per application, not per user, and not a secret: it identifies the software.
const CLIENT_KEY: &str = "6uSuYbLF5R";

const META: &str = "recordings releasegroups releases tracks compress";

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub score: f64,
    pub recordings: Vec<Recording>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub id: String,
    pub title: String,
    pub artists: Vec<Credit>,
    pub releases: Vec<Release>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub id: String,
    pub title: String,
    /// `Various Artists` on a compilation; the recording keeps its own credit.
    pub artists: Vec<Credit>,
    pub compilation: bool,
    pub date: Option<Date>,
    pub track: Option<Track>,
}

/// May stop at the year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: u32,
    pub month: u32,
    pub day: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub position: Option<i64>,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    pub name: String,
    pub joinphrase: String,
}

#[must_use]
pub fn credited(credits: &[Credit]) -> String {
    credits
        .iter()
        .fold(String::new(), |mut text, credit| {
            text.push_str(&credit.name);
            text.push_str(&credit.joinphrase);
            text
        })
        .trim()
        .to_owned()
}

pub struct AcoustId {
    limiter: RateLimiter,
}

impl AcoustId {
    #[must_use]
    pub fn new(min_interval_seconds: f64) -> Self {
        Self {
            limiter: RateLimiter::new(min_interval_seconds),
        }
    }

    /// The duration goes up in whole seconds; a print without one is refused before any request.
    pub fn lookup(&self, http: &dyn ServiceHttp, fingerprint: &Fingerprint) -> Result<Vec<Hit>> {
        let Some(duration) = fingerprint.duration_seconds else {
            return Err(Error::Audio(
                "the fingerprint carries no duration, which the lookup needs".to_owned(),
            ));
        };

        self.limiter.wait();
        let payload = http.post_form_json(
            LOOKUP_URL,
            &[
                ("client", CLIENT_KEY.to_owned()),
                ("duration", format!("{}", duration.round())),
                ("fingerprint", fingerprint.compressed()),
                ("meta", META.to_owned()),
            ],
        )?;

        if payload.get("status").and_then(Value::as_str) != Some("ok") {
            let message = payload
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("no status in the response");
            return Err(Error::Config(format!("AcoustID: {message}")));
        }

        let mut hits: Vec<Hit> = list(payload.get("results")).iter().map(hit).collect();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(hits)
    }
}

fn hit(value: &Value) -> Hit {
    Hit {
        score: value.get("score").and_then(Value::as_f64).unwrap_or(0.0),
        recordings: list(value.get("recordings"))
            .iter()
            .map(recording)
            .collect(),
    }
}

fn recording(value: &Value) -> Recording {
    let title = text(value.get("title"));
    let artists = credits(value.get("artists"));
    let releases = list(value.get("releasegroups"))
        .iter()
        .flat_map(|group| {
            let group_title = text(group.get("title"));
            let group_artists = credits(group.get("artists"));
            let compilation = list(group.get("secondarytypes"))
                .iter()
                .filter_map(Value::as_str)
                .any(|kind| kind == "Compilation");
            list(group.get("releases"))
                .iter()
                .map(|release| Release {
                    id: text(release.get("id")),
                    title: release
                        .get("title")
                        .and_then(Value::as_str)
                        .map_or_else(|| group_title.clone(), str::to_owned),
                    artists: if release.get("artists").is_some() {
                        credits(release.get("artists"))
                    } else if group.get("artists").is_some() {
                        group_artists.clone()
                    } else {
                        artists.clone()
                    },
                    compilation,
                    date: release.get("date").and_then(date),
                    track: first_track(release, &title),
                })
                .collect::<Vec<_>>()
        })
        .collect();

    Recording {
        id: text(value.get("id")),
        title,
        artists,
        releases,
    }
}

/// A recording twice on one release is the same recording, so the first is as good as any.
fn first_track(release: &Value, recording_title: &str) -> Option<Track> {
    list(release.get("mediums"))
        .iter()
        .flat_map(|medium| list(medium.get("tracks")).to_vec())
        .next()
        .map(|track| Track {
            position: track.get("position").and_then(Value::as_i64),
            title: track
                .get("title")
                .and_then(Value::as_str)
                .map_or_else(|| recording_title.to_owned(), str::to_owned),
        })
}

fn date(value: &Value) -> Option<Date> {
    let part = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
    };
    Some(Date {
        year: part("year")?,
        month: part("month").unwrap_or(0),
        day: part("day").unwrap_or(0),
    })
}

fn credits(value: Option<&Value>) -> Vec<Credit> {
    list(value)
        .iter()
        .map(|credit| Credit {
            name: text(credit.get("name")),
            joinphrase: text(credit.get("joinphrase")),
        })
        .filter(|credit| !credit.name.is_empty())
        .collect()
}

fn list(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_owned()
}
