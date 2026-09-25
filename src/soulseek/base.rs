//! What the rest of the crate needs from a Soulseek backend. Everything works
//! against the trait, so tests run on a fake with no network or daemon.

use std::fmt;
use std::str::FromStr;

use crate::errors::Result;
use crate::models::Candidate;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TransferState {
    #[default]
    Queued,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl TransferState {
    #[must_use]
    pub fn finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for TransferState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TransferState {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        match text {
            "queued" => Ok(Self::Queued),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown transfer state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transfer {
    pub id: String,
    pub username: String,
    pub remote_path: String,
    pub state: TransferState,
    pub size: u64,
    pub bytes_transferred: u64,
    pub place_in_queue: Option<u32>,
    pub local_path: String,
    pub error: String,
}

impl Transfer {
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn percent_complete(&self) -> f64 {
        if self.size == 0 {
            return 0.0;
        }
        100.0 * self.bytes_transferred as f64 / self.size as f64
    }
}

/// `Sync`, because searches run concurrently.
pub trait SoulseekClient: Sync {
    fn is_connected(&self) -> bool;

    fn search(
        &self,
        text: &str,
        timeout_seconds: u32,
        response_limit: u32,
    ) -> Result<Vec<Candidate>>;

    fn enqueue(
        &self,
        username: &str,
        files: &[Candidate],
        destination: &str,
    ) -> Result<Vec<Transfer>>;

    fn transfers(&self) -> Result<Vec<Transfer>>;

    fn cancel(&self, username: &str, transfer_id: &str) -> Result<()>;
}
