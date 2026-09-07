//! The last oracle gate: `dupes content`, over chromaprint fingerprints.
//!
//! The capture was taken with `fpcalc` installed, so it pins chromaprint rather
//! than the loudness-envelope fallback the Python used when it was absent. The
//! fingerprints here are computed by `rusty-chromaprint`, which disagrees with
//! `fpcalc` in about 1.5% of bits -- so what this gate compares is the pairs a
//! library-wide run finds, which is the thing that has to hold, and not the
//! fingerprints themselves, which cannot.
//!
//! To produce what this reads:
//!
//! ```console
//! winget install AcoustID.Chromaprint
//! export PYTHONIOENCODING=utf-8          # until #4 is fixed
//! stk --json dupes content > oracle/dupes-content.json
//! ```
//!
//! Skips while the capture is absent, which is what CI sees.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use stk::duplicates::{self, DEFAULT_AUDIO_THRESHOLD, DEFAULT_DURATION_TOLERANCE_SECONDS};
use stk::library::index::{default_extensions, LibraryIndex};
use stk::library::scan::Excludes;
use support::Scratch;

fn oracle(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../oracle")
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

/// A pair, in the form the capture records it, so order is not compared.
///
/// The Python sorts on score alone over pairs yielded from a `frozenset`, whose
/// order its per-process hash randomisation changes on every run; only the set
/// of pairs is meaningful.
type Pair = (String, String);

fn pairs_from(document: &Value) -> BTreeSet<Pair> {
    document
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|pair| {
            (
                pair["keep"].as_str().unwrap_or_default().to_owned(),
                pair["other"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[test]
fn content_duplicates_match_the_oracle() {
    let (Some(capture), Some(root)) = (oracle("dupes-content.json"), library_root()) else {
        eprintln!(
            "skipping: capture Chromaprint fingerprints first --\n  \
             winget install AcoustID.Chromaprint\n  \
             stk --json dupes content > oracle/dupes-content.json"
        );
        return;
    };

    let scratch = Scratch::new("oracle-dupes-content");
    let mut index = LibraryIndex::open(&scratch.join("index.db")).expect("an index");
    index
        .refresh(
            std::slice::from_ref(&root),
            default_extensions(),
            &Excludes::default(),
            false,
            None,
        )
        .expect("a refresh");
    let entries = index.entries().expect("entries");

    let mut source = duplicates::cached_fingerprints(&mut index);
    let found = duplicates::find_by_audio(
        &entries,
        DEFAULT_AUDIO_THRESHOLD,
        DEFAULT_DURATION_TOLERANCE_SECONDS,
        &mut source,
    );

    let ours: BTreeSet<Pair> = found
        .iter()
        .map(|pair| {
            (
                pair.larger().path.display().to_string(),
                pair.smaller().path.display().to_string(),
            )
        })
        .collect();

    let text = std::fs::read_to_string(&capture).expect("a readable capture");
    let theirs = pairs_from(&serde_json::from_str(&text).expect("valid JSON"));

    let only_oracle: Vec<&Pair> = theirs.difference(&ours).collect();
    let only_ours: Vec<&Pair> = ours.difference(&theirs).collect();

    assert!(
        only_oracle.is_empty() && only_ours.is_empty(),
        "{} pair(s) only in the oracle, {} only here\n  oracle only: {:?}\n  ours only:   {:?}",
        only_oracle.len(),
        only_ours.len(),
        only_oracle.iter().take(5).collect::<Vec<_>>(),
        only_ours.iter().take(5).collect::<Vec<_>>()
    );
}
