//! `stk`: build a music library from a streaming export via Soulseek, and keep
//! it clean afterwards.
//!
//! Ported from Python one module at a time, each diffed against `oracle/` --
//! output captured from that build against a real library -- before the next was
//! started. See <https://github.com/lboeglin/soulseektools/issues/1>.

pub mod artwork;
pub mod audio;
pub mod cli;
pub mod config;
pub mod duplicates;
pub mod errors;
pub mod fetch;
pub mod genres;
pub mod library;
pub mod matching;
pub mod models;
pub mod review;
pub mod soulseek;
pub mod text;
pub mod wantlist;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
