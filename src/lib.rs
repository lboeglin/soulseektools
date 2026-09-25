//! `stk`: build a music library from a streaming export via Soulseek, and keep
//! it clean afterwards.

pub mod artwork;
pub mod audio;
pub mod cli;
pub mod config;
pub mod duplicates;
pub mod errors;
pub mod fetch;
pub mod genres;
pub mod identify;
pub mod library;
pub mod matching;
pub mod models;
pub mod organise;
pub mod playlist;
pub mod replaygain;
pub mod retag;
pub mod review;
pub mod services;
pub mod soulseek;
pub mod tagsheet;
pub mod text;
pub mod wantlist;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
