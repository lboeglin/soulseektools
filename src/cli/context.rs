//! Shared plumbing for the commands: settings, output, exit codes.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::config::Resolved;
use crate::errors::{Error, Result};
use crate::library::index::LibraryIndex;
use crate::library::scan::Excludes;
use crate::review::store::ReviewStore;

pub struct AppContext {
    pub resolved: Resolved,
    pub verbose: bool,
    pub as_json: bool,
}

impl AppContext {
    #[must_use]
    pub fn settings(&self) -> &crate::config::Settings {
        &self.resolved.settings
    }

    #[must_use]
    pub fn library_roots(&self) -> Vec<PathBuf> {
        self.resolved.resolved_library_roots()
    }

    pub fn require_library_roots(&self) -> Result<Vec<PathBuf>> {
        let roots = self.library_roots();
        let missing: Vec<String> = roots
            .iter()
            .filter(|root| !root.is_dir())
            .map(|root| root.display().to_string())
            .collect();
        if missing.is_empty() {
            return Ok(roots);
        }
        Err(Error::Config(format!(
            "library root does not exist: {}\nhint: set it with 'stk config init' or --library",
            missing.join(", ")
        )))
    }

    pub fn excludes(&self) -> Result<Excludes> {
        Excludes::new(&self.settings().library.exclude)
    }

    pub fn open_index(&self) -> Result<LibraryIndex> {
        LibraryIndex::open(&self.settings().index.path)
    }

    pub fn open_review(&self) -> Result<ReviewStore> {
        ReviewStore::open(&self.settings().review.path)
    }

    /// Nothing is written in `--json` mode.
    pub fn say(&self, line: &str) {
        if !self.as_json {
            println!("{line}");
        }
    }

    /// Errors often name their file already; it is said once.
    pub fn say_failure(&self, path: &str, reason: &str) {
        let reason = reason.replace(&format!("{path}: "), "");
        self.say(&format!("failed {path}: {reason}"));
    }

    /// Under `--json` the run carries on, so an empty document is still printed.
    #[must_use]
    pub fn nothing_to_do(&self, line: &str) -> bool {
        if self.as_json {
            return false;
        }
        self.say(line);
        true
    }

    pub fn emit(&self, value: &Value) {
        let mut stdout = std::io::stdout().lock();
        let _ = serde_json::to_writer_pretty(&mut stdout, value);
        let _ = stdout.write_all(b"\n");
    }
}

/// Parses `--candidates`: no more than a search keeps.
pub fn candidate_count(text: &str) -> std::result::Result<usize, String> {
    let limit = crate::config::STORED_CANDIDATES;
    match text.parse::<usize>() {
        Ok(count) if (1..=limit).contains(&count) => Ok(count),
        _ => Err(format!("must be a number from 1 to {limit}")),
    }
}

#[must_use]
pub fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

#[must_use]
pub fn fail(message: &str) -> i32 {
    eprintln!("error: {message}");
    1
}

/// Splits a `hint:` line off the message, so the error type needs no CLI-only field.
#[must_use]
pub fn report(error: &Error) -> i32 {
    let text = error.to_string();
    match text.split_once("\nhint: ") {
        Some((message, hint)) => {
            eprintln!("error: {message}");
            eprintln!("hint: {hint}");
            1
        }
        None => fail(&text),
    }
}

#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn human_size(size: u64) -> String {
    if size >= 1_000_000 {
        format!("{:.1} MB", size as f64 / 1_000_000.0)
    } else {
        format!("{:.0} KB", size as f64 / 1_000.0)
    }
}

/// Padded by display width: a CJK character takes two terminal columns.
#[must_use]
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let columns = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0_usize; columns];

    for (index, header) in headers.iter().enumerate() {
        widths[index] = header.width();
    }
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.width());
        }
    }

    let render = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let padding = widths[index].saturating_sub(cell.width());
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };

    let mut lines = Vec::new();
    if !headers.iter().all(|header| header.is_empty()) {
        lines.push(render(
            &headers
                .iter()
                .map(|header| (*header).to_owned())
                .collect::<Vec<_>>(),
        ));
    }
    lines.extend(rows.iter().map(|row| render(row)));
    lines.join("\n")
}

#[must_use]
pub fn show(path: &Path) -> String {
    path.display().to_string()
}
