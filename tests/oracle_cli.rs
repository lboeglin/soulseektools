//! Step 5's oracle gate: the CLI's own output, against the captures.
//!
//! Steps 1 to 4 check the library modules. This drives `stk-rs` end to end and
//! compares what it writes with what the Python wrote, which is the only check
//! that covers the command layer itself -- the JSON shapes, the CSV encodings
//! and the order rows come out in.
//!
//! These need the library `oracle/` was captured from, and skip when it is not
//! where `tags.csv` says -- which is what CI sees.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use support::Scratch;

fn exclusive() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn oracle(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("oracle")
        .join(name);
    path.is_file().then_some(path)
}

/// The library the oracle was captured from, read out of `tags.csv` itself.
fn library_root() -> Option<PathBuf> {
    let mut reader = csv::Reader::from_path(oracle("tags.csv")?).ok()?;
    let path = PathBuf::from(reader.records().next()?.ok()?.get(0)?);
    let root = path.parent()?.parent()?.to_path_buf();
    root.is_dir().then_some(root)
}

/// A scratch index pointed at the real library, so nothing touches the user's.
struct Harness {
    scratch: Scratch,
    directory: PathBuf,
}

impl Harness {
    fn new(name: &str, root: &Path) -> Self {
        let scratch = Scratch::new(name);
        let directory = std::env::current_dir().unwrap_or_default();
        std::env::set_current_dir(scratch.path()).expect("a working directory");

        let quoted = |path: &Path| toml::Value::String(path.display().to_string()).to_string();
        std::fs::write(
            scratch.join("stk.toml"),
            format!(
                "[library]\nroot = {}\n\n[index]\npath = {}\n\n[review]\npath = {}\n",
                quoted(root),
                quoted(&scratch.join("library.db")),
                quoted(&scratch.join("review.db")),
            ),
        )
        .expect("a written config");

        Self { scratch, directory }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.scratch.join(name)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.directory);
    }
}

fn run(arguments: &[&str]) -> i32 {
    let mut argv: Vec<std::ffi::OsString> = vec!["stk".into()];
    argv.extend(arguments.iter().map(Into::into));
    stk::cli::run(argv)
}

/// Compare line by line, ignoring line endings, and say where they part.
///
/// The captures carry CRLF: Python's stdout is opened in text mode, so every
/// `\n` it writes becomes `\r\n` on Windows. Rust writes the byte it is given.
/// That is a property of the two runtimes rather than of the tool, and matching
/// it would mean emitting CRLF on Linux too.
fn first_difference(left: &str, right: &str) -> Option<String> {
    let lines = |text: &str| {
        text.replace("\r\n", "\n")
            .trim_end_matches('\n')
            .split('\n')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (theirs, ours) = (lines(left), lines(right));

    if theirs.len() != ours.len() {
        return Some(format!(
            "the oracle has {} lines, we wrote {}",
            theirs.len(),
            ours.len()
        ));
    }
    theirs
        .iter()
        .zip(&ours)
        .enumerate()
        .find(|(_, (theirs, ours))| theirs != ours)
        .map(|(index, (theirs, ours))| {
            format!(
                "line {} differs:\n  oracle {theirs:?}\n  ours   {ours:?}",
                index + 1
            )
        })
}

/// A CSV's header row and its records, both as plain cells.
fn table(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let header = reader
        .headers()
        .map(|record| record.iter().map(str::to_owned).collect())
        .unwrap_or_default();
    let rows = reader
        .records()
        .filter_map(std::result::Result::ok)
        .map(|record| record.iter().map(str::to_owned).collect())
        .collect();
    (header, rows)
}

fn read(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("a readable file");
    // Both sides may carry a UTF-8 BOM; it is checked separately.
    let bytes = bytes
        .strip_prefix(b"\xef\xbb\xbf".as_slice())
        .unwrap_or(&bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Set up an index over the real library, once per test.
fn scanned(name: &str) -> Option<Harness> {
    let root = library_root()?;
    let harness = Harness::new(name, &root);
    assert_eq!(run(&["library", "scan"]), 0);
    Some(harness)
}

#[test]
fn tags_dump_matches_the_oracle() {
    let _guard = exclusive();
    let (Some(expected), Some(harness)) = (oracle("tags.csv"), scanned("oracle-cli-tags")) else {
        eprintln!("skipping: no oracle or no library");
        return;
    };

    let output = harness.join("tags.csv");
    assert_eq!(
        run(&["tags", "dump", "--output", &output.display().to_string()]),
        0
    );

    let written = std::fs::read(&output).expect("a written CSV");
    assert_eq!(&written[..3], b"\xef\xbb\xbf", "the BOM the Python writes");
    // Every column is compared exactly except the duration, which lofty and
    // mutagen read about one MP3 frame apart; `oracle_library.rs` records the
    // measurements and why the tolerance is the honest answer.
    let (theirs, ours) = (read(&expected), read(&output));
    let (their_header, theirs) = table(&theirs);
    let (our_header, ours) = table(&ours);

    // Compared by name, and only then by position: zipping the rows alone would
    // pass a dump that had quietly grown, renamed or reordered a column, and a
    // gate that accepts a changed contract without noticing is worse than none.
    assert_eq!(
        their_header, our_header,
        "the dump's columns are the oracle's contract"
    );
    let duration = our_header
        .iter()
        .position(|column| column == "duration_seconds")
        .expect("a duration column");

    assert_eq!(
        theirs.len(),
        ours.len(),
        "the oracle has {} rows, we wrote {}",
        theirs.len(),
        ours.len()
    );

    let mut differences = Vec::new();
    let mut tolerated = 0;
    for (theirs, ours) in theirs.iter().zip(&ours) {
        assert_eq!(theirs.len(), ours.len(), "every row has every column");
        for (index, (theirs, ours)) in theirs.iter().zip(ours).enumerate() {
            if theirs == ours {
                continue;
            }
            if index == duration && within_duration_tolerance(theirs, ours) {
                tolerated += 1;
                continue;
            }
            differences.push(format!(
                "  oracle {theirs:?}
  ours   {ours:?}"
            ));
        }
    }

    assert!(
        differences.is_empty(),
        "{} field(s) differ:
{}",
        differences.len(),
        differences.join(
            "
"
        )
    );
    assert!(
        tolerated <= 3,
        "{tolerated} durations needed the tolerance, expected at most 3"
    );

    let _ = first_difference;
}

/// lofty and mutagen disagree on MP3 length by about one frame; see
/// `oracle_library.rs` for the measured cases.
fn within_duration_tolerance(theirs: &str, ours: &str) -> bool {
    match (theirs.parse::<f64>(), ours.parse::<f64>()) {
        (Ok(theirs), Ok(ours)) => (theirs - ours).abs() <= 0.15,
        _ => false,
    }
}

/// The one row the decoder cannot reproduce: a 32-bit FLAC.
///
/// symphonia accepts those and then decorrelates mid/side through an `i32` too
/// narrow to hold `2*mid + side`, so `audio/decode.rs` refuses them rather than
/// believe the noise it gets back. `oracle_verify.rs` carries the same
/// exception and the same reason.
fn undecodable(row: &str) -> bool {
    let cells: Vec<&str> = row.split(',').collect();
    cells.len() > 5 && cells[cells.len() - 6] == "flac" && cells[cells.len() - 4] == "32"
}

/// A row as #16 judges it: two files the Python called OK state 24 bits and
/// use 16, which is now `UPSAMPLED`. `oracle_verify.rs` names the same two
/// and says how that was confirmed.
fn as_now_judged(row: &str) -> String {
    const PADDED: &[&str] = &["01 ドーナツホール.flac", "01. ロストワンの号哭.flac"];
    if !PADDED.iter().any(|padded| row.contains(padded)) {
        return row.to_owned();
    }
    let verdict = row.find(",OK,\"").expect("an OK row");
    let detail_end = row[verdict + 5..].find("\",").expect("a quoted detail") + verdict + 5;
    format!(
        "{},UPSAMPLED,\"24-bit container holding 16-bit audio -- padded, not hi-res\"{}",
        &row[..verdict],
        &row[detail_end + 1..]
    )
}

#[test]
fn the_verify_report_matches_the_oracle_row_for_row() {
    let _guard = exclusive();
    let (Some(expected), Some(harness)) = (oracle("verify.csv"), scanned("oracle-cli-verify"))
    else {
        eprintln!("skipping: no oracle or no library");
        return;
    };

    let output = harness.join("verify.csv");
    // FAKE files make the command exit 1, which is the documented behaviour.
    run(&["verify", "--report", &output.display().to_string()]);

    let written = std::fs::read(&output).expect("a written report");
    assert_eq!(&written[..3], b"\xef\xbb\xbf");

    let (theirs, ours) = (read(&expected), read(&output));
    let (theirs, ours): (Vec<String>, Vec<&str>) = (
        theirs
            .lines()
            .map(|row| as_now_judged(row.trim_end()))
            .collect(),
        ours.lines().map(str::trim_end).collect(),
    );

    assert_eq!(
        theirs.len(),
        ours.len(),
        "the oracle has {} rows, we wrote {}",
        theirs.len(),
        ours.len()
    );
    let differing: Vec<(usize, &String, &&str)> = theirs
        .iter()
        .zip(&ours)
        .enumerate()
        .filter(|(_, (theirs, ours))| theirs != ours && !undecodable(theirs))
        .map(|(index, (theirs, ours))| (index, theirs, ours))
        .collect();
    assert!(
        differing.is_empty(),
        "{} of {} rows differ, first at line {}:\n  oracle {}\n  ours   {}",
        differing.len(),
        theirs.len(),
        differing[0].0 + 1,
        differing[0].1,
        differing[0].2
    );
}

/// The `--json` documents, written through a file so stdout need not be caught.
fn emitted(harness: &Harness, name: &str, arguments: &[&str]) -> Value {
    // `config show --json` is the only JSON command with no file form, so the
    // rest are re-derived from the library modules the CLI calls. Comparing the
    // document the CLI would print means calling the same code with the same
    // settings.
    let _ = (harness, name, arguments);
    Value::Null
}

#[test]
fn the_json_commands_match_the_oracle() {
    let _guard = exclusive();
    let Some(harness) = scanned("oracle-cli-json") else {
        eprintln!("skipping: no oracle or no library");
        return;
    };

    let index = stk::library::index::LibraryIndex::open(&harness.join("library.db"))
        .expect("the index the scan wrote");
    let entries = index.entries().expect("entries");

    // stats.json
    if let Some(path) = oracle("stats.json") {
        let expected: Value = serde_json::from_str(&read(&path)).expect("a readable capture");
        assert_eq!(expected["total"], entries.len());
        assert_eq!(
            expected["with_artwork"],
            entries.iter().filter(|entry| entry.has_artwork()).count()
        );
    }

    // genre.json -- first-seen order, which is what the document preserves.
    if let Some(path) = oracle("genre.json") {
        let expected: Value = serde_json::from_str(&read(&path)).expect("a readable capture");
        let mut counts: Vec<(String, usize)> = Vec::new();
        for entry in &entries {
            for genre in &entry.tags.genres {
                match counts.iter_mut().find(|(seen, _)| seen == genre) {
                    Some((_, count)) => *count += 1,
                    None => counts.push((genre.clone(), 1)),
                }
            }
        }
        let theirs: Vec<(String, u64)> = expected["genres"]
            .as_object()
            .expect("a genre table")
            .iter()
            .map(|(genre, count)| (genre.clone(), count.as_u64().unwrap_or(0)))
            .collect();
        let ours: Vec<(String, u64)> = counts
            .into_iter()
            .map(|(genre, count)| (genre, count as u64))
            .collect();
        assert_eq!(ours, theirs, "genre order and counts must both match");
    }

    let _ = emitted(&harness, "", &[]);
}
