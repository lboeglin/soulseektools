//! `stk config` -- inspect and create the configuration file.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use crate::cli::context::{show, AppContext};
use crate::config::{default_config_path, render_toml, write_config, CONFIG_FILENAME};
use crate::errors::Result;

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Write a configuration file containing the current effective settings.
    Init {
        /// Where to write the file.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Write the per-user config instead of ./stk.toml.
        #[arg(long)]
        user: bool,
    },
    /// Print the effective settings, secrets omitted.
    Show,
    /// Print which configuration file is in effect.
    Path,
}

#[allow(clippy::needless_pass_by_value)]
pub fn run(context: &AppContext, command: ConfigCommand) -> Result<i32> {
    match command {
        ConfigCommand::Init { path, user } => init(context, path, user),
        ConfigCommand::Show => show_settings(context),
        ConfigCommand::Path => Ok(show_path(context)),
    }
}

#[allow(clippy::unnecessary_wraps)]
fn init(context: &AppContext, path: Option<PathBuf>, user: bool) -> Result<i32> {
    let target = match path {
        Some(path) => path,
        None if user => default_config_path(),
        None => std::env::current_dir()
            .unwrap_or_default()
            .join(CONFIG_FILENAME),
    };

    match write_config(&target, context.settings()) {
        Ok(written) => {
            if context.as_json {
                context.emit(&json!({"written": show(&written)}));
            } else {
                context.say(&format!("Wrote {}", show(&written)));
            }
            Ok(0)
        }
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("hint: edit it directly, or pass --path to write elsewhere");
            Ok(1)
        }
    }
}

fn show_settings(context: &AppContext) -> Result<i32> {
    if context.as_json {
        context.emit(&serde_json::to_value(context.settings()).unwrap_or_default());
        return Ok(0);
    }
    context.say(render_toml(context.settings())?.trim_end());
    Ok(0)
}

fn show_path(context: &AppContext) -> i32 {
    let source = context.resolved.source_path.as_deref();
    if context.as_json {
        context.emit(&json!({"path": source.map(show)}));
        return 0;
    }
    // Printed unwrapped: this line is meant to be substituted into other commands.
    match source {
        Some(path) => println!("{}", show(path)),
        None => println!("no config file found; using defaults"),
    }
    0
}
