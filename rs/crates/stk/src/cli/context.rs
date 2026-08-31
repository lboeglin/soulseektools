//! Shared plumbing for the command groups: settings, output, exit codes.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::Resolved;
use crate::errors::{Error, Result};
use crate::library::index::LibraryIndex;
use crate::review::store::ReviewStore;

/// What every command needs: resolved settings and output verbosity.
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
    pub fn library_root(&self) -> PathBuf {
        self.resolved.resolved_library_root()
    }

    pub fn require_library_root(&self) -> Result<PathBuf> {
        let root = self.library_root();
        if root.is_dir() {
            return Ok(root);
        }
        Err(Error::Config(format!(
            "library root does not exist: {}\nhint: set it with 'stk config init' or --library",
            root.display()
        )))
    }

    pub fn open_index(&self) -> Result<LibraryIndex> {
        LibraryIndex::open(&self.settings().index.path)
    }

    pub fn open_review(&self) -> Result<ReviewStore> {
        ReviewStore::open(&self.settings().review.path)
    }

    /// Print a line of human-readable output.
    ///
    /// Nothing is written in `--json` mode: a progress line in the middle of a
    /// document is exactly what makes `--json` unusable in a pipeline.
    pub fn say(&self, line: &str) {
        if !self.as_json {
            println!("{line}");
        }
    }

    /// Emit the machine-readable document, two-space indented, UTF-8 as-is.
    pub fn emit(&self, value: &Value) {
        let mut stdout = std::io::stdout().lock();
        let _ = serde_json::to_writer_pretty(&mut stdout, value);
        let _ = stdout.write_all(b"\n");
    }
}

/// Print an error the way the Python's `fail` does, and give the exit code.
#[must_use]
pub fn fail(message: &str) -> i32 {
    eprintln!("error: {message}");
    1
}

/// Split a hint off an error message, so both halves land on their own line.
///
/// The Python passes `hint=` separately; carrying it in the message keeps the
/// error type from growing a field only the CLI reads.
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

/// A size the two copies of a duplicate can actually be compared by.
///
/// File sizes are well inside an `f64`, so the cast cannot lose a digit anyone
/// would see at one decimal place.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn human_size(size: u64) -> String {
    if size >= 1_000_000 {
        format!("{:.1} MB", size as f64 / 1_000_000.0)
    } else {
        format!("{:.0} KB", size as f64 / 1_000.0)
    }
}

/// Render rows as a plain aligned table.
///
/// `rich` draws these; nothing here needs its styling, and keeping the columns
/// aligned is the only part the output depends on.
#[must_use]
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let columns = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0_usize; columns];

    for (index, header) in headers.iter().enumerate() {
        widths[index] = header.chars().count();
    }
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }

    let render = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let padding = widths[index].saturating_sub(cell.chars().count());
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

/// The path as the Python prints it.
#[must_use]
pub fn show(path: &Path) -> String {
    path.display().to_string()
}
