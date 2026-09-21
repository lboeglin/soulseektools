//! Step 3's oracle gate: the verify verdict for every lossless file in the
//! real library.
//!
//! `oracle/verify.csv` holds 258 rows -- 248 OK, 9 FAKE, 1 SUSPECT -- with the
//! path, verdict, detail, stream properties, cutoff and steepness the Python
//! build produced. The paths come from the file itself, so this needs neither
//! config loading nor a directory walk, only the library still being where it
//! was.
//!
//! This skips when the library the rows point at is absent -- which is what CI
//! sees.
//!
//! `oracle/README.md` warns that these verdicts are a strong reference rather
//! than ground truth: the classifier had never seen a real file before that run.
//! A disagreement is a reason to look at the file, not to assume either side.

use std::path::{Path, PathBuf};

use stk::audio::verify::verify_files;
use stk::config::VerifySettings;
use stk::models::VerifyResult;

struct Row {
    path: PathBuf,
    verdict: String,
    detail: String,
    codec: String,
    sample_rate: String,
    bit_depth: String,
    cutoff_hz: String,
    nyquist_hz: String,
    steepness: String,
}

fn oracle_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/verify.csv")
}

fn rows() -> Option<Vec<Row>> {
    let path = oracle_path();
    if !path.is_file() {
        eprintln!("skipping: {} is absent", path.display());
        return None;
    }

    // The Python writes this file as utf-8-sig; the csv crate does not strip a
    // BOM, so the first header name would otherwise carry one.
    let mut reader = csv::Reader::from_path(&path).expect("oracle/verify.csv is readable");
    let rows: Vec<Row> = reader
        .records()
        .map(|record| {
            let record = record.expect("a well-formed row");
            Row {
                path: PathBuf::from(&record[0]),
                verdict: record[1].to_owned(),
                detail: record[2].to_owned(),
                codec: record[3].to_owned(),
                sample_rate: record[4].to_owned(),
                bit_depth: record[5].to_owned(),
                cutoff_hz: record[6].to_owned(),
                nyquist_hz: record[7].to_owned(),
                steepness: record[8].to_owned(),
            }
        })
        .collect();

    if !rows.first().is_some_and(|row| row.path.is_file()) {
        eprintln!(
            "skipping: the library {} names is not present",
            path.display()
        );
        return None;
    }
    Some(rows)
}

/// Render an optional numeric field the way the Python's CSV writer does: a
/// `None` becomes an empty cell.
fn cell<T: ToString>(value: Option<T>) -> String {
    value.map(|inner| inner.to_string()).unwrap_or_default()
}

fn steepness_cell(result: &VerifyResult) -> String {
    // Python writes a float, so 27.9 stays "27.9" and a whole number keeps its
    // ".0"; Rust's `to_string` drops the trailing zero.
    result
        .steepness_db_per_khz
        .map(|value| format!("{value:.1}"))
        .unwrap_or_default()
}

/// The one row the decoder cannot reproduce.
///
/// symphonia accepts 32-bit FLAC and then decorrelates mid/side through an
/// `i32` too narrow to hold `2*mid + side`: it panics on a debug build and
/// returns noise on a release one. `audio/decode.rs` refuses those files rather
/// than believe them, so the row that carried a verdict now carries an error.
/// It is one file of 258, and ffmpeg is no longer here to read it.
fn undecodable(row: &Row) -> bool {
    row.codec == "flac" && row.bit_depth == "32"
}

fn differences(row: &Row, result: &VerifyResult) -> Vec<String> {
    let mut found = Vec::new();
    let mut compare = |field: &str, expected: &str, got: String| {
        if expected != got {
            found.push(format!("{field}: oracle {expected:?}, ours {got:?}"));
        }
    };

    compare("verdict", &row.verdict, result.verdict.to_string());
    compare("detail", &row.detail, result.detail.clone());
    compare("codec", &row.codec, result.codec.clone());
    compare("sample_rate", &row.sample_rate, cell(result.sample_rate));
    compare("bit_depth", &row.bit_depth, cell(result.bit_depth));
    compare("cutoff_hz", &row.cutoff_hz, cell(result.cutoff_hz));
    compare("nyquist_hz", &row.nyquist_hz, cell(result.nyquist_hz));
    compare(
        "steepness_db_per_khz",
        &row.steepness,
        steepness_cell(result),
    );
    found
}

#[test]
fn every_verdict_matches_the_oracle() {
    let Some(rows) = rows() else { return };

    let paths: Vec<PathBuf> = rows.iter().map(|row| row.path.clone()).collect();
    let results = verify_files(&paths, &VerifySettings::default());

    let mut report = Vec::new();
    for (row, result) in rows.iter().zip(&results) {
        if undecodable(row) {
            eprintln!("skipping {}: 32-bit FLAC", row.path.display());
            continue;
        }
        let found = differences(row, result);
        if !found.is_empty() {
            report.push(format!(
                "{}\n    {}",
                row.path.display(),
                found.join("\n    ")
            ));
        }
    }

    assert!(
        report.is_empty(),
        "{} of {} files differ from the oracle:\n{}",
        report.len(),
        rows.len(),
        report
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
