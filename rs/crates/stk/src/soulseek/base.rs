//! What the rest of the crate needs from a Soulseek backend.
//!
//! Everything above this line works against the trait, not against slskd, so the
//! search and download flows are exercised in tests by a fake with no network,
//! no daemon and no Soulseek account. Preserving that boundary is what makes the
//! ported tests runnable in CI.

use std::fmt;
use std::str::FromStr;

use crate::errors::Result;
use crate::models::Candidate;

/// Where a download has got to, collapsed from the backend's own vocabulary.
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

/// One download, as the backend currently sees it.
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
    /// Progress as a percentage. Sizes are file lengths, well inside an `f64`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn percent_complete(&self) -> f64 {
        if self.size == 0 {
            return 0.0;
        }
        100.0 * self.bytes_transferred as f64 / self.size as f64
    }
}

/// A Soulseek backend that can search and download.
///
/// `Sync`, because `fetch::search_pending` runs the searches concurrently: every
/// one of them is spent waiting on a socket, and a serial port would be no
/// faster than the serial Python it replaces.
pub trait SoulseekClient: Sync {
    /// Whether the backend is logged in and able to search.
    fn is_connected(&self) -> bool;

    /// Run one search to completion and return every file offered.
    fn search(
        &self,
        text: &str,
        timeout_seconds: u32,
        response_limit: u32,
    ) -> Result<Vec<Candidate>>;

    /// Queue files for download from one peer.
    fn enqueue(
        &self,
        username: &str,
        files: &[Candidate],
        destination: &str,
    ) -> Result<Vec<Transfer>>;

    /// Every download the backend is tracking.
    fn transfers(&self) -> Result<Vec<Transfer>>;

    /// Abandon one queued or running download.
    ///
    /// A transfer that has stalled long enough to be retried elsewhere has to
    /// go: a peer that wakes up hours later would otherwise deliver the file a
    /// second time, beside the copy the retry already fetched.
    fn cancel(&self, username: &str, transfer_id: &str) -> Result<()>;
}
