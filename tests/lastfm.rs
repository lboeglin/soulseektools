//! Reading a Last.fm profile by username, against canned responses.
//!
//! Last.fm answers nothing without an API key, so the bodies under
//! `tests/data/lastfm` are written to the shape its API documentation gives
//! rather than recorded: loved tracks over two pages, the second holding one
//! track and so written as an object rather than an array, the way Last.fm
//! writes a list of one; two pages of top tracks, most played first; and an
//! error.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use stk::errors::{Error, Result};
use stk::models::Track;
use stk::services::ServiceHttp;
use stk::wantlist::lastfm::API_URL;
use stk::wantlist::{read_source, Discogs, Format, LastFm, Remote, Source};
use support::Scratch;

/// A transport answering each method and page with a canned body.
#[derive(Default)]
struct Canned {
    bodies: HashMap<(String, String), Value>,
    failure: Option<String>,
    requested: RefCell<Vec<HashMap<String, String>>>,
}

impl Canned {
    fn with(mut self, method: &str, page: u32, name: &str) -> Self {
        self.bodies
            .insert((method.to_owned(), page.to_string()), canned(name));
        self
    }

    fn profile() -> Self {
        Self::default()
            .with("user.getlovedtracks", 1, "loved-page-1")
            .with("user.getlovedtracks", 2, "loved-page-2")
            .with("user.gettoptracks", 1, "top-page-1")
            .with("user.gettoptracks", 2, "top-page-2")
    }

    fn methods(&self) -> Vec<(String, String)> {
        self.requested
            .borrow()
            .iter()
            .map(|params| (params["method"].clone(), params["page"].clone()))
            .collect()
    }
}

impl ServiceHttp for Canned {
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value> {
        assert_eq!(url, API_URL);
        let params: HashMap<String, String> = params
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();
        self.requested.borrow_mut().push(params.clone());

        if let Some(message) = &self.failure {
            return Err(Error::Config(message.clone()));
        }
        let key = (params["method"].clone(), params["page"].clone());
        self.bodies
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::Config(format!("nothing canned for {key:?}")))
    }

    fn post_form_json(&self, url: &str, _form: &[(&str, String)]) -> Result<Value> {
        Err(Error::Config(format!("nothing posts to {url}")))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        Err(Error::Config(format!("nothing gets {url}")))
    }
}

fn canned(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/lastfm")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).expect("a canned response");
    serde_json::from_str(&text).expect("valid JSON")
}

fn read(
    http: &Canned,
    api_key: Option<&str>,
    source: &str,
    format: Option<Format>,
    min_plays: Option<usize>,
) -> Result<Vec<Track>> {
    let scratch = Scratch::new("lastfm-read");
    let discogs = Discogs::new(None, 0.0, scratch.join("discogs.json"));
    let lastfm = LastFm::new(api_key.map(str::to_owned), 0.0);
    let source: Source = source.parse().expect("a source");
    read_source(
        &source,
        format,
        min_plays,
        &Remote {
            http,
            discogs: &discogs,
            lastfm: &lastfm,
        },
    )
}

#[test]
fn a_lastfm_source_names_a_user_and_anything_else_a_file() {
    assert_eq!("lastfm:rj".parse(), Ok(Source::LastFm("rj".to_owned())));
    assert_eq!(
        r"C:\exports\library.csv".parse(),
        Ok(Source::File(PathBuf::from(r"C:\exports\library.csv")))
    );
    assert!("lastfm:".parse::<Source>().is_err());
    assert!("lastfm:  ".parse::<Source>().is_err());
}

#[test]
fn every_loved_track_is_wanted_across_pages() {
    let http = Canned::profile();

    let tracks = read(&http, Some("k3y"), "lastfm:rj", None, None).expect("loved tracks");
    let queries: Vec<String> = tracks.iter().map(Track::query).collect();
    assert_eq!(
        queries,
        [
            "DECO*27 - Rabbit Hole",
            "Ado - うっせぇわ",
            "YOASOBI - 夜に駆ける"
        ]
    );
    assert_eq!(
        http.methods(),
        [
            ("user.getlovedtracks".to_owned(), "1".to_owned()),
            ("user.getlovedtracks".to_owned(), "2".to_owned()),
        ]
    );
    assert_eq!(
        tracks[0].source_value(&["loved"]),
        Some("31 Jan 2021, 12:34")
    );
    assert!(!tracks[0].title_guessed);
}

#[test]
fn min_plays_adds_the_tracks_played_that_often_and_stops_below_it() {
    let http = Canned::profile();

    let tracks = read(&http, Some("k3y"), "lastfm:rj", None, Some(10)).expect("a profile");
    let queries: Vec<String> = tracks.iter().map(Track::query).collect();
    assert_eq!(
        queries,
        [
            "DECO*27 - Rabbit Hole",
            "Ado - うっせぇわ",
            "YOASOBI - 夜に駆ける",
            "DECO*27 - Ghost Rule",
        ]
    );
    // Forty pages of top tracks, and the one read was enough: its last track
    // was played fewer times than asked.
    assert_eq!(
        http.methods().last(),
        Some(&("user.gettoptracks".to_owned(), "1".to_owned()))
    );
    assert_eq!(http.methods().len(), 3);
}

#[test]
fn a_loved_track_also_played_often_is_listed_once_with_its_playcount() {
    let http = Canned::profile();

    let tracks = read(&http, Some("k3y"), "lastfm:rj", None, Some(10)).expect("a profile");
    let rabbit: Vec<&Track> = tracks
        .iter()
        .filter(|track| track.title == "Rabbit Hole")
        .collect();
    assert_eq!(rabbit.len(), 1);
    assert_eq!(rabbit[0].source_value(&["playcount"]), Some("212"));
    assert_eq!(
        rabbit[0].source_value(&["loved"]),
        Some("31 Jan 2021, 12:34")
    );
}

#[test]
fn an_error_from_last_fm_is_reported_in_its_own_words() {
    let http = Canned::default().with("user.getlovedtracks", 1, "user-not-found");

    let error = read(&http, Some("k3y"), "lastfm:nobody", None, None).expect_err("no user");
    assert_eq!(error.to_string(), "Last.fm: User not found");
}

#[test]
fn a_failed_request_never_quotes_the_key() {
    let http = Canned {
        failure: Some(format!(
            "HTTP status client error (403 Forbidden) for url ({API_URL}?api_key=k3y)"
        )),
        ..Canned::default()
    };

    let error = read(&http, Some("k3y"), "lastfm:rj", None, None).expect_err("a failure");
    assert!(!error.to_string().contains("k3y"), "{error}");
    assert!(error.to_string().contains("403"), "{error}");
}
