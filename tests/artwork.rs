//! Cover art selection, providers, pacing and embedding, against recorded HTTP
//! responses. The rate limiter is timed for real, with short intervals.

mod support;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};

use stk::artwork::base::{select, ArtCandidate, ArtProvider};
use stk::artwork::coverartarchive::CoverArtArchiveProvider;
use stk::artwork::itunes::ITunesProvider;
use stk::artwork::service::{
    audit, build_providers, detect_mime, embed, find_artwork, ArtworkResult,
};
use stk::config::ArtworkSettings;
use stk::errors::{Error, Result};
use stk::matching::NameForms;
use stk::models::{Artwork, LibraryEntry, ReplayGainTags, TrackTags};
use stk::services::{user_agent, RateLimiter, ServiceHttp};
use support::{encode, Encoded, Scratch};

#[derive(Default)]
struct Recorded {
    json: HashMap<String, Value>,
    bytes: HashMap<String, Vec<u8>>,
    failures: HashMap<String, String>,
    requested: Mutex<Vec<String>>,
}

impl Recorded {
    fn json_at(mut self, url: &str, body: Value) -> Self {
        self.json.insert(url.to_owned(), body);
        self
    }

    fn bytes_at(mut self, url: &str, body: Vec<u8>) -> Self {
        self.bytes.insert(url.to_owned(), body);
        self
    }

    fn failing(mut self, url: &str, message: &str) -> Self {
        self.failures.insert(url.to_owned(), message.to_owned());
        self
    }
}

impl ServiceHttp for Recorded {
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value> {
        let query: Vec<String> = params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        self.requested
            .lock()
            .expect("the recorder is not poisoned")
            .push(format!("{url}?{}", query.join("&")));

        if let Some(message) = self.failures.get(url) {
            return Err(Error::Config(message.clone()));
        }
        self.json
            .get(url)
            .cloned()
            .ok_or_else(|| Error::Config(format!("no stub for {url}")))
    }

    fn post_form_json(&self, url: &str, _form: &[(&str, String)]) -> Result<Value> {
        Err(Error::Config(format!("no provider posts to {url}")))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.requested
            .lock()
            .expect("the recorder is not poisoned")
            .push(url.to_owned());
        if let Some(message) = self.failures.get(url) {
            return Err(Error::Config(message.clone()));
        }
        self.bytes
            .get(url)
            .cloned()
            .ok_or_else(|| Error::Config(format!("no stub for {url}")))
    }
}

fn candidate(artist: &str, source: &str) -> ArtCandidate {
    ArtCandidate {
        url: format!("https://example.test/{artist}.jpg"),
        artist: artist.to_owned(),
        title: "Roki".to_owned(),
        source: source.to_owned(),
        album: String::new(),
    }
}

fn entry(artist: &str, title: &str, artwork_bytes: u64) -> LibraryEntry {
    LibraryEntry {
        path: PathBuf::from(artist).join(format!("{title}.flac")),
        size: 1,
        mtime: 1.0,
        folder_artist: artist.to_owned(),
        tags: TrackTags {
            artist: artist.to_owned(),
            title: title.to_owned(),
            ..TrackTags::default()
        },
        artist: NameForms::of(artist),
        title: NameForms::of(title),
        duration_seconds: None,
        artwork_bytes,
        codec: String::new(),
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
        replay_gain: ReplayGainTags::default(),
    }
}

fn jpeg(size: usize) -> Vec<u8> {
    let mut data = vec![0xff, 0xd8, 0xff];
    data.resize(size, 0);
    data
}

#[test]
fn the_wrong_artist_is_rejected_rather_than_embedded() {
    let selection = select(
        &[candidate("Some Other Band", "itunes")],
        "DECO*27",
        true,
        0.8,
    );

    assert!(selection.candidate.is_none());
    assert!(
        selection.reason.contains("Some Other Band"),
        "{}",
        selection.reason
    );
}

#[test]
fn the_best_matching_artist_wins_over_the_first_result() {
    let candidates = [
        candidate("Some Other Band", "itunes"),
        candidate("DECO*27", "itunes"),
    ];

    let selection = select(&candidates, "DECO*27", true, 0.8);

    assert_eq!(selection.candidate.expect("a selection").artist, "DECO*27");
}

#[test]
fn time_already_spent_counts_towards_the_interval() {
    // A second of interval so a loaded runner's oversleep cannot pass for a
    // full wait: at a fifth of a second a macOS runner overslept the 50 ms
    // remainder past the bound.
    let limiter = RateLimiter::new(1.0);
    limiter.wait();
    std::thread::sleep(std::time::Duration::from_millis(600));

    let started = Instant::now();
    limiter.wait();

    // Only the remainder is waited out, not another full interval.
    assert!(started.elapsed().as_secs_f64() < 0.8);
}

#[test]
fn user_agent_names_the_tool_and_only_a_configured_contact() {
    let anonymous = user_agent("");
    assert!(anonymous.starts_with("soulseektools/"), "{anonymous}");
    assert!(!anonymous.contains('('), "{anonymous}");
    assert!(user_agent("me@example.test").contains("me@example.test"));
}

#[test]
fn itunes_parses_results_and_upscales_the_thumbnail() {
    let http = Recorded::default().json_at(
        stk::artwork::itunes::SEARCH_URL,
        json!({
            "results": [{
                "artworkUrl100": "https://is1.test/image/100x100bb.jpg",
                "artistName": "DECO*27",
                "trackName": "Roki",
                "collectionName": "Ghost"
            }]
        }),
    );

    let provider = ITunesProvider::new(1200, 0.0);
    let candidates = provider.search(&http, "DECO*27", "Roki").expect("results");

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].url, "https://is1.test/image/1200x1200bb.jpg");
    assert_eq!(candidates[0].artist, "DECO*27");
    assert_eq!(candidates[0].album, "Ghost");
    assert_eq!(candidates[0].source, "itunes");
}

#[test]
fn coverartarchive_builds_an_archive_url_from_the_recordings_release() {
    let http = Recorded::default().json_at(
        stk::artwork::coverartarchive::SEARCH_URL,
        json!({
            "recordings": [{
                "title": "Roki",
                "artist-credit": [{"name": "DECO*27"}, {"name": "Hatsune Miku"}],
                "releases": [{"id": "abc-123", "title": "Ghost"}]
            }]
        }),
    );

    let provider = CoverArtArchiveProvider::new(1200, 0.0);
    let candidates = provider.search(&http, "DECO*27", "Roki").expect("results");

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].url,
        "https://coverartarchive.org/release/abc-123/front-1200"
    );
    assert_eq!(candidates[0].artist, "DECO*27 Hatsune Miku");
    assert_eq!(candidates[0].album, "Ghost");
}

#[test]
fn a_placeholder_sized_cover_counts_as_missing() {
    let statuses = audit(
        &[entry("DECO*27", "Roki", 500)],
        &ArtworkSettings::default(),
    );

    assert!(statuses[0].needs_artwork);
    assert!(
        statuses[0].reason.contains("only 500 bytes"),
        "{}",
        statuses[0].reason
    );
}

fn itunes_only() -> ArtworkSettings {
    ArtworkSettings {
        providers: vec!["itunes".to_owned()],
        min_request_interval_seconds: Some(0.0),
        ..ArtworkSettings::default()
    }
}

#[test]
fn a_wrong_artist_result_is_not_downloaded() {
    let settings = itunes_only();
    let http = Recorded::default().json_at(
        stk::artwork::itunes::SEARCH_URL,
        json!({"results": [{
            "artworkUrl100": "https://is1.test/100x100bb.jpg",
            "artistName": "Some Other Band", "trackName": "Roki"
        }]}),
    );

    let providers = build_providers(&settings).expect("providers");
    let result = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);

    assert!(!result.ok());
    assert!(result.artwork.is_none());
    // Nothing was fetched: the guard runs before the download.
    assert!(http
        .requested
        .lock()
        .expect("the recorder")
        .iter()
        .all(|url| url.starts_with(stk::artwork::itunes::SEARCH_URL)));
}

#[test]
fn an_unreachable_provider_does_not_abort_the_search() {
    let settings = ArtworkSettings {
        min_request_interval_seconds: Some(0.0),
        ..ArtworkSettings::default()
    };
    let http = Recorded::default()
        .failing(stk::artwork::itunes::SEARCH_URL, "503 Service Unavailable")
        .json_at(
            stk::artwork::coverartarchive::SEARCH_URL,
            json!({"recordings": [{
                "title": "Roki",
                "artist-credit": [{"name": "DECO*27"}],
                "releases": [{"id": "abc-123", "title": "Ghost"}]
            }]}),
        )
        .bytes_at(
            "https://coverartarchive.org/release/abc-123/front-1200",
            jpeg(10_000),
        );

    let providers = build_providers(&settings).expect("providers");
    let result = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);

    assert!(result.ok(), "{}", result.error);
}

#[test]
fn a_download_too_small_to_be_a_cover_is_rejected() {
    let settings = itunes_only();
    let http = Recorded::default()
        .json_at(
            stk::artwork::itunes::SEARCH_URL,
            json!({"results": [{
                "artworkUrl100": "https://is1.test/100x100bb.jpg",
                "artistName": "DECO*27", "trackName": "Roki"
            }]}),
        )
        .bytes_at("https://is1.test/1200x1200bb.jpg", jpeg(100));

    let providers = build_providers(&settings).expect("providers");
    let result = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);

    assert!(!result.ok());
    assert!(result.error.contains("only 100 bytes"), "{}", result.error);
}

#[test]
fn type_comes_from_the_bytes_not_the_headers() {
    for (data, expected) in [
        (
            vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "image/png",
        ),
        (vec![0xff, 0xd8, 0xff, 0xe0], "image/jpeg"),
        (vec![0x00, 0x01, 0x02], "image/jpeg"),
    ] {
        assert_eq!(detect_mime(&data), expected, "{data:?}");
    }
}

#[test]
fn embeds_a_verified_cover_into_a_real_file() {
    let scratch = Scratch::new("artwork-embed");
    let track = encode(
        &scratch.join("DECO_27").join("Roki.flac"),
        &Encoded::default(),
    );

    let mut written = entry("DECO*27", "Roki", 0);
    written.path = track.clone();
    let result = ArtworkResult {
        entry: written,
        selection: select(&[candidate("DECO*27", "itunes")], "DECO*27", true, 0.8),
        artwork: Some(Artwork {
            data: jpeg(10_000),
            mime: "image/jpeg".to_owned(),
        }),
        error: String::new(),
    };

    embed(&result).expect("an embedded cover");

    let embedded = stk::library::tags::read_artwork(&track).expect("readable tags");
    assert_eq!(embedded.expect("a cover").data.len(), 10_000);
}

#[test]
fn a_slow_provider_does_not_pace_a_fast_one() {
    let http = Recorded::default()
        .json_at(stk::artwork::itunes::SEARCH_URL, json!({"results": []}))
        .json_at(
            stk::artwork::coverartarchive::SEARCH_URL,
            json!({"recordings": []}),
        );
    let slow = ITunesProvider::new(1200, 0.3);
    let fast = CoverArtArchiveProvider::new(1200, 0.0);

    slow.search(&http, "a", "b").expect("a search");
    fast.search(&http, "a", "b").expect("a search");

    let started = Instant::now();
    fast.search(&http, "a", "b").expect("a search");

    assert!(started.elapsed().as_secs_f64() < 0.05);
}
