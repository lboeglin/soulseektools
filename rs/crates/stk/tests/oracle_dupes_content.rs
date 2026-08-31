//! The last oracle gate: `dupes content`, over chromaprint fingerprints.
//!
//! Held back from step 3 because `fpcalc` was not installed when the oracle was
//! captured. Diffing against the loudness-envelope fallback instead would have
//! pinned the wrong backend as the reference, so `oracle/README.md` asks for
//! Chromaprint to be installed and the capture taken before this is trusted.
//!
//! To produce what this reads:
//!
//! ```console
//! winget install AcoustID.Chromaprint
//! export PYTHONIOENCODING=utf-8          # until #4 is fixed
//! stk --json dupes content > oracle/dupes-content.json
//! ```
//!
//! Skips while either the capture or `fpcalc` is absent, which is what CI sees.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use stk::audio::fingerprint::{available_kind, FingerprintKind};
use stk::duplicates::{self, DEFAULT_AUDIO_THRESHOLD, DEFAULT_DURATION_TOLERANCE_SECONDS};
use stk::library::index::{default_extensions, LibraryIndex};
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

    // The capture is only meaningful against the same backend that produced it.
    // Absent fpcalc this would compare chromaprint against the envelope
    // fallback, which is worse than not running at all: skip, and say so.
    if available_kind() != FingerprintKind::Chromaprint {
        eprintln!(
            "skipping: oracle/dupes-content.json was captured with fpcalc \n             installed, and it is not on PATH here"
        );
        return;
    }

    let scratch = Scratch::new("oracle-dupes-content");
    let mut index = LibraryIndex::open(&scratch.join("index.db")).expect("an index");
    index
        .refresh(&root, default_extensions(), false, None)
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

/// Which backend is used decides what a fingerprint even means, so the choice
/// is worth pinning separately from the pairs it produces.
#[test]
fn the_chromaprint_backend_is_chosen_when_fpcalc_is_installed() {
    if stk::audio::fingerprint::fpcalc_path().is_none() {
        eprintln!("skipping: fpcalc is not on PATH");
        return;
    }
    assert_eq!(available_kind(), FingerprintKind::Chromaprint);
}
