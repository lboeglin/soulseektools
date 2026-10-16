//! Cover art selection, providers, pacing and embedding, against recorded HTTP responses.

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
    audit, build_providers, cover_targets, detect_mime, embed, fill_year, find_artwork, load_image,
    ArtworkResult,
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
    /// When set, a search for any other term finds nothing.
    only_term: Option<String>,
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
        if let Some(term) = &self.only_term {
            if !query.contains(&format!("term={term}")) {
                return Ok(json!({"results": []}));
            }
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
        year: None,
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
        isrc: None,
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
    // A full second, so a loaded runner's oversleep can't pass for a full wait.
    let limiter = RateLimiter::new(1.0);
    limiter.wait();
    std::thread::sleep(std::time::Duration::from_millis(600));

    let started = Instant::now();
    limiter.wait();

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
                "collectionName": "Ghost",
                "releaseDate": "2017-02-24T08:00:00Z"
            }]
        }),
    );

    let provider = ITunesProvider::new(1200, 0.0);
    let candidates = provider.search(&http, "DECO*27", "Roki").expect("results");

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].url, "https://is1.test/image/1200x1200bb.jpg");
    assert_eq!(candidates[0].artist, "DECO*27");
    assert_eq!(candidates[0].album, "Ghost");
    assert_eq!(candidates[0].year, Some(2017));
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
                "releases": [{"id": "abc-123", "title": "Ghost", "date": "2016-11-02"}]
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
    assert_eq!(candidates[0].year, Some(2016));
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
    // The artist guard runs before any download.
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
        year: None,
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

#[test]
fn an_artist_tag_crediting_several_people_falls_back_to_the_album_artist() {
    let settings = itunes_only();
    let http = Recorded {
        only_term: Some("Duca Nekoichi".to_owned()),
        ..Recorded::default()
    }
    .json_at(
        stk::artwork::itunes::SEARCH_URL,
        json!({"results": [{
            "artworkUrl100": "https://is1.test/100x100bb.jpg",
            "artistName": "Duca", "trackName": "Nekoichi"
        }]}),
    )
    .bytes_at("https://is1.test/1200x1200bb.jpg", jpeg(10_000));
    let mut credited = entry("Duca, Puyo Suzuki, Peak A Soul+", "Nekoichi", 0);
    credited.tags.album_artist = "Duca".to_owned();

    let providers = build_providers(&settings).expect("providers");
    let result = find_artwork(&credited, &providers, &settings, &http);

    assert!(result.ok(), "{}", result.error);
    let searches = http.requested.lock().expect("the recorder").clone();
    assert!(searches[1].contains("term=Duca Nekoichi"), "{searches:?}");
}

fn itunes_dated(release_date: &str) -> Recorded {
    Recorded::default()
        .json_at(
            stk::artwork::itunes::SEARCH_URL,
            json!({"results": [{
                "artworkUrl100": "https://is1.test/100x100bb.jpg",
                "artistName": "DECO*27", "trackName": "Roki",
                "releaseDate": release_date
            }]}),
        )
        .bytes_at("https://is1.test/1200x1200bb.jpg", jpeg(10_000))
}

#[test]
fn a_verified_cover_brings_its_year_to_a_file_without_one() {
    let settings = itunes_only();
    let providers = build_providers(&settings).expect("providers");
    let http = itunes_dated("2017-02-24T08:00:00Z");
    let mut dated = entry("DECO*27", "Roki", 0);
    dated.tags.year = "2016".to_owned();

    let undated = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);
    let kept = find_artwork(&dated, &providers, &settings, &http);

    assert_eq!(undated.year.as_deref(), Some("2017"));
    assert_eq!(kept.year, None);
    assert!(kept.ok());
}

#[test]
fn an_unverified_cover_or_a_run_told_not_to_brings_no_year() {
    let http = itunes_dated("2017-02-24T08:00:00Z");
    for settings in [
        ArtworkSettings {
            require_artist_match: false,
            ..itunes_only()
        },
        ArtworkSettings {
            fill_year: false,
            ..itunes_only()
        },
    ] {
        let providers = build_providers(&settings).expect("providers");
        let result = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);

        assert!(result.ok(), "{}", result.error);
        assert_eq!(result.year, None);
    }
}

#[test]
fn a_date_that_does_not_start_with_a_year_is_no_year() {
    let settings = itunes_only();
    let providers = build_providers(&settings).expect("providers");
    let http = itunes_dated("unknown");

    let result = find_artwork(&entry("DECO*27", "Roki", 0), &providers, &settings, &http);

    assert!(result.ok(), "{}", result.error);
    assert_eq!(result.year, None);
}

/// Noise, so neither format compresses it under `min_bytes`.
fn image_bytes(side: u32, format: image::ImageFormat) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(side, side, |x, y| {
        let seed = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503);
        let [r, g, b, _] = seed.wrapping_mul(seed | 1).to_le_bytes();
        image::Rgb([r, g, b])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut bytes, format)
        .expect("an encoded image");
    bytes.into_inner()
}

fn small_covers() -> ArtworkSettings {
    ArtworkSettings {
        preferred_size: 64,
        ..ArtworkSettings::default()
    }
}

#[test]
fn a_chosen_cover_loads_from_a_file_or_a_url() {
    let scratch = Scratch::new("artwork-load");
    let png = image_bytes(64, image::ImageFormat::Png);
    let jpeg = image_bytes(64, image::ImageFormat::Jpeg);
    let local = scratch.join("cover.png");
    std::fs::write(&local, &png).expect("a written image");
    let url = "https://i1.sndcdn.test/artworks-t500x500.jpg";
    let http = Recorded::default().bytes_at(url, jpeg.clone());

    let (from_file, warning) =
        load_image(&local.display().to_string(), &http, &small_covers()).expect("a cover");
    assert_eq!(from_file.data, png);
    assert_eq!(from_file.mime, "image/png");
    assert_eq!(warning, None);

    let (from_url, _) = load_image(url, &http, &small_covers()).expect("a cover");
    assert_eq!(from_url.data, jpeg);
    assert_eq!(from_url.mime, "image/jpeg");
}

#[test]
fn a_chosen_cover_under_the_preferred_size_is_loaded_with_a_warning() {
    let scratch = Scratch::new("artwork-load-small");
    let local = scratch.join("cover.png");
    std::fs::write(&local, image_bytes(64, image::ImageFormat::Png)).expect("a written image");

    let (_, warning) = load_image(
        &local.display().to_string(),
        &Recorded::default(),
        &ArtworkSettings::default(),
    )
    .expect("a cover");

    assert_eq!(
        warning.as_deref(),
        Some("64x64 is under artwork.preferred_size (1200)")
    );
}

#[test]
fn a_chosen_cover_that_is_not_an_image_or_is_too_small_is_refused() {
    let scratch = Scratch::new("artwork-load-refused");
    let text = scratch.join("cover.jpg");
    std::fs::write(&text, b"<html>not found</html>".repeat(500)).expect("a written file");
    let broken = scratch.join("broken.jpg");
    std::fs::write(&broken, jpeg(10_000)).expect("a written file");
    let tiny = scratch.join("tiny.png");
    std::fs::write(&tiny, image_bytes(4, image::ImageFormat::Png)).expect("a written image");

    for (path, reason) in [
        (&text, "not a JPEG or PNG image"),
        (&broken, "not a readable image"),
        (&tiny, "under artwork.min_bytes (3000)"),
    ] {
        let error = load_image(
            &path.display().to_string(),
            &Recorded::default(),
            &small_covers(),
        )
        .expect_err("a refused image");
        assert!(error.to_string().contains(reason), "{error}");
    }
}

#[test]
fn a_folder_gives_the_covered_files_directly_inside_it() {
    let scratch = Scratch::new("artwork-targets");
    let folder = scratch.join("UNDEAD CORPORATION");
    let flac = encode(&folder.join("b.flac"), &Encoded::default());
    let mp3 = encode(&folder.join("a.mp3"), &Encoded::default());
    encode(&folder.join("c.wav"), &Encoded::default());
    std::fs::write(folder.join("notes.txt"), b"").expect("a written file");
    encode(&folder.join("Album").join("d.flac"), &Encoded::default());

    assert_eq!(
        cover_targets(&folder).expect("a listing"),
        vec![mp3, flac.clone()]
    );
    assert_eq!(cover_targets(&flac).expect("a file"), vec![flac]);
}

#[test]
fn a_filled_year_is_logged_never_overwrites_and_can_be_undone() {
    use stk::library::changes::{apply_undo, plan_undo, ChangeFilter, ChangeLog};
    use stk::library::index::LibraryIndex;
    use stk::library::tags;

    let scratch = Scratch::new("artwork-fill-year");
    let root = scratch.join("Music");
    let track = encode(
        &root.join("Rhapsody").join("Dawn of Victory.flac"),
        &Encoded::default(),
    );
    let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");
    let log = ChangeLog::open(&index, "art fetch", 100.0).expect("a run");

    assert!(fill_year(&mut index, &log, &track, "2000").expect("a written year"));
    assert!(!fill_year(&mut index, &log, &track, "2017").expect("a kept year"));
    assert_eq!(tags::read_tags(&track).expect("readable tags").year, "2000");

    let changes = index
        .changes(&ChangeFilter::default())
        .expect("the logged rows");
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (changes[0].command.as_str(), changes[0].field.as_str()),
        ("art fetch", "year")
    );

    let plan = plan_undo(&index, changes[0].clone()).expect("a plan");
    let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
    apply_undo(&mut index, &plan, &undo, &[root]).expect("an undo");
    assert_eq!(tags::read_tags(&track).expect("readable tags").year, "");
}
