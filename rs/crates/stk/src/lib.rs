//! A Rust port of `stk`, in progress.
//!
//! Modules mirror `py/src/soulseektools/` one for one so the two trees can be
//! read against each other, and each is diffed against `oracle/` -- output
//! captured from the Python build against a real library -- before the next is
//! started. See <https://github.com/lboeglin/soulseektools/issues/1>.

pub mod audio;
pub mod config;
pub mod duplicates;
pub mod errors;
pub mod genres;
pub mod library;
pub mod matching;
pub mod models;
pub mod review;
pub mod soulseek;
pub mod text;
pub mod wantlist;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
