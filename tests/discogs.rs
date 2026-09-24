//! Reading a Discogs collection export, against recorded responses.
//!
//! Every release under `tests/data/discogs` was recorded from the live API:
//! a plain vinyl album, a compilation whose tracks carry their own artists --
//! one of them disambiguated as `Care Company (2)`, one joined by `Presents` --
//! and a classical record whose concertos are indexes over their movements.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use stk::errors::{Error, Result};
use stk::services::ServiceHttp;
use stk::wantlist::discogs::{MIN_INTERVAL_SECONDS, MIN_INTERVAL_WITH_TOKEN_SECONDS, RELEASE_URL};
use stk::wantlist::{read_export, Discogs, Format, LastFm, Remote, TRACK_NUMBER_COLUMNS};
use support::Scratch;

const HEADER: &str = "Catalog#,Artist,Title,Label,Format,Rating,Released,release_id,\
                      CollectionFolder,Date Added,Collection Media Condition,\
                      Collection Sleeve Condition,Collection Notes";

/// A URL asked for, and the query parameters it was asked with.
type Request = (String, Vec<(String, String)>);

/// A transport answering each release URL with its recorded body.
#[derive(Default)]
struct Recorded {
    releases: HashMap<String, Value>,
    requested: RefCell<Vec<Request>>,
}

impl Recorded {
    fn with(mut self, id: &str, body: Value) -> Self {
        self.releases.insert(format!("{RELEASE_URL}/{id}"), body);
        self
    }

    fn requested_urls(&self) -> Vec<String> {
        self.requested
            .borrow()
            .iter()
            .map(|(url, _)| url.clone())
            .collect()
    }
}

impl ServiceHttp for Recorded {
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value> {
        self.requested.borrow_mut().push((
            url.to_owned(),
            params
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        ));
        self.releases.get(url).cloned().ok_or_else(|| {
            Error::Config(format!(
                "HTTP status client error (404 Not Found) for url ({url}?token=s3cret)"
            ))
        })
    }

    fn post_form_json(&self, url: &str, _form: &[(&str, String)]) -> Result<Value> {
        Err(Error::Config(format!("nothing posts to {url}")))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        Err(Error::Config(format!("nothing gets {url}")))
    }
}

fn recorded(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/discogs")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).expect("a recorded response");
    serde_json::from_str(&text).expect("valid JSON")
}

fn all_recorded() -> Recorded {
    Recorded::default()
        .with("1", recorded("stockholm"))
        .with("3", recorded("profound-sounds"))
        .with("3321212", recorded("trompeten-konzerte"))
}

fn write_collection(scratch: &Scratch, rows: &[&str]) -> PathBuf {
    let path = scratch.join("collection.csv");
    std::fs::write(&path, format!("{HEADER}\n{}\n", rows.join("\n"))).expect("a written export");
    path
}

const STOCKHOLM: &str =
    "SK001,The Persuader,Stockholm,Svek,\"2x12\"\", EP\",,1999,1,Uncategorized,2021-01-31 12:34:56,,,";
const PROFOUND: &str =
    "PRO-CD-001,Josh Wink,Profound Sounds Vol. 1,Ruffhouse,\"CD, Comp, Mixed\",4,1999,3,Uncategorized,2021-02-01 09:00:00,,,";
const TROMPETEN: &str =
    "EMI 1C 063,Maurice André,Trompeten-Konzerte,EMI,LP,,1972,3321212,Uncategorized,2021-02-02 10:00:00,,,";

fn discogs(scratch: &Scratch, token: Option<&str>) -> Discogs {
    Discogs::new(
        token.map(str::to_owned),
        0.0,
        scratch.join("cache").join("discogs.json"),
    )
}

fn read(path: &Path, http: &Recorded, discogs: &Discogs) -> Result<Vec<stk::models::Track>> {
    let lastfm = LastFm::new(None, 0.0);
    read_export(
        path,
        None,
        Some(&Remote {
            http,
            discogs,
            lastfm: &lastfm,
        }),
    )
}

#[test]
fn a_collection_export_is_recognised_by_its_release_id_column() {
    let scratch = Scratch::new("discogs-detect");
    let path = write_collection(&scratch, &[STOCKHOLM]);

    assert_eq!(Format::detect(&path).expect("a format"), Format::Discogs);
    assert_eq!("discogs".parse::<Format>(), Ok(Format::Discogs));
}

#[test]
fn every_track_of_a_release_is_wanted_under_the_release_artist() {
    let scratch = Scratch::new("discogs-album");
    let path = write_collection(&scratch, &[STOCKHOLM]);

    let tracks = read(&path, &all_recorded(), &discogs(&scratch, None)).expect("a collection");
    let queries: Vec<String> = tracks.iter().map(stk::models::Track::query).collect();
    assert_eq!(
        queries,
        [
            "The Persuader - Östermalm",
            "The Persuader - Vasastaden",
            "The Persuader - Kungsholmen",
            "The Persuader - Södermalm",
            "The Persuader - Norrmalm",
            "The Persuader - Gamla Stan",
        ]
    );
    assert_eq!(tracks[0].album, "Stockholm");
    assert!((tracks[0].duration_seconds.expect("a duration") - 285.0).abs() < 1e-9);
    assert!(!tracks[0].title_guessed);
}

#[test]
fn a_track_keeps_its_position_and_the_rows_own_columns() {
    let scratch = Scratch::new("discogs-row");
    let path = write_collection(&scratch, &[STOCKHOLM]);

    let track = &read(&path, &all_recorded(), &discogs(&scratch, None)).expect("a collection")[1];
    assert_eq!(track.source_value(TRACK_NUMBER_COLUMNS), Some("B1"));
    assert_eq!(track.source_value(&["album artist"]), Some("The Persuader"));
    assert_eq!(track.source_value(&["label"]), Some("Svek"));
    assert_eq!(track.source_value(&["release_id"]), Some("1"));
    // The release's own title never stands in for the track's.
    assert_eq!(track.source_value(&["title"]), Some("Vasastaden"));
    let titles = track
        .source_row
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("title"))
        .count();
    assert_eq!(titles, 1);
}

#[test]
fn a_compilation_credits_each_track_to_its_own_artists() {
    let scratch = Scratch::new("discogs-compilation");
    let path = write_collection(&scratch, &[PROFOUND]);

    let tracks = read(&path, &all_recorded(), &discogs(&scratch, None)).expect("a collection");
    let artists: Vec<&str> = tracks.iter().map(|track| track.artist.as_str()).collect();
    assert_eq!(artists[0], "Johannes Heil & Heiko Laux");
    assert_eq!(artists[3], "The Persuader");
    assert_eq!(tracks[0].source_value(&["album artist"]), Some("Josh Wink"));
}

#[test]
fn a_disambiguated_artist_is_searched_for_without_its_number() {
    let scratch = Scratch::new("discogs-disambiguation");
    let path = write_collection(&scratch, &[PROFOUND]);

    let tracks = read(&path, &all_recorded(), &discogs(&scratch, None)).expect("a collection");
    assert_eq!(tracks[4].query(), "Care Company - Vol. 2");
    assert_eq!(tracks[9].artist, "Stacey Pullen Presents Black Odyssey");
}

#[test]
fn the_movements_of_an_indexed_work_are_named_after_it() {
    let scratch = Scratch::new("discogs-index");
    let path = write_collection(&scratch, &[TROMPETEN]);

    let tracks = read(&path, &all_recorded(), &discogs(&scratch, None)).expect("a collection");
    let titles: Vec<&str> = tracks.iter().map(|track| track.title.as_str()).collect();
    assert_eq!(titles.len(), 10);
    assert_eq!(
        titles[0],
        "Konzert Für 2 Trompeten Und Orchester C-Dur: Allegro"
    );
    assert_eq!(
        titles[3],
        "Konzert Für Trompete Und Orchester D-Dur: Adagio"
    );
    assert_eq!(titles[7], "Trumpet Voluntary");
    assert_eq!(tracks[0].source_value(TRACK_NUMBER_COLUMNS), Some("A1a"));
    assert_eq!(tracks[0].duration_seconds, None);
}

#[test]
fn a_heading_is_not_a_track() {
    let scratch = Scratch::new("discogs-heading");
    let path = write_collection(&scratch, &["X1,Someone,Two Sides,Label,LP,,2000,42,,,,,"]);
    let http = Recorded::default().with(
        "42",
        json!({
            "title": "Two Sides",
            "artists": [{"name": "Someone", "join": ""}],
            "tracklist": [
                {"type_": "heading", "position": "", "title": "Side One", "duration": ""},
                {"type_": "track", "position": "A1", "title": "First", "duration": "3:00"},
            ],
        }),
    );

    let tracks = read(&path, &http, &discogs(&scratch, None)).expect("a collection");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].query(), "Someone - First");
}

#[test]
fn a_release_already_fetched_is_read_from_the_cache() {
    let scratch = Scratch::new("discogs-cache");
    let path = write_collection(&scratch, &[STOCKHOLM, PROFOUND]);
    let client = discogs(&scratch, None);

    let http = all_recorded();
    let first = read(&path, &http, &client).expect("a collection");
    assert_eq!(http.requested_urls().len(), 2);

    let offline = Recorded::default();
    let second = read(&path, &offline, &client).expect("a cached collection");
    assert!(offline.requested_urls().is_empty());
    assert_eq!(first, second);
}

#[test]
fn a_grown_collection_fetches_only_the_new_release() {
    let scratch = Scratch::new("discogs-cache-grown");
    let client = discogs(&scratch, None);
    read(
        &write_collection(&scratch, &[STOCKHOLM]),
        &all_recorded(),
        &client,
    )
    .expect("a collection");

    let http = all_recorded();
    read(
        &write_collection(&scratch, &[STOCKHOLM, TROMPETEN]),
        &http,
        &client,
    )
    .expect("a grown collection");
    assert_eq!(http.requested_urls(), [format!("{RELEASE_URL}/3321212")]);
}

#[test]
fn a_failed_lookup_names_the_release_and_keeps_what_came_before() {
    let scratch = Scratch::new("discogs-failure");
    let path = write_collection(&scratch, &[STOCKHOLM, "X,Gone,Deleted,,,,,999,,,,,"]);
    let client = discogs(&scratch, Some("s3cret"));

    let error = read(&path, &all_recorded(), &client).expect_err("a missing release");
    let message = error.to_string();
    assert!(message.contains("Discogs release 999"), "{message}");
    assert!(!message.contains("s3cret"), "{message}");

    let offline = Recorded::default();
    let resumed = read(&write_collection(&scratch, &[STOCKHOLM]), &offline, &client)
        .expect("the cached release");
    assert_eq!(resumed.len(), 6);
    assert!(offline.requested_urls().is_empty());
}

#[test]
fn the_token_is_sent_only_when_one_is_set() {
    let scratch = Scratch::new("discogs-token");
    let path = write_collection(&scratch, &[STOCKHOLM]);

    let anonymous = all_recorded();
    read(&path, &anonymous, &discogs(&scratch, None)).expect("a collection");
    assert!(anonymous.requested.borrow()[0].1.is_empty());

    let scratch = Scratch::new("discogs-token-set");
    let path = write_collection(&scratch, &[STOCKHOLM]);
    let authorised = all_recorded();
    read(&path, &authorised, &discogs(&scratch, Some("s3cret"))).expect("a collection");
    assert_eq!(
        authorised.requested.borrow()[0].1,
        [("token".to_owned(), "s3cret".to_owned())]
    );
}

#[test]
fn a_token_buys_the_faster_pace() {
    assert!((Discogs::min_interval_seconds(None) - MIN_INTERVAL_SECONDS).abs() < 1e-9);
    assert!(
        (Discogs::min_interval_seconds(Some("s3cret")) - MIN_INTERVAL_WITH_TOKEN_SECONDS).abs()
            < 1e-9
    );
    const { assert!(MIN_INTERVAL_SECONDS >= 60.0 / 25.0) };
    const { assert!(MIN_INTERVAL_WITH_TOKEN_SECONDS >= 1.0) };
}

#[test]
fn an_unreadable_cache_is_fetched_again_rather_than_refused() {
    let scratch = Scratch::new("discogs-cache-corrupt");
    let path = write_collection(&scratch, &[STOCKHOLM]);
    let client = discogs(&scratch, None);
    std::fs::create_dir_all(scratch.join("cache")).expect("a cache folder");
    std::fs::write(scratch.join("cache").join("discogs.json"), "{not json").expect("a cache");

    let http = all_recorded();
    assert_eq!(read(&path, &http, &client).expect("a collection").len(), 6);
    assert_eq!(http.requested_urls().len(), 1);
}

#[test]
fn a_collection_is_refused_without_the_api() {
    let scratch = Scratch::new("discogs-offline");
    let path = write_collection(&scratch, &[STOCKHOLM]);

    let error = read_export(&path, None, None).expect_err("no API to read through");
    assert!(error.to_string().contains("Discogs API"), "{error}");
}
