//! Artist and title forms covering the hard cases -- kanji IPADIC misreads,
//! small kana, featured credits, full-width symbols -- with their expected
//! `normalize` and `to_romaji` output.

use std::path::Path;

use stk::text::normalize::normalize;
use stk::text::romaji::to_romaji;

struct Row {
    kind: String,
    raw: String,
    normalized: String,
    romaji: String,
}

fn rows() -> Vec<Row> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/names.csv");
    let mut reader = csv::Reader::from_path(&path).expect("tests/data/names.csv is readable");
    reader
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
        .collect()
}

fn assert_all(what: &str, expected: impl Fn(&Row) -> &str, actual: impl Fn(&Row) -> String) {
    let rows = rows();
    let mismatches: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            let got = actual(row);
            (got != expected(row)).then(|| {
                format!(
                    "  {} {:?}\n    expected {:?}\n    got      {got:?}",
                    row.kind,
                    row.raw,
                    expected(row)
                )
            })
        })
        .collect();

    assert!(
        mismatches.is_empty(),
        "{} of {} {what} forms differ:\n{}",
        mismatches.len(),
        rows.len(),
        mismatches
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn normalized_forms_match() {
    assert_all(
        "normalized",
        |row| &row.normalized,
        |row| normalize(&row.raw),
    );
}

#[test]
fn romaji_forms_match() {
    assert_all(
        "romaji",
        |row| &row.romaji,
        |row| to_romaji(&normalize(&row.raw)),
    );
}
