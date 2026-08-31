//! The slskd backend.
//!
//! Every request and response shape here was pinned against a running
//! `slskd/slskd` container's OpenAPI document and its bundled web UI, not from
//! memory; see `docs/slskd-setup.md`. Keeping the wire format in this one module
//! means a change to slskd is a change to one file.
//!
//! Authentication is the `X-API-Key` header. The key never comes from the config
//! file -- see [`crate::config`].

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::SlskdSettings;
use crate::errors::{Error, Result};
use crate::models::Candidate;
use crate::soulseek::base::{SoulseekClient, Transfer, TransferState};

pub const API_PREFIX: &str = "/api/v0";
pub const API_KEY_HEADER: &str = "X-API-Key";

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const SEARCH_GRACE: Duration = Duration::from_secs(5);

/// slskd reports the underlying Soulseek.NET transfer states, which are
/// flag-like strings such as "Completed, Succeeded" or "Completed, Errored".
///
/// The order matters: a terminal flag wins over the bare "Completed" that
/// accompanies it, so "Completed, Errored" is a failure rather than a success.
/// That was a real bug once -- see the changelog.
const TERMINAL_STATES: &[(&str, TransferState)] = &[
    ("succeeded", TransferState::Completed),
    ("cancelled", TransferState::Cancelled),
    ("errored", TransferState::Failed),
    ("timedout", TransferState::Failed),
    ("rejected", TransferState::Failed),
];

/// The HTTP calls the client makes, so the wire format can be tested without a
/// daemon.
///
/// The Python mocks `httpx` with `respx`; a trait here keeps the JSON handling
/// -- which is the part that actually encodes slskd's quirks -- testable on its
/// own.
pub trait Http: Sync {
    /// Perform one request, returning the parsed body, or `None` for an empty one.
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Option<Value>>;
}

/// Talks to a slskd daemon over its REST API.
pub struct SlskdClient<H: Http> {
    pub settings: SlskdSettings,
    http: H,
}

impl<H: Http> SlskdClient<H> {
    pub fn new(settings: SlskdSettings, http: H) -> Self {
        Self { settings, http }
    }

    fn get(&self, path: &str) -> Result<Option<Value>> {
        self.http.request("GET", path, None)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Option<Value>> {
        self.http.request("POST", path, Some(body))
    }

    /// A search slskd has already cleaned up is not a failure worth surfacing
    /// over the results it produced.
    fn delete(&self, path: &str) {
        let _ = self.http.request("DELETE", path, None);
    }

    fn await_completion(&self, search_id: &str, timeout_seconds: u32) {
        let deadline =
            Instant::now() + Duration::from_secs(u64::from(timeout_seconds)) + SEARCH_GRACE;
        while Instant::now() < deadline {
            match self.get(&format!("/searches/{search_id}")) {
                Ok(Some(state))
                    if state.get("isComplete").and_then(Value::as_bool) == Some(true) =>
                {
                    return;
                }
                Ok(_) => {}
                Err(_) => return,
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl<H: Http> SoulseekClient for SlskdClient<H> {
    fn is_connected(&self) -> bool {
        let Ok(Some(state)) = self.get("/server") else {
            return false;
        };
        let flag = |name: &str| state.get(name).and_then(Value::as_bool).unwrap_or(false);
        flag("isConnected") && flag("isLoggedIn")
    }

    fn search(
        &self,
        text: &str,
        timeout_seconds: u32,
        response_limit: u32,
    ) -> Result<Vec<Candidate>> {
        let search_id = new_search_id();
        self.post(
            "/searches",
            &serde_json::json!({
                "id": search_id,
                "searchText": text,
                "searchTimeout": timeout_seconds.max(5),
                "responseLimit": response_limit,
            }),
        )?;

        self.await_completion(&search_id, timeout_seconds);
        let responses = self.get(&format!("/searches/{search_id}/responses"));
        self.delete(&format!("/searches/{search_id}"));

        Ok(as_list(responses?.as_ref())
            .iter()
            .flat_map(candidates_from)
            .collect())
    }

    fn enqueue(
        &self,
        username: &str,
        files: &[Candidate],
        destination: &str,
    ) -> Result<Vec<Transfer>> {
        if files.is_empty() {
            return Ok(Vec::new());
        }

        let items: Vec<Value> = files
            .iter()
            .map(|candidate| {
                serde_json::json!({
                    "filename": candidate.remote_path,
                    "size": candidate.size,
                })
            })
            .collect();

        if !destination.is_empty() {
            let payload = serde_json::json!({
                "username": username,
                "files": items,
                "options": {"destination": destination},
            });
            let response = self
                .post("/transfers/downloads/batches", &payload)?
                .unwrap_or(Value::Null);

            let failures = as_list(response.get("failures"));
            if !failures.is_empty() {
                return Err(Error::SoulseekClient(
                    failures
                        .iter()
                        .map(|failure| {
                            format!(
                                "{}: {}",
                                text_of(failure.get("filename")),
                                text_of(failure.get("message"))
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                ));
            }

            return Ok(as_list(
                response
                    .get("batch")
                    .and_then(|batch| batch.get("transfers")),
            )
            .iter()
            .map(|item| transfer_from(item, ""))
            .collect());
        }

        self.post(
            &format!("/transfers/downloads/{username}"),
            &Value::Array(items),
        )?;

        let wanted: Vec<&str> = files
            .iter()
            .map(|candidate| candidate.remote_path.as_str())
            .collect();
        Ok(self
            .transfers()?
            .into_iter()
            .filter(|transfer| {
                transfer.username == username && wanted.contains(&transfer.remote_path.as_str())
            })
            .collect())
    }

    fn cancel(&self, username: &str, transfer_id: &str) -> Result<()> {
        self.http
            .request(
                "DELETE",
                &format!("/transfers/downloads/{username}/{transfer_id}"),
                None,
            )
            .map(|_| ())
    }

    fn transfers(&self) -> Result<Vec<Transfer>> {
        let response = self.get("/transfers/downloads")?;
        Ok(as_list(response.as_ref())
            .iter()
            .flat_map(|user| {
                let username = text_of(user.get("username"));
                as_list(user.get("directories"))
                    .into_iter()
                    .flat_map(move |directory| {
                        let username = username.clone();
                        as_list(directory.get("files"))
                            .into_iter()
                            .map(move |item| transfer_from(&item, &username))
                            .collect::<Vec<_>>()
                    })
            })
            .collect())
    }
}

/// A search id slskd will accept, without pulling in a uuid crate for one call.
fn new_search_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        u32::try_from(now.as_secs() & 0xffff_ffff).unwrap_or(0),
        (now.subsec_nanos() >> 16) & 0xffff,
        now.subsec_nanos() & 0xfff,
        std::process::id() & 0xfff,
        u64::from(now.subsec_nanos()) << 16 | u64::from(std::process::id() & 0xffff)
    )
}

fn candidates_from(response: &Value) -> Vec<Candidate> {
    let username = text_of(response.get("username"));
    let free_slots = response
        .get("hasFreeUploadSlot")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let queue_length = response
        .get("queueLength")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);

    as_list(response.get("files"))
        .iter()
        .filter(|file| !text_of(file.get("filename")).is_empty())
        .map(|file| Candidate {
            username: username.clone(),
            remote_path: text_of(file.get("filename")),
            size: file.get("size").and_then(Value::as_u64).unwrap_or(0),
            bitrate: optional_u32(file.get("bitRate")),
            sample_rate: optional_u32(file.get("sampleRate")),
            bit_depth: optional_u32(file.get("bitDepth")),
            is_vbr: file
                .get("isVariableBitRate")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            free_upload_slots: free_slots,
            queue_length,
            duration_seconds: optional_f64(file.get("length")),
        })
        .collect()
}

fn transfer_from(item: &Value, username: &str) -> Transfer {
    let own_username = text_of(item.get("username"));
    Transfer {
        id: text_of(item.get("id")),
        username: if own_username.is_empty() {
            username.to_owned()
        } else {
            own_username
        },
        remote_path: text_of(item.get("filename")),
        state: state_from(&text_of(item.get("state"))),
        size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
        bytes_transferred: item
            .get("bytesTransferred")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        place_in_queue: optional_u32(item.get("placeInQueue")),
        local_path: String::new(),
        error: text_of(item.get("exception")),
    }
}

/// Collapse a comma-separated Soulseek.NET state into one outcome.
#[must_use]
pub fn state_from(raw: &str) -> TransferState {
    let parts: Vec<String> = raw
        .split(',')
        .map(|part| part.trim().to_lowercase())
        .collect();

    for (name, state) in TERMINAL_STATES {
        if parts.iter().any(|part| part == name) {
            return *state;
        }
    }
    if parts.iter().any(|part| part == "completed") {
        return TransferState::Completed;
    }
    if parts.iter().any(|part| part == "inprogress") {
        return TransferState::InProgress;
    }
    TransferState::Queued
}

fn as_list(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

/// slskd writes an id as a number in some payloads and a string in others.
fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// A zero means "unknown" throughout slskd's search payloads, not "zero".
fn optional_u32(value: Option<&Value>) -> Option<u32> {
    let number = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.parse().ok()?,
        _ => return None,
    };
    // slskd sends these as JSON numbers; anything outside `u32` is not a
    // bitrate, a sample rate or a bit depth.
    #[allow(clippy::cast_possible_truncation)]
    let number = number as i64;
    u32::try_from(number).ok().filter(|value| *value != 0)
}

fn optional_f64(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.parse().ok()?,
        _ => return None,
    };
    (number != 0.0).then_some(number)
}

impl<H: Http> SlskdClient<H> {
    /// Hand back the transport, so a test can inspect what was sent.
    pub fn into_http(self) -> H {
        self.http
    }
}

/// The headers every request carries.
///
/// Split out so the `X-API-Key` carve-out is testable without a live daemon:
/// the key is read from the environment, never from the config file, and this is
/// the only place it reaches the wire.
#[must_use]
pub fn headers_for(settings: &SlskdSettings) -> Vec<(String, String)> {
    let mut headers = vec![("Accept".to_owned(), "application/json".to_owned())];
    if let Some(key) = &settings.api_key {
        headers.push((API_KEY_HEADER.to_owned(), key.clone()));
    }
    headers
}

/// The real transport: slskd over HTTP.
pub struct HttpTransport {
    client: reqwest::blocking::Client,
    base_url: String,
    headers: Vec<(String, String)>,
}

impl HttpTransport {
    pub fn new(settings: &SlskdSettings) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs_f64(
                settings.request_timeout_seconds,
            ))
            .build()
            .map_err(|error| {
                Error::SoulseekClient(format!("cannot build an HTTP client: {error}"))
            })?;

        Ok(Self {
            client,
            base_url: settings.url.trim_end_matches('/').to_owned(),
            headers: headers_for(settings),
        })
    }
}

impl Http for HttpTransport {
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Option<Value>> {
        let url = format!("{}{API_PREFIX}{path}", self.base_url);
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| Error::SoulseekClient(format!("bad method: {error}")))?;

        let mut request = self.client.request(method.clone(), &url);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().map_err(|error| {
            Error::SoulseekClient(format!("cannot reach slskd at {}: {error}", self.base_url))
        })?;

        let status = response.status();
        if !status.is_success() {
            // 401 is nearly always the key rather than the request, and saying so
            // saves a round of guessing.
            let hint = if status == reqwest::StatusCode::UNAUTHORIZED {
                ": check SLSKD_API_KEY"
            } else {
                ""
            };
            return Err(Error::SoulseekClient(format!(
                "slskd returned {} for {method} {path}{hint}",
                status.as_u16()
            )));
        }

        let bytes = response
            .bytes()
            .map_err(|error| Error::SoulseekClient(format!("slskd sent no body: {error}")))?;
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(serde_json::from_slice(&bytes).ok())
    }
}
