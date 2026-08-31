//! Step 1's oracle gate: every distinct name form in the real library.
//!
//! `oracle/romaji.csv` holds 434 rows of `raw`, `normalized` and `romaji`,
//! dumped from the index the Python build wrote. `normalized` is `normalize(raw)`
//! and `romaji` is `to_romaji(normalize(raw))`, so both columns are reproducible
//! from `raw` alone -- no index, no library, no ffmpeg.
//!
//! The file is not committed: it names artists and titles from a private library.
//! These tests skip when it is absent, which is what CI sees.

use std::path::{Path, PathBuf};

use stk::text::normalize::normalize;
use stk::text::romaji::to_romaji;

struct Row {
    kind: String,
    raw: String,
    normalized: String,
    romaji: String,
}

fn oracle_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/romaji.csv")
}

fn rows() -> Option<Vec<Row>> {
    let path = oracle_path();
    if !path.is_file() {
        eprintln!("skipping: {} is absent", path.display());
        return None;
    }

    let mut reader = csv::Reader::from_path(&path).expect("oracle/romaji.csv is readable");
    let rows = reader
        .records()
        .map(|record| {
            let record = record.expect("a well-formed row");
            Row {
                kind: record[0].to_owned(),
                raw: record[1].to_owned(),
                normalized: record[2].to_owned(),
                romaji: record[3].to_owned(),
            }
        })
        .collect();
    Some(rows)
}

fn report(what: &str, mismatches: &[(String, String, String, String)], total: usize) -> String {
    let mut lines = vec![format!(
        "{} of {total} {what} forms differ from the oracle:",
        mismatches.len()
    )];
    for (kind, raw, expected, got) in mismatches.iter().take(25) {
        lines.push(format!(
            "  {kind} {raw:?}\n    oracle {expected:?}\n    ours   {got:?}"
        ));
    }
    if mismatches.len() > 25 {
        lines.push(format!("  ... and {} more", mismatches.len() - 25));
    }
    lines.join("\n")
}

#[test]
fn normalized_forms_match_the_oracle() {
    let Some(rows) = rows() else { return };

    let mismatches: Vec<_> = rows
        .iter()
        .filter(|row| normalize(&row.raw) != row.normalized)
        .map(|row| {
            (
                row.kind.clone(),
                row.raw.clone(),
                row.normalized.clone(),
                normalize(&row.raw),
            )
        })
        .collect();

    assert!(
        mismatches.is_empty(),
        "{}",
        report("normalized", &mismatches, rows.len())
    );
}

#[test]
fn romaji_forms_match_the_oracle() {
    let Some(rows) = rows() else { return };

    let mismatches: Vec<_> = rows
        .iter()
        .filter(|row| to_romaji(&normalize(&row.raw)) != row.romaji)
        .map(|row| {
            (
                row.kind.clone(),
                row.raw.clone(),
                row.romaji.clone(),
                to_romaji(&normalize(&row.raw)),
            )
        })
        .collect();

    assert!(
        mismatches.is_empty(),
        "{}",
        report("romaji", &mismatches, rows.len())
    );
}
