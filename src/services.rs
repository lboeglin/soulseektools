//! What every public web service this tool talks to has in common.
//!
//! iTunes, MusicBrainz, the Cover Art Archive, AcoustID and Discogs are free,
//! publish a request limit, and ask callers to say who they are. The pacing, the
//! identification and the transport are one thing rather than one per service,
//! and the transport is a trait so each service's wire format is testable
//! against recorded responses with no network. The slskd daemon is not one of
//! these: it is local, authenticated, and has its own client.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::errors::{Error, Result};

/// The User-Agent sent to every service.
///
/// MusicBrainz and AcoustID ask callers to identify themselves with a real
/// contact -- a URL or an email address -- and throttle or block those that do
/// not. Nothing is invented here: set `artwork.contact` and it is included,
/// leave it unset and the header simply names the tool and its version.
#[must_use]
pub fn user_agent(contact: &str) -> String {
    let version = crate::VERSION;
    if contact.trim().is_empty() {
        format!("soulseektools/{version}")
    } else {
        format!("soulseektools/{version} ( {contact} )")
    }
}

/// Holds a caller to a minimum interval between requests.
///
/// These APIs are free and publish limits; exceeding them earns 503s and then a
/// block. One limiter lives on each service's client, so the pacing is per
/// service rather than global -- a slow provider must not slow a fast one.
#[derive(Debug)]
pub struct RateLimiter {
    pub min_interval_seconds: f64,
    next_allowed_at: std::sync::Mutex<Option<Instant>>,
}

impl RateLimiter {
    #[must_use]
    pub fn new(min_interval_seconds: f64) -> Self {
        Self {
            min_interval_seconds,
            next_allowed_at: std::sync::Mutex::new(None),
        }
    }

    /// Block until the next request is allowed.
    pub fn wait(&self) {
        if self.min_interval_seconds <= 0.0 {
            return;
        }
        let mut next = self
            .next_allowed_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(allowed_at) = *next {
            let now = Instant::now();
            if allowed_at > now {
                std::thread::sleep(allowed_at - now);
            }
        }
        *next = Some(Instant::now() + Duration::from_secs_f64(self.min_interval_seconds));
    }
}

/// The HTTP a service client needs, so its wire format is testable without
/// network.
pub trait ServiceHttp {
    /// A JSON GET with query parameters.
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value>;

    /// A JSON POST of a form-encoded body.
    fn post_form_json(&self, url: &str, form: &[(&str, String)]) -> Result<Value>;

    /// A raw GET, for an image.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>>;
}

/// The real transport, shared by every service client.
pub struct HttpTransport {
    client: reqwest::blocking::Client,
}

impl HttpTransport {
    pub fn new(timeout_seconds: f64, contact: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs_f64(timeout_seconds))
            .user_agent(user_agent(contact))
            .build()
            .map_err(|error| Error::Config(format!("cannot build an HTTP client: {error}")))?;
        Ok(Self { client })
    }
}

impl ServiceHttp for HttpTransport {
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value> {
        // `RequestBuilder::query` needs a feature this build does not carry, so
        // the query string is built on the URL itself.
        let url = reqwest::Url::parse_with_params(url, params)
            .map_err(|error| Error::Config(format!("{url}: {error}")))?;
        let response = self
            .client
            .get(url.clone())
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| Error::Config(format!("{url}: {error}")))?;
        response
            .json()
            .map_err(|error| Error::Config(format!("{url}: {error}")))
    }

    fn post_form_json(&self, url: &str, form: &[(&str, String)]) -> Result<Value> {
        let response = self
            .client
            .post(url)
            .form(form)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| Error::Config(format!("{url}: {error}")))?;
        response
            .json()
            .map_err(|error| Error::Config(format!("{url}: {error}")))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| Error::Config(format!("{url}: {error}")))?;
        Ok(response
            .bytes()
            .map_err(|error| Error::Config(format!("{url}: {error}")))?
            .to_vec())
    }
}
