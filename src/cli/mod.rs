//! The `stk` entry point.

pub mod commands;
pub mod context;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{load_settings, Overrides};
use context::{report, AppContext};

#[derive(Parser, Debug)]
#[command(
    name = "stk",
    version,
    about = "Keep a music library clean: transcodes, duplicates, covers, genres and tags, and fill its gaps through Soulseek.",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Configuration file to use.
    #[arg(short = 'c', long, global = true, env = "STK_CONFIG")]
    pub config: Option<PathBuf>,

    /// Override the configured library root.
    #[arg(short = 'l', long, global = true)]
    pub library: Option<PathBuf>,

    /// Emit machine-readable output.
    #[arg(long = "json", global = true)]
    pub as_json: bool,

    /// Show more detail.
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Inspect and create the configuration file.
    #[command(subcommand)]
    Config(commands::config::ConfigCommand),

    /// Report whether the environment can run every command.
    Doctor,

    /// Check that files hold the audio their container claims: lossless, hi-res, or an MP3's bitrate.
    Verify(commands::verify::VerifyArgs),

    /// Search for tracks by name -- one, several, or a list -- and fetch them.
    Get(commands::get::GetArgs),

    /// Scan and inspect the library index.
    #[command(subcommand)]
    Library(commands::library::LibraryCommand),

    /// Measure loudness and write the ReplayGain tags a player levels with.
    #[command(alias = "replaygain")]
    Loudness(commands::loudness::LoudnessArgs),

    /// Find duplicate tracks by title or by audio.
    #[command(subcommand)]
    Dupes(commands::dupes::DupesCommand),

    /// Audit and repair embedded cover art.
    #[command(subcommand)]
    Art(commands::art::ArtCommand),

    /// Audit and apply genre tags.
    #[command(subcommand)]
    Genre(commands::genre::GenreCommand),

    /// Inspect tags across the library.
    #[command(subcommand)]
    Tags(commands::tags::TagsCommand),

    /// Move files into the layout their tags describe.
    #[command(alias = "organize")]
    Organise(commands::organise::OrganiseArgs),

    /// Write playlists from the library.
    #[command(subcommand)]
    Playlist(commands::playlist::PlaylistCommand),

    /// Compare a streaming export against the library.
    #[command(subcommand)]
    Wantlist(commands::wantlist::WantlistCommand),

    /// Review and approve download candidates.
    #[command(subcommand)]
    Review(commands::review::ReviewCommand),

    /// Search Soulseek and download approved candidates.
    #[command(subcommand)]
    Fetch(commands::fetch::FetchCommand),

    /// Scan, diff an export, search, pick, download and organise in one pass.
    Sync(commands::sync::SyncArgs),

    /// Inspect and undo what the writing commands changed.
    #[command(subcommand)]
    Changes(commands::changes::ChangesCommand),

    /// Print a completion script for a shell.
    Completions(commands::completions::CompletionsArgs),
}

/// Returns the exit code rather than exiting, so the whole CLI runs in tests.
#[must_use]
pub fn run(arguments: Vec<std::ffi::OsString>) -> i32 {
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => {
            // Help and version exit 0; usage errors exit 2.
            let _ = error.print();
            return i32::from(error.exit_code() != 0) * 2;
        }
    };

    if let Command::Completions(args) = &cli.command {
        return commands::completions::run(args);
    }

    let overrides = Overrides {
        library_root: cli.library.clone(),
    };
    let resolved = match load_settings(cli.config.as_deref(), &overrides) {
        Ok(resolved) => resolved,
        Err(error) => return report(&error),
    };

    let context = AppContext {
        resolved,
        verbose: cli.verbose,
        as_json: cli.as_json,
    };

    let outcome = match cli.command {
        Command::Config(command) => commands::config::run(&context, command),
        Command::Doctor => commands::doctor::run(&context),
        Command::Verify(args) => commands::verify::run(&context, &args),
        Command::Get(args) => commands::get::run(&context, &args),
        Command::Library(command) => commands::library::run(&context, command),
        Command::Loudness(args) => commands::loudness::run(&context, &args),
        Command::Dupes(command) => commands::dupes::run(&context, command),
        Command::Art(command) => commands::art::run(&context, command),
        Command::Genre(command) => commands::genre::run(&context, command),
        Command::Tags(command) => commands::tags::run(&context, command),
        Command::Organise(args) => commands::organise::run(&context, &args),
        Command::Playlist(command) => commands::playlist::run(&context, command),
        Command::Wantlist(command) => commands::wantlist::run(&context, command),
        Command::Review(command) => commands::review::run(&context, command),
        Command::Fetch(command) => commands::fetch::run(&context, command),
        Command::Sync(args) => commands::sync::run(&context, &args),
        Command::Changes(command) => commands::changes::run(&context, command),
        Command::Completions(_) => unreachable!("answered before the settings are loaded"),
    };

    match outcome {
        Ok(code) => code,
        Err(error) => report(&error),
    }
}
