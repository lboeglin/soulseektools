//! The slskd wire format, against recorded responses in place of the daemon.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use stk::config::SlskdSettings;
use stk::errors::{Error, Result};
use stk::soulseek::base::{SoulseekClient, TransferState};
use stk::soulseek::slskd::{state_from, Http, SlskdClient};

#[derive(Default)]
struct Recorded {
    responses: HashMap<String, Value>,
    failures: HashMap<String, String>,
    // `Mutex` rather than `RefCell`: `Http` is `Sync` now that searches fan out.
    calls: Mutex<Vec<(String, String, Option<Value>)>>,
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
            .lock()
            .expect("the recorder is not poisoned")
            .iter()
            .filter(|(seen, path, _)| seen == method && path.starts_with(prefix))
            .map(|(_, path, body)| (path.clone(), body.clone()))
            .collect()
    }
}

impl Http for Recorded {
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Option<Value>> {
        self.calls
            .lock()
            .expect("the recorder is not poisoned")
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
fn a_zero_bitrate_is_treated_as_unknown_not_as_zero() {
    let candidates = client(searching()).search("x", 15, 200).expect("a search");
    let flac = candidates
        .iter()
        .find(|candidate| candidate.extension() == "flac")
        .expect("a flac result");

    assert_eq!(flac.bitrate, None);
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
fn a_failure_to_clean_up_does_not_lose_the_results() {
    let http = searching().failing("DELETE", "/searches/search", "gone");
    assert_eq!(
        client(http).search("x", 15, 200).expect("a search").len(),
        3
    );
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
