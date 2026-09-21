//! The AcoustID lookup and the release choice, against recorded responses.
//!
//! Every response under `tests/data/acoustid` was recorded from the live
//! service: a print with one recording, one linked to a cover as well as the
//! original and spread over a decade of compilations, a featured credit with a
//! joinphrase, an unknown print, and a refused key. The rules in
//! `identify::choose` are checked against those, so a test that fails here is
//! a rule that would misfile a real library.

mod support;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use stk::audio::fingerprint::Fingerprint;
use stk::config::IdentifySettings;
use stk::errors::{Error, Result};
use stk::identify::acoustid::{credited, AcoustId, Credit, LOOKUP_URL};
use stk::identify::{choose, edits, identify, Identification, Match, Outcome};
use stk::library::tags::TagField;
use stk::matching::NameForms;
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags};
use stk::services::ServiceHttp;
use stk::tagsheet::Cell;

/// A transport answering every lookup with one recorded body.
struct Recorded {
    body: Result<Value>,
    posted: RefCell<Vec<Vec<(String, String)>>>,
}

impl Recorded {
    fn answering(body: Value) -> Self {
        Self {
            body: Ok(body),
            posted: RefCell::new(Vec::new()),
        }
    }

    fn failing(message: &str) -> Self {
        Self {
            body: Err(Error::Config(message.to_owned())),
            posted: RefCell::new(Vec::new()),
        }
    }

    fn form(&self, name: &str) -> Option<String> {
        self.posted
            .borrow()
            .first()?
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
    }
}

impl ServiceHttp for Recorded {
    fn get_json(&self, url: &str, _params: &[(&str, String)]) -> Result<Value> {
        Err(Error::Config(format!("nothing here gets {url}")))
    }

    fn post_form_json(&self, url: &str, form: &[(&str, String)]) -> Result<Value> {
        assert_eq!(url, LOOKUP_URL);
        self.posted.borrow_mut().push(
            form.iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        );
        match &self.body {
            Ok(body) => Ok(body.clone()),
            Err(error) => Err(Error::Config(error.to_string())),
        }
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        Err(Error::Config(format!("nothing here gets {url}")))
    }
}

fn recorded(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/acoustid")
        .join(name);
    let text = std::fs::read_to_string(&path).expect("a recorded response");
    serde_json::from_str(&text).expect("valid JSON")
}

fn print(duration_seconds: Option<f64>) -> Fingerprint {
    Fingerprint {
        values: (0..200_u32)
            .map(|value| value.wrapping_mul(2_654_435_761))
            .collect(),
        duration_seconds,
    }
}

/// An indexed file under `Music/<artist>/...`, resolved the way `describe`
/// resolves an untagged one: the artist is the folder below the root.
fn entry(path: &str, album: &str) -> LibraryEntry {
    let path = PathBuf::from(path);
    let folder_artist = path
        .components()
        .nth(1)
        .map(|name| name.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    LibraryEntry {
        artist: NameForms::of(&folder_artist),
        title: NameForms::of(""),
        folder_artist,
        path,
        size: 1,
        mtime: 1.0,
        tags: TrackTags {
            album: album.to_owned(),
            genres: vec!["Eurobeat".to_owned()],
            ..TrackTags::default()
        },
        duration_seconds: Some(261.0),
        artwork_bytes: 0,
        codec: String::new(),
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
        replay_gain: ReplayGainTags::default(),
    }
}

fn settings() -> IdentifySettings {
    IdentifySettings::default()
}

fn lookup(name: &str) -> Vec<stk::identify::acoustid::Hit> {
    AcoustId::new(0.0)
        .lookup(&Recorded::answering(recorded(name)), &print(Some(261.0)))
        .expect("a parsed answer")
}

fn matched(outcome: Outcome) -> Match {
    match outcome {
        Outcome::Match(found) => found,
        other => panic!("expected a match, got {other:?}"),
    }
}

#[test]
fn the_request_carries_the_key_the_whole_seconds_and_the_compressed_print() {
    let http = Recorded::answering(recorded("no-match.json"));
    let fingerprint = print(Some(261.4));

    AcoustId::new(0.0)
        .lookup(&http, &fingerprint)
        .expect("an answer");

    assert!(!http.form("client").expect("a client key").is_empty());
    assert_eq!(http.form("duration").as_deref(), Some("261"));
    assert_eq!(http.form("fingerprint"), Some(fingerprint.compressed()));
    assert!(http.form("meta").expect("meta").contains("releases"));
}

#[test]
fn a_print_without_a_duration_is_refused_before_any_request() {
    let http = Recorded::answering(recorded("no-match.json"));

    let error = AcoustId::new(0.0)
        .lookup(&http, &print(None))
        .expect_err("no duration");

    assert!(error.to_string().contains("duration"), "{error}");
    assert!(http.posted.borrow().is_empty());
}

#[test]
fn a_refused_key_is_reported_with_the_services_message() {
    let error = AcoustId::new(0.0)
        .lookup(
            &Recorded::answering(recorded("invalid-key.json")),
            &print(Some(8.0)),
        )
        .expect_err("an error status");

    assert!(error.to_string().contains("invalid API key"), "{error}");
}

#[test]
fn hits_come_back_best_first() {
    let hits = lookup("deja-vu.json");

    assert_eq!(hits.len(), 2);
    assert!(hits[0].score > hits[1].score);
}

#[test]
fn a_compressed_answer_falls_back_to_its_parents_values() {
    // Under `compress` a release with no title has its group's, and a track
    // with no title or artists has its recording's.
    let hits = lookup("deja-vu.json");
    let original = hits[0]
        .recordings
        .iter()
        .find(|recording| recording.id.starts_with("b5ccfa0f"))
        .expect("the Dave Rodgers recording");
    let euro_best = original
        .releases
        .iter()
        .find(|release| release.id.starts_with("94532064"))
        .expect("the 2000 compilation");

    assert_eq!(
        euro_best.title,
        "Super Eurobeat Presents Initial D Super Euro-Best"
    );
    assert_eq!(credited(&euro_best.artists), "Various Artists");
    let track = euro_best.track.as_ref().expect("a track");
    assert_eq!(track.title, "Deja Vu");
    assert_eq!(track.position, Some(8));
}

#[test]
fn a_joinphrase_joins_a_credit_as_it_is_written() {
    assert_eq!(
        credited(&[
            Credit {
                name: "Anamanaguchi".to_owned(),
                joinphrase: " feat. ".to_owned()
            },
            Credit {
                name: "初音ミク".to_owned(),
                joinphrase: String::new()
            },
        ]),
        "Anamanaguchi feat. 初音ミク"
    );
}

#[test]
fn an_unknown_print_is_no_match() {
    let outcome = choose(
        &lookup("no-match.json"),
        &entry("Music/X/a.flac", ""),
        &settings(),
    );

    assert_eq!(outcome, Outcome::NoMatch("unknown to AcoustID".to_owned()));
}

#[test]
fn below_the_threshold_nothing_is_proposed_and_the_best_score_is_reported() {
    let outcome = choose(
        &lookup("ghost-rule.json"),
        &entry("Music/DECO_27/00 - DECO_27 - Ghost Rule.flac", ""),
        &IdentifySettings {
            min_score: 0.99,
            ..settings()
        },
    );

    assert_eq!(
        outcome,
        Outcome::NoMatch("no confident match (best 0.98)".to_owned())
    );
}

#[test]
fn a_known_print_with_no_release_linked_is_no_match() {
    let hits = AcoustId::new(0.0)
        .lookup(
            &Recorded::answering(json!({"status": "ok", "results": [{"id": "x", "score": 0.99}]})),
            &print(Some(261.0)),
        )
        .expect("an answer");

    let outcome = choose(&hits, &entry("Music/X/a.flac", ""), &settings());

    assert!(
        matches!(outcome, Outcome::NoMatch(reason) if reason.contains("no MusicBrainz release"))
    );
}

#[test]
fn a_release_matching_the_album_tag_is_chosen_over_an_earlier_one() {
    let found = matched(choose(
        &lookup("deja-vu.json"),
        &entry(
            "Music/Dave Rodgers/05.flac",
            "Super Eurobeat Presents Dave Rodgers Special Collection Vol. 2",
        ),
        &settings(),
    ));

    assert!(
        found.release_id.starts_with("cb12fe02"),
        "{}",
        found.release_id
    );
    assert_eq!(found.proposed.title, "Deja Vu (extended version)");
    assert_eq!(found.proposed.track_number, Some(5));
    assert_eq!(found.reason, "the release matches the file's album tag");
}

#[test]
fn a_release_matching_the_folder_name_is_chosen_over_an_earlier_one() {
    let found = matched(choose(
        &lookup("deja-vu.json"),
        &entry(
            "Music/Dave Rodgers/Super Eurobeat Presents Initial D World Hits Selection 3/01.flac",
            "",
        ),
        &settings(),
    ));

    assert!(
        found.release_id.starts_with("3c61e397"),
        "{}",
        found.release_id
    );
    assert_eq!(found.year, Some(2020));
    assert_eq!(found.reason, "the release matches the folder name");
}

#[test]
fn the_recording_credited_to_the_files_artist_beats_a_cover() {
    // One print, three recordings: the original twice over and a cover by
    // someone else. The folder says which was meant.
    let found = matched(choose(
        &lookup("deja-vu.json"),
        &entry("Music/Tsuko G./Deja Vu.flac", ""),
        &settings(),
    ));

    assert_eq!(found.proposed.artist, "Tsuko G.");
    assert_eq!(found.proposed.title, "Deja Vu (Initial D)");
    assert_eq!(
        found.reason,
        "the recording is credited to the file's artist"
    );
}

#[test]
fn otherwise_the_earliest_release_wins() {
    let found = matched(choose(
        &lookup("deja-vu.json"),
        &entry("Music/Dave Rodgers/1.01. DEJA VU.flac", ""),
        &settings(),
    ));

    assert!(
        found.release_id.starts_with("94532064"),
        "{}",
        found.release_id
    );
    assert_eq!(found.year, Some(2000));
    assert_eq!(found.reason, "the earliest of 7 releases (2000-11-22)");
}

#[test]
fn a_compilation_gives_the_release_artist_beside_the_recordings() {
    // The hazard from the issue: with the two the wrong way round, `organise`
    // files everything under Various Artists.
    let found = matched(choose(
        &lookup("deja-vu.json"),
        &entry("Music/Dave Rodgers/1.01. DEJA VU.flac", ""),
        &settings(),
    ));

    assert_eq!(found.proposed.artist, "Dave Rodgers");
    assert_eq!(found.proposed.album_artist, "Various Artists");
    assert_eq!(
        found.proposed.album,
        "Super Eurobeat Presents Initial D Super Euro-Best"
    );
    assert_eq!(found.proposed.track_number, Some(8));
}

#[test]
fn the_original_is_chosen_over_a_compilation_released_the_same_day() {
    // GHOST and a compilation carrying the same track both came out on
    // 2016-09-28; the album wins, and the choice is stable across runs.
    let found = matched(choose(
        &lookup("ghost-rule.json"),
        &entry("Music/DECO_27/00 - DECO_27 - Ghost Rule.flac", ""),
        &settings(),
    ));

    assert_eq!(found.proposed.album, "GHOST");
    assert_eq!(found.proposed.artist, "DECO*27");
    assert_eq!(found.proposed.title, "ゴーストルール");
    assert!(
        found.release_id.starts_with("3ab7f368"),
        "{}",
        found.release_id
    );
    assert_eq!(
        found.reason,
        "one of 5 releases dated 2016-09-28, taken by id"
    );
}

#[test]
fn the_reason_names_the_compilation_rule_when_that_is_what_decided() {
    let hits = AcoustId::new(0.0)
        .lookup(
            &Recorded::answering(json!({"status": "ok", "results": [{
                "id": "x", "score": 0.99,
                "recordings": [{
                    "id": "r", "title": "Roki", "artists": [{"name": "DECO*27"}],
                    "releasegroups": [
                        {"id": "g1", "title": "Best Of", "type": "Album",
                         "secondarytypes": ["Compilation"],
                         "artists": [{"name": "Various Artists"}],
                         "releases": [{"id": "a", "date": {"year": 2016},
                                       "mediums": [{"tracks": [{"position": 3}]}]}]},
                        {"id": "g2", "title": "Ghost", "type": "Album",
                         "releases": [{"id": "b", "date": {"year": 2016},
                                       "mediums": [{"tracks": [{"position": 1}]}]}]}
                    ]
                }]
            }]})),
            &print(Some(261.0)),
        )
        .expect("an answer");

    let found = matched(choose(
        &hits,
        &entry("Music/DECO_27/a.flac", ""),
        &settings(),
    ));

    assert_eq!(found.proposed.album, "Ghost");
    assert_eq!(found.reason, "the original over a compilation, both 2016");
}

#[test]
fn a_match_credited_to_someone_else_says_so() {
    // With --all a tagged file is looked up too, and one print linked only to
    // a cover is usually the cover; the plan says so rather than refusing.
    let found = matched(choose(
        &lookup("deja-vu.json"),
        &entry("Music/Someone Else/Deja Vu.flac", ""),
        &settings(),
    ));

    assert!(
        found.reason.ends_with("; not credited to Someone Else"),
        "{}",
        found.reason
    );
}

#[test]
fn a_solo_release_leaves_the_album_artist_empty() {
    let found = matched(choose(
        &lookup("ghost-rule.json"),
        &entry("Music/DECO_27/00 - DECO_27 - Ghost Rule.flac", ""),
        &settings(),
    ));

    assert_eq!(found.proposed.album_artist, "");
}

#[test]
fn a_featured_credit_is_written_with_its_joinphrase() {
    let found = matched(choose(
        &lookup("miku.json"),
        &entry("Music/Anamanaguchi/Miku.flac", ""),
        &settings(),
    ));

    assert_eq!(found.proposed.artist, "Anamanaguchi feat. 初音ミク");
    assert_eq!(found.proposed.album, "HATSUNE MIKU EXPO 2016 E.P.");
    assert_eq!(found.proposed.album_artist, "初音ミク");
    assert_eq!(found.proposed.track_number, Some(11));
    assert!(
        found.reason.starts_with("the earliest of"),
        "{}",
        found.reason
    );
}

#[test]
fn an_undated_release_sorts_after_every_dated_one() {
    // The EXPO E.P. is on MusicBrainz twice, once with no date; the dated
    // copy is the one chosen.
    let found = matched(choose(
        &lookup("miku.json"),
        &entry("Music/Anamanaguchi/Miku.flac", ""),
        &settings(),
    ));

    assert!(
        found.release_id.starts_with("cb3a030e"),
        "{}",
        found.release_id
    );
}

#[test]
fn genres_are_carried_over_untouched() {
    let found = matched(choose(
        &lookup("ghost-rule.json"),
        &entry("Music/DECO_27/00 - DECO_27 - Ghost Rule.flac", ""),
        &settings(),
    ));

    assert_eq!(found.proposed.genres, ["Eurobeat"]);
}

#[test]
fn identify_reports_a_failed_print_and_a_failed_request_against_their_files() {
    let entries = [entry("Music/X/a.flac", ""), entry("Music/X/b.flac", "")];
    let prints = vec![
        Err(Error::Audio("no decoder".to_owned())),
        Ok(print(Some(261.0))),
    ];

    let results = identify(
        &Recorded::failing("503 Service Unavailable"),
        &AcoustId::new(0.0),
        &entries,
        prints,
        &settings(),
    );

    assert!(
        matches!(&results[0].outcome, Outcome::Failed(reason) if reason.contains("no decoder"))
    );
    assert!(matches!(&results[1].outcome, Outcome::Failed(reason) if reason.contains("503")));
}

#[test]
fn identify_matches_each_print_to_its_own_entry() {
    let entries = [entry("Music/DECO_27/00 - DECO_27 - Ghost Rule.flac", "")];
    let results = identify(
        &Recorded::answering(recorded("ghost-rule.json")),
        &AcoustId::new(0.0),
        &entries,
        vec![Ok(print(Some(210.0)))],
        &settings(),
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.path, entries[0].path);
    assert!(matches!(results[0].outcome, Outcome::Match(_)));
}

#[test]
fn edits_set_what_the_release_says_and_leave_what_it_cannot() {
    let found = matched(choose(
        &lookup("ghost-rule.json"),
        &entry("Music/DECO_27/00 - DECO_27 - Ghost Rule.flac", ""),
        &settings(),
    ));
    let identifications = [
        Identification {
            entry: entry("Music/DECO_27/00 - DECO_27 - Ghost Rule.flac", ""),
            outcome: Outcome::Match(found),
        },
        Identification {
            entry: entry("Music/X/b.flac", ""),
            outcome: Outcome::NoMatch("unknown to AcoustID".to_owned()),
        },
    ];

    let edits = edits(&identifications);

    assert_eq!(edits.len(), 1);
    let cell = |field: TagField| {
        edits[0]
            .cells
            .iter()
            .find(|(candidate, _)| *candidate == field)
            .map(|(_, cell)| cell.clone())
            .expect("a cell for the field")
    };
    assert_eq!(cell(TagField::Artist), Cell::Set("DECO*27".to_owned()));
    assert_eq!(cell(TagField::Album), Cell::Set("GHOST".to_owned()));
    assert_eq!(cell(TagField::TrackNumber), Cell::Set("1".to_owned()));
    // A solo album names no separate album artist; the file's is left alone
    // rather than blanked.
    assert_eq!(cell(TagField::AlbumArtist), Cell::Untouched);
    assert!(edits[0]
        .cells
        .iter()
        .all(|(field, _)| *field != TagField::Genres));
}

#[test]
fn acoustid_uses_its_published_three_per_second_limit() {
    assert!((stk::identify::acoustid::MIN_INTERVAL_SECONDS - 1.0 / 3.0).abs() < f64::EPSILON);
}
