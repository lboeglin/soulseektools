//! A Last.fm user's loved tracks, plus with `--min-plays N` their top tracks
//! played at least N times (paging stops below N). Needs `LASTFM_API_KEY`.

use std::collections::HashMap;

use serde_json::Value;

use crate::errors::{Error, Result};
use crate::models::Track;
use crate::services::{RateLimiter, ServiceHttp};

pub const API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

/// Last.fm asks for no more than five requests a second.
pub const MIN_INTERVAL_SECONDS: f64 = 0.25;

/// The most a page may hold.
const PAGE_SIZE: u32 = 1000;

/// The columns every track carries, loved or played, so `--missing` writes one
/// header whichever list a track came from.
const COLUMNS: [&str; 7] = [
    "artist",
    "title",
    "playcount",
    "loved",
    "duration",
    "url",
    "mbid",
];

pub struct LastFm {
    limiter: RateLimiter,
    api_key: Option<String>,
}

impl LastFm {
    #[must_use]
    pub fn new(api_key: Option<String>, min_interval_seconds: f64) -> Self {
        Self {
            limiter: RateLimiter::new(min_interval_seconds),
            api_key,
        }
    }

    /// The user's loved tracks, then, under `min_plays`, every track played at
    /// least that often that is not already among them.
    pub fn read(
        &self,
        http: &dyn ServiceHttp,
        user: &str,
        min_plays: Option<usize>,
    ) -> Result<Vec<Track>> {
        let Some(api_key) = self.api_key.as_deref() else {
            return Err(Error::Config(format!(
                "lastfm:{user} needs a Last.fm API key: export LASTFM_API_KEY"
            )));
        };

        let mut tracks = Vec::new();
        let mut positions: HashMap<String, usize> = HashMap::new();
        for item in self.pages(
            http,
            api_key,
            user,
            "user.getlovedtracks",
            "lovedtracks",
            None,
        )? {
            let track = track(&item, true);
            positions.insert(key(&track), tracks.len());
            tracks.push(track);
        }

        if let Some(min_plays) = min_plays {
            for item in self.pages(
                http,
                api_key,
                user,
                "user.gettoptracks",
                "toptracks",
                Some(min_plays),
            )? {
                let played = track(&item, false);
                if let Some(&at) = positions.get(&key(&played)) {
                    let playcount = played.source_value(&["playcount"]).unwrap_or("");
                    set(&mut tracks[at], "playcount", playcount);
                } else {
                    positions.insert(key(&played), tracks.len());
                    tracks.push(played);
                }
            }
        }
        Ok(tracks)
    }

    /// Every item of a paged list, stopping under `min_plays` at the first
    /// played fewer times.
    fn pages(
        &self,
        http: &dyn ServiceHttp,
        api_key: &str,
        user: &str,
        method: &str,
        list: &str,
        min_plays: Option<usize>,
    ) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut page = 1_u64;
        loop {
            let params = vec![
                ("method", method.to_owned()),
                ("user", user.to_owned()),
                ("api_key", api_key.to_owned()),
                ("format", "json".to_owned()),
                ("limit", PAGE_SIZE.to_string()),
                ("page", page.to_string()),
            ];

            self.limiter.wait();
            let body = http.get_json(API_URL, &params).map_err(|error| {
                // The key rides in the query string, which the transport quotes back.
                Error::Config(format!(
                    "Last.fm {method}: {}",
                    error.to_string().replace(api_key, "<key>")
                ))
            })?;
            if let Some(error) = body.get("error") {
                let message = body
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("error {error}"), str::to_owned);
                return Err(Error::Config(format!("Last.fm: {message}")));
            }

            let listed = body.get(list);
            for item in entries(listed.and_then(|listed| listed.get("track"))) {
                if min_plays.is_some_and(|min_plays| plays(item) < min_plays) {
                    return Ok(items);
                }
                items.push(item.clone());
            }

            let total_pages = listed
                .and_then(|listed| listed.get("@attr"))
                .and_then(|attributes| number(attributes.get("totalPages")))
                .unwrap_or(0);
            if page >= total_pages {
                return Ok(items);
            }
            page += 1;
        }
    }
}

fn track(item: &Value, loved: bool) -> Track {
    let artist = text(item.get("artist").and_then(|artist| artist.get("name")));
    let title = text(item.get("name"));
    let duration = number(item.get("duration"))
        .and_then(|seconds| u32::try_from(seconds).ok())
        .filter(|seconds| *seconds > 0);
    let loved_at = if loved {
        text(item.get("date").and_then(|date| date.get("#text")))
    } else {
        String::new()
    };
    let playcount = if loved {
        String::new()
    } else {
        text(item.get("playcount"))
    };

    let values = [
        artist.clone(),
        title.clone(),
        playcount,
        loved_at,
        duration
            .map(|seconds| seconds.to_string())
            .unwrap_or_default(),
        text(item.get("url")),
        text(item.get("mbid")),
    ];
    Track {
        artist,
        title,
        album: String::new(),
        duration_seconds: duration.map(f64::from),
        title_guessed: false,
        source_row: COLUMNS
            .iter()
            .map(|&column| column.to_owned())
            .zip(values)
            .collect(),
    }
}

fn key(track: &Track) -> String {
    track.query().to_lowercase()
}

fn set(track: &mut Track, column: &str, value: &str) {
    if let Some((_, cell)) = track.source_row.iter_mut().find(|(key, _)| key == column) {
        value.clone_into(cell);
    }
}

fn plays(item: &Value) -> usize {
    number(item.get("playcount"))
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0)
}

/// A list Last.fm writes as an array, or as the one object when it holds one.
fn entries(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(item @ Value::Object(_)) => vec![item],
        _ => Vec::new(),
    }
}

/// Last.fm writes its numbers as strings.
fn number(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}
