//! `stk completions` -- a completion script for a shell.
//!
//! Generated from the same definitions the parser uses, so a new flag is
//! completed the moment it exists rather than when someone remembers to add it.

use std::io::Write as _;

use clap::{Args, CommandFactory};
use clap_complete::Shell;

use crate::cli::Cli;

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// The shell to complete for.
    pub shell: Shell,
}

/// Print the script. Needs no settings, so a broken config cannot stop it.
#[must_use]
pub fn run(args: &CompletionsArgs) -> i32 {
    let mut script = Vec::new();
    clap_complete::generate(args.shell, &mut Cli::command(), "stk", &mut script);
    // Written whole rather than streamed: `generate` panics on a reader that
    // stops early, and `| head` stopping early is not a failure.
    let _ = std::io::stdout().write_all(&script);
    0
}
