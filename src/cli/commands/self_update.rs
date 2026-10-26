//! `stk self-update`, through the receipt the release installer leaves.

use axoupdater::{AxoUpdater, AxoupdateError, UpdateRequest};
use clap::Args;

use crate::cli::context::report;
use crate::errors::{Error, Result};

const REINSTALL: &str = "hint: install stk with the one-liner in the README, \
     https://github.com/lboeglin/soulseektools#install";

#[derive(Args, Debug)]
pub struct SelfUpdateArgs {
    /// Only say whether a newer release exists.
    #[arg(long)]
    pub check: bool,
}

/// Needs no settings, so a broken config cannot stop it.
#[must_use]
pub fn run(args: &SelfUpdateArgs) -> i32 {
    match installed_updater().and_then(|updater| update(updater, args.check)) {
        Ok(()) => 0,
        Err(error) => report(&error),
    }
}

pub fn installed_updater() -> Result<AxoUpdater> {
    let mut updater = AxoUpdater::new_for("stk");
    match updater.load_receipt() {
        Ok(_) => {}
        Err(AxoupdateError::NoReceipt { .. }) => {
            return Err(Error::Update(format!(
                "stk was not installed by the release installer, so it cannot update itself\n{REINSTALL}"
            )));
        }
        Err(error) => return Err(failed(&error)),
    }
    if !updater
        .check_receipt_is_for_this_executable()
        .map_err(|error| failed(&error))?
    {
        let installed = updater
            .install_prefix_root()
            .map_err(|error| failed(&error))?;
        return Err(Error::Update(format!(
            "this stk is not the installed one, which is under {installed}\n\
             hint: run that one's 'self-update', or reinstall over it"
        )));
    }
    Ok(updater)
}

fn update(mut updater: AxoUpdater, check_only: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    // A pre-release install follows pre-releases; a release ignores them.
    if current.contains('-') {
        updater.configure_version_specifier(UpdateRequest::LatestMaybePrerelease);
    }
    if check_only {
        if updater
            .is_update_needed_sync()
            .map_err(|error| failed(&error))?
        {
            println!("a newer release than stk {current} is out; run 'stk self-update'");
        } else {
            println!("stk {current} is the latest release");
        }
        return Ok(());
    }
    updater.disable_installer_stdout();
    match updater.run_sync().map_err(|error| failed(&error))? {
        Some(result) => println!("updated stk {current} to {}", result.new_version),
        None => println!("stk {current} is the latest release"),
    }
    Ok(())
}

fn failed(error: &AxoupdateError) -> Error {
    Error::Update(format!("self-update failed: {error}"))
}
