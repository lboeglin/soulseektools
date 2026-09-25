//! What the public web services share: pacing, a User-Agent, and a testable transport.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::errors::{Error, Result};

#[must_use]
pub fn user_agent(contact: &str) -> String {
    let version = crate::VERSION;
    if contact.trim().is_empty() {
        format!("soulseektools/{version}")
    } else {
        format!("soulseektools/{version} ( {contact} )")
    }
}

/// One per service client, so a slow provider doesn't pace a fast one.
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

pub trait ServiceHttp {
    fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<Value>;

    fn post_form_json(&self, url: &str, form: &[(&str, String)]) -> Result<Value>;

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>>;
}

fn refused(
    url: &reqwest::Url,
    status: reqwest::StatusCode,
    response: reqwest::blocking::Response,
) -> String {
    let message = response.json::<Value>().ok().and_then(|body| {
        body.get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    match message {
        Some(message) => format!("{url}: {status}: {message}"),
        None => format!("{url}: {status}"),
    }
}

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
        // The `query` builder needs a reqwest feature this build doesn't enable.
        let url = reqwest::Url::parse_with_params(url, params)
            .map_err(|error| Error::Config(format!("{url}: {error}")))?;
        let response = self
            .client
            .get(url.clone())
            .send()
            .map_err(|error| Error::Config(format!("{url}: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Config(refused(&url, status, response)));
        }
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
