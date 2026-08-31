//! Ported from `py/tests/unit/test_artwork.py`.
//!
//! The Python mocks `httpx` with `respx` and freezes the clock with a fake
//! `time.monotonic`. Here the providers take an `ArtHttp`, so a recorded set of
//! responses substitutes for the network, and the rate limiter is measured
//! against real elapsed time with intervals small enough to stay quick.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{json, Value};

use stk::artwork::base::{select, user_agent, ArtCandidate, ArtHttp, ArtProvider, RateLimiter};
use stk::artwork::coverartarchive::CoverArtArchiveProvider;
use stk::artwork::itunes::ITunesProvider;
use stk::artwork::service::{
    audit, build_providers, detect_mime, download, embed, find_artwork, ArtworkResult,
};
use stk::config::ArtworkSettings;
use stk::errors::{Error, Result};
use stk::matching::NameForms;
use stk::models::{Artwork, LibraryEntry, TrackTags};
use support::{encode, Encoded, Scratch};

macro_rules! needs_ffmpeg {
    () => {
        if support::ffmpeg().is_none() {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        }
    };
}

#[derive(Default)]
struct Recorded {
    json: HashMap<String, Value>,
    bytes: HashMap<String, Vec<u8>>,
    failures: HashMap<String, String>,
    requested: RefCell<Vec<String>>,
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

impl ArtHttp for Recorded {
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value> {
        let query: Vec<String> = params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        self.requested
            .borrow_mut()
            .push(format!("{url}?{}", query.join("&")));

        if let Some(message) = self.failures.get(url) {
            return Err(Error::Config(message.clone()));
        }
        self.json
            .get(url)
            .cloned()
            .ok_or_else(|| Error::Config(format!("no stub for {url}")))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.requested.borrow_mut().push(url.to_owned());
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
    }
}

fn jpeg(size: usize) -> Vec<u8> {
    let mut data = vec![0xff, 0xd8, 0xff];
    data.resize(size, 0);
    data
}

#[test]
fn nothing_is_selected_from_an_empty_result_set() {
    let selection = select(&[], "DECO*27", true, 0.8);
    assert!(selection.candidate.is_none());
    assert_eq!(selection.reason, "no results");
}

#[test]
fn a_matching_artist_is_selected() {
    let selection = select(&[candidate("DECO*27", "itunes")], "DECO_27", true, 0.8);
    assert!(selection.is_some());
}

#[test]
fn the_wrong_artist_is_rejected_rather_than_embedded() {
    // The failure this guard exists for: the script it replaces pasted the wrong
    // artist's cover onto files twice.
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
fn verification_can_be_disabled() {
    let selection = select(
        &[candidate("Some Other Band", "itunes")],
        "DECO*27",
        false,
        0.8,
    );

    assert!(selection.is_some());
    assert!(
        selection.reason.contains("unverified"),
        "{}",
        selection.reason
    );
}

#[test]
fn the_threshold_is_configurable() {
    let candidates = [candidate("DECO 27 Official", "itunes")];

    assert!(select(&candidates, "DECO*27", true, 0.99)
        .candidate
        .is_none());
    assert!(select(&candidates, "DECO*27", true, 0.3).is_some());
}

#[test]
fn the_first_request_is_not_delayed() {
    let limiter = RateLimiter::new(0.2);
    let started = Instant::now();
    limiter.wait();

    assert!(started.elapsed().as_secs_f64() < 0.1);
}

#[test]
fn a_following_request_waits_out_the_interval() {
    let limiter = RateLimiter::new(0.2);
    limiter.wait();

    let started = Instant::now();
    limiter.wait();

    assert!(started.elapsed().as_secs_f64() >= 0.15);
}

#[test]
fn time_already_spent_counts_towards_the_interval() {
    let limiter = RateLimiter::new(0.2);
    limiter.wait();
    std::thread::sleep(std::time::Duration::from_millis(150));

    let started = Instant::now();
    limiter.wait();

    // Only the remainder is waited out, not another full interval.
    assert!(started.elapsed().as_secs_f64() < 0.15);
}

#[test]
fn no_wait_is_needed_once_the_interval_has_passed() {
    let limiter = RateLimiter::new(0.1);
    limiter.wait();
    std::thread::sleep(std::time::Duration::from_millis(150));

    let started = Instant::now();
    limiter.wait();

    assert!(started.elapsed().as_secs_f64() < 0.05);
}

#[test]
fn a_zero_interval_disables_pacing() {
    let limiter = RateLimiter::new(0.0);
    let started = Instant::now();
    limiter.wait();
    limiter.wait();

    assert!(started.elapsed().as_secs_f64() < 0.05);
}

#[test]
fn each_limiter_paces_independently() {
    // A slow provider must not slow a fast one.
    let slow = RateLimiter::new(0.3);
    let fast = RateLimiter::new(0.0);
    slow.wait();
    fast.wait();

    let started = Instant::now();
    fast.wait();

    assert!(started.elapsed().as_secs_f64() < 0.05);
}

#[test]
fn musicbrainz_uses_its_published_one_second_limit() {
    assert!((stk::artwork::coverartarchive::MIN_INTERVAL_SECONDS - 1.0).abs() < f64::EPSILON);
}

#[test]
fn itunes_uses_apples_documented_limit() {
    assert!((stk::artwork::itunes::MIN_INTERVAL_SECONDS - 3.0).abs() < f64::EPSILON);
}

#[test]
fn providers_are_built_with_their_own_limits_by_default() {
    let providers = build_providers(&ArtworkSettings::default()).expect("providers");

    assert_eq!(providers[0].name(), "itunes");
    assert!((providers[0].min_interval_seconds() - 3.0).abs() < f64::EPSILON);
    assert_eq!(providers[1].name(), "coverartarchive");
    assert!((providers[1].min_interval_seconds() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn the_interval_can_be_overridden_for_every_provider() {
    let settings = ArtworkSettings {
        min_request_interval_seconds: Some(0.0),
        ..ArtworkSettings::default()
    };
    let providers = build_providers(&settings).expect("providers");

    assert!(providers
        .iter()
        .all(|provider| provider.min_interval_seconds() == 0.0));
}

#[test]
fn user_agent_names_the_tool_and_its_version() {
    let agent = user_agent("");
    assert!(agent.starts_with("soulseektools/"), "{agent}");
}

#[test]
fn a_configured_contact_is_included() {
    let agent = user_agent("me@example.test");
    assert!(agent.contains("me@example.test"), "{agent}");
}

#[test]
fn nothing_is_sent_that_was_not_configured() {
    // MusicBrainz asks callers to identify themselves; inventing a contact would
    // be worse than sending none.
    assert!(!user_agent("").contains('('));
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
fn itunes_results_without_artwork_are_skipped() {
    let http = Recorded::default().json_at(
        stk::artwork::itunes::SEARCH_URL,
        json!({"results": [{"artistName": "DECO*27"}]}),
    );

    let provider = ITunesProvider::new(1200, 0.0);
    assert!(provider
        .search(&http, "DECO*27", "Roki")
        .expect("results")
        .is_empty());
}

#[test]
fn an_http_error_propagates_to_the_caller() {
    let http = Recorded::default().failing(stk::artwork::itunes::SEARCH_URL, "503");
    let provider = ITunesProvider::new(1200, 0.0);

    assert!(provider.search(&http, "DECO*27", "Roki").is_err());
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
fn a_recording_with_no_release_is_skipped() {
    let http = Recorded::default().json_at(
        stk::artwork::coverartarchive::SEARCH_URL,
        json!({"recordings": [{"title": "Roki", "artist-credit": [{"name": "DECO*27"}]}]}),
    );

    let provider = CoverArtArchiveProvider::new(1200, 0.0);
    assert!(provider
        .search(&http, "DECO*27", "Roki")
        .expect("results")
        .is_empty());
}

#[test]
fn a_file_with_no_cover_needs_artwork() {
    let statuses = audit(&[entry("DECO*27", "Roki", 0)], &ArtworkSettings::default());

    assert!(statuses[0].needs_artwork);
    assert_eq!(statuses[0].reason, "no embedded cover");
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

#[test]
fn a_real_cover_is_left_alone() {
    let statuses = audit(
        &[entry("DECO*27", "Roki", 50_000)],
        &ArtworkSettings::default(),
    );

    assert!(!statuses[0].needs_artwork);
}

#[test]
fn builds_the_configured_providers_in_order() {
    let settings = ArtworkSettings {
        providers: vec!["coverartarchive".to_owned(), "itunes".to_owned()],
        ..ArtworkSettings::default()
    };
    let providers = build_providers(&settings).expect("providers");

    assert_eq!(providers[0].name(), "coverartarchive");
    assert_eq!(providers[1].name(), "itunes");
}

#[test]
fn an_unknown_provider_is_rejected_with_the_valid_names() {
    let settings = ArtworkSettings {
        providers: vec!["discogs".to_owned()],
        ..ArtworkSettings::default()
    };

    let Err(error) = build_providers(&settings) else {
        panic!("an unknown provider should be rejected");
    };
    assert!(error.to_string().contains("itunes"), "{error}");
    assert!(error.to_string().contains("coverartarchive"), "{error}");
}

fn itunes_only() -> ArtworkSettings {
    ArtworkSettings {
        providers: vec!["itunes".to_owned()],
        min_request_interval_seconds: Some(0.0),
        ..ArtworkSettings::default()
    }
}

#[test]
fn downloads_a_verified_match() {
    let settings = itunes_only();
    let http = Recorded::default()
        .json_at(
            stk::artwork::itunes::SEARCH_URL,
            json!({"results": [{
                "artworkUrl100": "https://is1.test/100x100bb.jpg",
                "artistName": "DECO*27", "trackName": "Roki"
            }]}),
        )
        .bytes_at("https://is1.test/1200x1200bb.jpg", jpeg(10_000));

    let providers = build_providers(&settings).expect("providers");
    let result = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);

    assert!(result.ok(), "{}", result.error);
    assert_eq!(result.artwork.expect("artwork").mime, "image/jpeg");
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
        .borrow()
        .iter()
        .all(|url| url.starts_with(stk::artwork::itunes::SEARCH_URL)));
}

#[test]
fn the_second_provider_is_tried_when_the_first_finds_nothing() {
    let settings = ArtworkSettings {
        min_request_interval_seconds: Some(0.0),
        ..ArtworkSettings::default()
    };
    let http = Recorded::default()
        .json_at(stk::artwork::itunes::SEARCH_URL, json!({"results": []}))
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
    assert_eq!(
        result.selection.candidate.expect("a selection").source,
        "coverartarchive"
    );
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
fn a_failed_download_is_reported_not_raised() {
    let settings = itunes_only();
    let http = Recorded::default()
        .json_at(
            stk::artwork::itunes::SEARCH_URL,
            json!({"results": [{
                "artworkUrl100": "https://is1.test/100x100bb.jpg",
                "artistName": "DECO*27", "trackName": "Roki"
            }]}),
        )
        .failing("https://is1.test/1200x1200bb.jpg", "404 Not Found");

    let providers = build_providers(&settings).expect("providers");
    let result = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);

    assert!(!result.ok());
    assert!(result.error.contains("404"), "{}", result.error);
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
fn download_takes_its_type_from_the_bytes() {
    let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0];
    let http = Recorded::default().bytes_at("https://x.test/cover", png.clone());

    let artwork = download(&http, "https://x.test/cover").expect("a download");

    assert_eq!(artwork.mime, "image/png");
    assert_eq!(artwork.data, png);
}

#[test]
fn embeds_a_verified_cover_into_a_real_file() {
    needs_ffmpeg!();
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

    assert_eq!(stk::library::tags::artwork_size(&track), 10_000);
}

#[test]
fn embedding_nothing_is_a_programming_error() {
    let result = ArtworkResult {
        entry: entry("DECO*27", "Roki", 0),
        selection: select(&[], "DECO*27", true, 0.8),
        artwork: None,
        error: String::new(),
    };

    let error = embed(&result).expect_err("nothing to embed");
    assert!(error.to_string().contains("nothing to embed"), "{error}");
}

#[test]
fn repeated_searches_are_paced() {
    let http =
        Recorded::default().json_at(stk::artwork::itunes::SEARCH_URL, json!({"results": []}));
    let provider = ITunesProvider::new(1200, 0.2);

    provider.search(&http, "a", "b").expect("a search");
    let started = Instant::now();
    provider.search(&http, "a", "b").expect("a search");

    assert!(started.elapsed().as_secs_f64() >= 0.15);
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

#[test]
fn the_configured_contact_reaches_the_request() {
    // The transport is what carries it; this pins the header value it is built
    // from rather than opening a socket.
    let settings = ArtworkSettings {
        contact: "me@example.test".to_owned(),
        ..ArtworkSettings::default()
    };
    assert!(user_agent(&settings.contact).contains("me@example.test"));
}
