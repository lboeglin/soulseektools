//! Reading an edited `stk tags dump` back, and working out what it changes.
//!
//! `tags dump` was a dead end: it showed every file's artist, album and title,
//! and there was no writer for any of them, so you could see what was wrong and
//! not fix it (#3). Mass-fixing untagged Soulseek downloads is close to the
//! whole point of the tool, and a spreadsheet is the right interface for it.
//!
//! The sheet is keyed on its `path` column and read by header name, so a
//! hand-written `path,genres` file is as valid an input as the dump itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};
use crate::library::tags::{self, TagField};
use crate::models::{LibraryEntry, TrackTags};

/// What one cell of the sheet asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// The sheet had no column for this field.
    Untouched,
    /// The column was there and empty.
    Clear,
    Set(String),
}

/// What an empty cell means for one run.
///
/// Leaving the tag alone is the default because the commoner cause of an empty
/// cell is a partly-filled sheet, or a spreadsheet that mangled the file, and
/// not an intent to erase. Clearing has to be possible or the command cannot
/// undo its own mistakes, so it is asked for per invocation -- a statement about
/// *this* sheet rather than a setting anyone would remember having made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blanks {
    #[default]
    LeaveAlone,
    Clear,
}

/// One row of an edited sheet, before it meets the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEdit {
    pub path: PathBuf,
    pub cells: Vec<(TagField, Cell)>,
}

impl TagEdit {
    /// The tags this row asks for, applied to what the file carries now.
    #[must_use]
    pub fn onto(&self, current: &TrackTags) -> TrackTags {
        let mut proposed = current.clone();
        for (field, cell) in &self.cells {
            match cell {
                Cell::Untouched => {}
                Cell::Clear => field.set(&mut proposed, ""),
                Cell::Set(value) => field.set(&mut proposed, value),
            }
        }
        proposed
    }
}

/// One file's proposed writes, against what the index holds.
#[derive(Debug, Clone, PartialEq)]
pub struct TagChange {
    pub edit: TagEdit,
    pub current: TrackTags,
    pub proposed: TrackTags,
    /// The fields that differ, in the order a dump lists them.
    pub fields: Vec<TagField>,
}

/// Why a row of the sheet writes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    NotIndexed,
    FileGone,
    NotTaggable,
    NoChange,
}

impl std::fmt::Display for Skipped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotIndexed => "not in the index",
            Self::FileGone => "the file is gone",
            Self::NotTaggable => "tags cannot be written to this container",
            Self::NoChange => "already what the sheet asks for",
        })
    }
}

/// What one row of the sheet comes to.
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedEdit {
    Change(Box<TagChange>),
    Skip { path: PathBuf, reason: Skipped },
}

/// Read an edited dump, keyed by its header row.
///
/// A column the header does not carry is left alone, whatever `blanks` says:
/// that is what makes a two-column `path,genres` sheet safe to write by hand.
pub fn read_sheet(sheet: &Path, blanks: Blanks) -> Result<Vec<TagEdit>> {
    let bytes = std::fs::read(sheet)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", sheet.display())))?;
    // `tags dump -o` writes a BOM so Excel does not mangle non-ASCII paths, and
    // the csv crate does not strip one: without this, a dump fails to read back
    // as its own input, with a header named "\u{feff}path".
    let bytes = bytes
        .strip_prefix(b"\xef\xbb\xbf".as_slice())
        .unwrap_or(&bytes);

    let mut reader = csv::Reader::from_reader(bytes);
    let headers: Vec<String> = reader
        .headers()
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", sheet.display())))?
        .iter()
        .map(|header| header.trim().to_lowercase())
        .collect();

    let path_column = headers
        .iter()
        .position(|header| header == "path")
        .ok_or_else(|| {
            Error::Config(format!(
                "{}: no 'path' column, only: {}",
                sheet.display(),
                headers.join(", ")
            ))
        })?;

    let columns: Vec<(usize, TagField)> = headers
        .iter()
        .enumerate()
        .filter_map(|(index, header)| header.parse().ok().map(|field| (index, field)))
        .collect();

    let mut edits = Vec::new();
    for record in reader.records() {
        let record = record
            .map_err(|error| Error::Config(format!("cannot read {}: {error}", sheet.display())))?;
        let Some(path) = record.get(path_column).map(str::trim) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }

        edits.push(TagEdit {
            path: PathBuf::from(path),
            cells: columns
                .iter()
                .map(|(index, field)| (*field, cell(record.get(*index), blanks)))
                .collect(),
        });
    }
    Ok(edits)
}

fn cell(value: Option<&str>, blanks: Blanks) -> Cell {
    match value.map(str::trim) {
        None => Cell::Untouched,
        Some("") => match blanks {
            Blanks::LeaveAlone => Cell::Untouched,
            Blanks::Clear => Cell::Clear,
        },
        Some(value) => Cell::Set(value.to_owned()),
    }
}

/// Diff the edited rows against the index, in the sheet's own order.
#[must_use]
pub fn plan_changes(edits: &[TagEdit], entries: &[LibraryEntry]) -> Vec<PlannedEdit> {
    // Keyed on the same raw string the index stores, so a row either names a
    // file the index knows or it does not; nothing here guesses at a near miss.
    let indexed: HashMap<&Path, &LibraryEntry> = entries
        .iter()
        .map(|entry| (entry.path.as_path(), entry))
        .collect();

    edits
        .iter()
        .map(|edit| {
            let skip = |reason| PlannedEdit::Skip {
                path: edit.path.clone(),
                reason,
            };
            let Some(entry) = indexed.get(edit.path.as_path()) else {
                return skip(Skipped::NotIndexed);
            };
            // Checked here rather than discovered at write time: a dry run that
            // promises N changes and then fails all N is not a plan.
            if !entry.path.is_file() {
                return skip(Skipped::FileGone);
            }
            if !taggable(&entry.path) {
                return skip(Skipped::NotTaggable);
            }

            let proposed = edit.onto(&entry.tags);
            let fields: Vec<TagField> = TagField::ALL
                .into_iter()
                .filter(|field| field.value(&entry.tags) != field.value(&proposed))
                .collect();
            if fields.is_empty() {
                return skip(Skipped::NoChange);
            }

            PlannedEdit::Change(Box::new(TagChange {
                edit: edit.clone(),
                current: entry.tags.clone(),
                proposed,
                fields,
            }))
        })
        .collect()
}

fn taggable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            tags::TAGGABLE_EXTENSIONS.contains(&format!(".{}", extension.to_lowercase()).as_str())
        })
}

/// Write one planned change, and report what the file carried before it.
///
/// The plan was diffed against the index, which is a cache. What actually gets
/// written is built from a fresh read, so a field the sheet never mentioned is
/// not quietly reverted to whatever the cache remembered; and the value handed
/// to the log is the one that was really there, or an undo restores a fiction.
///
/// A field this row means to write that has moved since the plan is refused
/// rather than overwritten. Fields it is not writing are not its business: a
/// bulk run must not abort because some unrelated column went stale.
pub fn apply_change(change: &TagChange) -> Result<TrackTags> {
    let before = tags::read_tags(&change.edit.path)?;
    for field in &change.fields {
        if field.value(&before) != field.value(&change.current) {
            return Err(Error::Tag(format!(
                "{}: the {field} tag changed since the plan was made",
                change.edit.path.display()
            )));
        }
    }

    let written = change.edit.onto(&before);
    tags::write_tags(&change.edit.path, &written)?;
    Ok(before)
}
