//! Error types raised across the crate.

use thiserror::Error;

/// Every error this crate raises deliberately.
#[derive(Debug, Error)]
pub enum Error {
    /// The file's container is not one this crate can read, write or decode.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    /// A tag could not be read from or written to a file.
    #[error("{0}")]
    Tag(String),

    /// The audio could not be decoded or measured.
    #[error("{0}")]
    Audio(String),

    /// The configuration is missing, malformed or internally inconsistent.
    #[error("{0}")]
    Config(String),

    /// The Soulseek backend rejected a request or could not be reached.
    #[error("{0}")]
    SoulseekClient(String),
}

pub type Result<T> = std::result::Result<T, Error>;
