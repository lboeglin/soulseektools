//! Ported from `py/tests/unit/test_slskd.py`.
//!
//! The Python mocks `httpx` with `respx`. Here the client takes an `Http`
//! implementation, so a recorded set of responses substitutes for the daemon and
//! the requests it made can be inspected afterwards. What is under test either
//! way is the wire format -- slskd's quirks -- not the HTTP library.

use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::{json, Value};

use stk::config::SlskdSettings;
use stk::errors::{Error, Result};
use stk::soulseek::base::{SoulseekClient, TransferState};
use stk::soulseek::slskd::{state_from, Http, SlskdClient};

#[derive(Default)]
struct Recorded {
    responses: HashMap<String, Value>,
    failures: HashMap<String, String>,
    calls: RefCell<Vec<(String, String, Option<Value>)>>,
}

impl Recorded {
    fn with(mut self, method: &str, path: &str, body: Value) -> Self {
        self.responses.insert(format!("{method} {path}"), body);
        self
    }

    fn failing(mut self, method: &str, path: &str, message: &str) -> Self {
        self.failures
            .insert(format!("{method} {path}"), message.to_owned());
        self
    }

    fn calls_to(&self, method: &str, prefix: &str) -> Vec<(String, Option<Value>)> {
        self.calls
            .borrow()
            .iter()
            .filter(|(seen, path, _)| seen == method && path.starts_with(prefix))
            .map(|(_, path, body)| (path.clone(), body.clone()))
            .collect()
    }
}

impl Http for Recorded {
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Option<Value>> {
        self.calls
            .borrow_mut()
            .push((method.to_owned(), path.to_owned(), body.cloned()));

        let key = format!("{method} {path}");
        if let Some(message) = self.failures.get(&key) {
            return Err(Error::SoulseekClient(message.clone()));
        }
        // A search id is generated per call, so the routes that carry one are
        // matched by prefix the way `respx`'s url regexes are.
        for (route, response) in &self.responses {
            if route == &key || matches_search_route(route, &key) {
                return Ok(Some(response.clone()));
            }
        }
        Ok(None)
    }
}

/// `GET /searches/*/responses` against `GET /searches/<id>/responses`.
///
/// The wildcard stands for exactly one path segment. Letting it span a `/` would
/// make `/searches/*` also match `/searches/<id>/responses`, and which of the
/// two won would depend on hash order.
fn matches_search_route(route: &str, key: &str) -> bool {
    let Some((prefix, suffix)) = route.split_once('*') else {
        return false;
    };
    let Some(rest) = key.strip_prefix(prefix) else {
        return false;
    };
    let Some(middle) = rest.strip_suffix(suffix) else {
        return false;
    };
    !middle.is_empty() && !middle.contains('/')
}

fn client(http: Recorded) -> SlskdClient<Recorded> {
    SlskdClient::new(SlskdSettings::default(), http)
}

fn search_responses() -> Value {
    json!([
        {
            "username": "peerA",
            "hasFreeUploadSlot": true,
            "queueLength": 0,
            "files": [
                {
                    "filename": "@@a\\Music\\Roki.flac",
                    "size": 30_000_000,
                    "bitRate": 0,
                    "sampleRate": 44100,
                    "bitDepth": 16,
                    "isVariableBitRate": false,
                    "length": 215
                },
                {
                    "filename": "@@a\\Music\\Roki.mp3",
                    "size": 8_000_000,
                    "bitRate": 320,
                    "length": 215
                }
            ]
        },
        {
            "username": "peerB",
            "hasFreeUploadSlot": false,
            "queueLength": 12,
            "files": [
                {
                    "filename": "@@b\\Music\\Roki.mp3",
                    "size": 7_000_000,
                    "bitRate": 256,
                    "isVariableBitRate": true,
                    "length": 215
                }
            ]
        }
    ])
}

fn searching() -> Recorded {
    Recorded::default()
        .with("POST", "/searches", json!({"id": "search"}))
        .with("GET", "/searches/*", json!({"isComplete": true}))
        .with("GET", "/searches/*/responses", search_responses())
}

fn downloads(body: Value) -> Recorded {
    Recorded::default().with("GET", "/transfers/downloads", body)
}

#[test]
fn true_when_logged_in() {
    let http = Recorded::default().with(
        "GET",
        "/server",
        json!({"isConnected": true, "isLoggedIn": true}),
    );
    assert!(client(http).is_connected());
}

#[test]
fn false_when_connected_but_not_logged_in() {
    let http = Recorded::default().with(
        "GET",
        "/server",
        json!({"isConnected": true, "isLoggedIn": false}),
    );
    assert!(!client(http).is_connected());
}

#[test]
fn false_when_the_daemon_is_unreachable() {
    let http = Recorded::default().failing("GET", "/server", "cannot reach slskd");
    assert!(!client(http).is_connected());
}

#[test]
fn files_from_every_peer_are_returned() {
    let candidates = client(searching())
        .search("DECO*27 Roki", 15, 200)
        .expect("a search");

    assert_eq!(candidates.len(), 3);
    let peers: std::collections::HashSet<&str> = candidates
        .iter()
        .map(|candidate| candidate.username.as_str())
        .collect();
    assert_eq!(peers, ["peerA", "peerB"].into_iter().collect());
}

#[test]
fn file_attributes_are_carried_through() {
    let candidates = client(searching()).search("x", 15, 200).expect("a search");
    let flac = candidates
        .iter()
        .find(|candidate| candidate.extension() == "flac")
        .expect("a flac result");

    assert_eq!(flac.size, 30_000_000);
    assert_eq!(flac.bit_depth, Some(16));
    assert_eq!(flac.sample_rate, Some(44100));
    assert!((flac.duration_seconds.expect("a duration") - 215.0).abs() < 1e-9);
    assert!(!flac.is_vbr);
}

#[test]
fn peer_availability_is_carried_through() {
    let candidates = client(searching()).search("x", 15, 200).expect("a search");
    let by_peer: HashMap<&str, _> = candidates
        .iter()
        .map(|candidate| (candidate.username.as_str(), candidate))
        .collect();

    assert!(by_peer["peerA"].free_upload_slots);
    assert!(!by_peer["peerB"].free_upload_slots);
    assert_eq!(by_peer["peerB"].queue_length, 12);
    assert!(by_peer["peerB"].is_vbr);
}

#[test]
fn a_zero_bitrate_is_treated_as_unknown_not_as_zero() {
    let candidates = client(searching()).search("x", 15, 200).expect("a search");
    let flac = candidates
        .iter()
        .find(|candidate| candidate.extension() == "flac")
        .expect("a flac result");

    assert_eq!(flac.bitrate, None);
}

#[test]
fn the_search_text_and_timeout_are_sent() {
    let client = SlskdClient::new(SlskdSettings::default(), searching());
    client.search("DECO*27 Roki", 20, 200).expect("a search");

    let recorder = client.into_http();
    let body = recorder.calls_to("POST", "/searches")[0]
        .1
        .clone()
        .expect("a body");
    assert_eq!(body["searchText"], "DECO*27 Roki");
    assert_eq!(body["searchTimeout"], 20);
}

/// The key is read from the environment, never from the config file, and this
/// is the only place it reaches the wire.
#[test]
fn the_api_key_is_sent_as_a_header() {
    let settings = SlskdSettings {
        api_key: Some("secret".to_owned()),
        ..SlskdSettings::default()
    };
    let headers = stk::soulseek::slskd::headers_for(&settings);

    assert!(headers.contains(&("X-API-Key".to_owned(), "secret".to_owned())));
}

#[test]
fn no_header_is_sent_when_no_key_is_configured() {
    let headers = stk::soulseek::slskd::headers_for(&SlskdSettings::default());
    assert!(headers.iter().all(|(name, _)| name != "X-API-Key"));
}

#[test]
fn a_timeout_below_the_daemon_minimum_is_raised_to_it() {
    let client = SlskdClient::new(SlskdSettings::default(), searching());
    client.search("x", 1, 200).expect("a search");

    let recorder = client.into_http();
    let body = recorder.calls_to("POST", "/searches")[0]
        .1
        .clone()
        .expect("a body");
    assert_eq!(body["searchTimeout"], 5);
}

#[test]
fn the_search_is_cleaned_up_afterwards() {
    let client = SlskdClient::new(SlskdSettings::default(), searching());
    client.search("x", 15, 200).expect("a search");

    let recorder = client.into_http();
    assert_eq!(recorder.calls_to("DELETE", "/searches/").len(), 1);
}

#[test]
fn a_failure_to_clean_up_does_not_lose_the_results() {
    let http = searching().failing("DELETE", "/searches/search", "gone");
    assert_eq!(
        client(http).search("x", 15, 200).expect("a search").len(),
        3
    );
}

#[test]
fn no_responses_yields_no_candidates() {
    let http = Recorded::default()
        .with("POST", "/searches", json!({}))
        .with("GET", "/searches/*", json!({"isComplete": true}))
        .with("GET", "/searches/*/responses", json!([]));

    assert!(client(http)
        .search("x", 15, 200)
        .expect("a search")
        .is_empty());
}

#[test]
fn a_response_with_no_files_is_skipped() {
    let http = Recorded::default()
        .with("POST", "/searches", json!({}))
        .with("GET", "/searches/*", json!({"isComplete": true}))
        .with(
            "GET",
            "/searches/*/responses",
            json!([{"username": "peerC", "files": []}]),
        );

    assert!(client(http)
        .search("x", 15, 200)
        .expect("a search")
        .is_empty());
}

#[test]
fn an_unreachable_daemon_is_reported() {
    let http = Recorded::default().failing("POST", "/searches", "cannot reach slskd at http://x");
    let error = client(http)
        .search("x", 15, 200)
        .expect_err("an unreachable daemon");
    assert!(error.to_string().contains("cannot reach slskd"), "{error}");
}

#[test]
fn a_destination_uses_the_batch_endpoint() {
    let http = Recorded::default().with(
        "POST",
        "/transfers/downloads/batches",
        json!({
            "batch": {
                "transfers": [
                    {"id": "t1", "username": "peerA", "filename": "f", "state": "Queued, Remotely"}
                ]
            }
        }),
    );
    let client = SlskdClient::new(SlskdSettings::default(), http);

    let transfers = client
        .enqueue(
            "peerA",
            &[stk::models::Candidate {
                username: "peerA".to_owned(),
                remote_path: "f".to_owned(),
                size: 1,
                ..stk::models::Candidate::default()
            }],
            "Artist",
        )
        .expect("an enqueue");

    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].state, TransferState::Queued);

    let recorder = client.into_http();
    let body = recorder.calls_to("POST", "/transfers/downloads/batches")[0]
        .1
        .clone()
        .expect("a body");
    assert_eq!(body["options"]["destination"], "Artist");
}

#[test]
fn a_batch_failure_is_raised_with_its_reason() {
    let http = Recorded::default().with(
        "POST",
        "/transfers/downloads/batches",
        json!({"failures": [{"filename": "f", "message": "peer offline"}]}),
    );

    let error = client(http)
        .enqueue(
            "peerA",
            &[stk::models::Candidate {
                remote_path: "f".to_owned(),
                ..stk::models::Candidate::default()
            }],
            "Artist",
        )
        .expect_err("a batch failure");

    assert!(error.to_string().contains("peer offline"), "{error}");
}

#[test]
fn without_a_destination_the_per_user_endpoint_is_used() {
    let http = downloads(json!([
        {
            "username": "peerA",
            "directories": [{"files": [
                {"id": "t1", "filename": "f", "state": "Completed, Succeeded"}
            ]}]
        }
    ]));
    let client = SlskdClient::new(SlskdSettings::default(), http);

    let transfers = client
        .enqueue(
            "peerA",
            &[stk::models::Candidate {
                remote_path: "f".to_owned(),
                ..stk::models::Candidate::default()
            }],
            "",
        )
        .expect("an enqueue");

    assert_eq!(transfers.len(), 1);
    let recorder = client.into_http();
    assert_eq!(
        recorder
            .calls_to("POST", "/transfers/downloads/peerA")
            .len(),
        1
    );
}

#[test]
fn enqueueing_nothing_makes_no_request() {
    let client = SlskdClient::new(SlskdSettings::default(), Recorded::default());
    assert!(client
        .enqueue("peerA", &[], "")
        .expect("no enqueue")
        .is_empty());

    let recorder = client.into_http();
    assert!(recorder.calls.borrow().is_empty());
}

#[test]
fn transfers_are_flattened_across_users_and_folders() {
    let http = downloads(json!([
        {
            "username": "peerA",
            "directories": [
                {"files": [{"id": "t1", "filename": "a", "state": "Completed, Succeeded"}]},
                {"files": [{"id": "t2", "filename": "b", "state": "InProgress"}]}
            ]
        },
        {
            "username": "peerB",
            "directories": [{"files": [{"id": "t3", "filename": "c", "state": "Queued"}]}]
        }
    ]));

    let transfers = client(http).transfers().expect("transfers");

    assert_eq!(transfers.len(), 3);
    assert_eq!(transfers[0].username, "peerA");
    assert_eq!(transfers[2].username, "peerB");
}

#[test]
fn progress_is_reported() {
    let http = downloads(json!([
        {
            "username": "p",
            "directories": [{"files": [
                {
                    "id": "t1", "filename": "a", "state": "Completed, Succeeded",
                    "size": 1000, "bytesTransferred": 1000
                },
                {
                    "id": "t2", "filename": "b", "state": "InProgress",
                    "size": 1000, "bytesTransferred": 250, "placeInQueue": 3
                }
            ]}]
        }
    ]));

    let transfers = client(http).transfers().expect("transfers");
    let by_id: HashMap<&str, _> = transfers
        .iter()
        .map(|transfer| (transfer.id.as_str(), transfer))
        .collect();

    assert!((by_id["t1"].percent_complete() - 100.0).abs() < 1e-9);
    assert!((by_id["t2"].percent_complete() - 25.0).abs() < 1e-9);
    assert_eq!(by_id["t2"].place_in_queue, Some(3));
}

#[test]
fn soulseek_state_flags_collapse_to_one_outcome() {
    // slskd passes through Soulseek.NET's flag-like state strings; a "Completed"
    // that errored is a failure, not a completion.
    for (raw, expected) in [
        ("Completed, Succeeded", TransferState::Completed),
        ("Completed, Errored", TransferState::Failed),
        ("Completed, Cancelled", TransferState::Cancelled),
        ("Completed, TimedOut", TransferState::Failed),
        ("Completed, Rejected", TransferState::Failed),
        ("InProgress", TransferState::InProgress),
        ("Queued, Remotely", TransferState::Queued),
        ("Requested", TransferState::Queued),
        ("", TransferState::Queued),
    ] {
        assert_eq!(state_from(raw), expected, "{raw:?}");

        let http = downloads(json!([
            {"username": "p", "directories": [{"files": [
                {"id": "t", "filename": "f", "state": raw}
            ]}]}
        ]));
        assert_eq!(
            client(http).transfers().expect("transfers")[0].state,
            expected,
            "{raw:?}"
        );
    }
}

#[test]
fn finished_states_are_identified() {
    assert!(TransferState::Completed.finished());
    assert!(TransferState::Failed.finished());
    assert!(TransferState::Cancelled.finished());
    assert!(!TransferState::Queued.finished());
    assert!(!TransferState::InProgress.finished());
}

#[test]
fn no_downloads_yields_nothing() {
    assert!(client(downloads(json!([])))
        .transfers()
        .expect("transfers")
        .is_empty());
}
