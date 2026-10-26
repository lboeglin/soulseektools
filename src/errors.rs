//! Error types raised across the crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("{0}")]
    Tag(String),

    #[error("{0}")]
    Audio(String),

    #[error("{0}")]
    Config(String),

    #[error("{0}")]
    SoulseekClient(String),

    #[error("{0}")]
    Update(String),
}

pub type Result<T> = std::result::Result<T, Error>;
