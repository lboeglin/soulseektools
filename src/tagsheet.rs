//! Reading an edited `stk tags dump` back, keyed on `path` and read by header name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};
use crate::library::tags::{self, TagField};
use crate::models::{LibraryEntry, TrackTags};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Untouched,
    Clear,
    Set(String),
}

/// Leaving the tag alone is the default; clearing is asked for per run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blanks {
    #[default]
    LeaveAlone,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEdit {
    pub path: PathBuf,
    pub cells: Vec<(TagField, Cell)>,
}

impl TagEdit {
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

#[derive(Debug, Clone, PartialEq)]
pub struct TagChange {
    pub edit: TagEdit,
    pub current: TrackTags,
    pub proposed: TrackTags,
    /// The fields that differ, in the order a dump lists them.
    pub fields: Vec<TagField>,
}

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

#[derive(Debug, Clone, PartialEq)]
pub enum PlannedEdit {
    Change(Box<TagChange>),
    Skip { path: PathBuf, reason: Skipped },
}

/// A column absent from the header is never written, whatever `blanks` says.
pub fn read_sheet(sheet: &Path, blanks: Blanks) -> Result<Vec<TagEdit>> {
    let bytes = std::fs::read(sheet)
        .map_err(|error| Error::Config(format!("cannot read {}: {error}", sheet.display())))?;
    // The dump writes a BOM for Excel, and the csv crate doesn't strip it.
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

#[must_use]
pub fn plan_changes(edits: &[TagEdit], entries: &[LibraryEntry]) -> Vec<PlannedEdit> {
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
            // Checked now, so a dry run doesn't promise changes that will fail.
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

#[must_use]
pub fn taggable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            tags::TAGGABLE_EXTENSIONS.contains(&format!(".{}", extension.to_lowercase()).as_str())
        })
}

/// Built from a fresh read, not the index; a field changed since the plan is refused.
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
