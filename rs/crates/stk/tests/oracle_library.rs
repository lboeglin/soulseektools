//! Step 4's oracle gate: the whole library, as the Python build described it.
//!
//! These rebuild the index from `E:\Music` and check it against the four
//! captures that describe its contents -- `tags.csv`, `stats.json`,
//! `genre.json` and `dupes-titles.json`. Between them they exercise the tag
//! facade, the scan, the index, the matcher, the duplicate pass and the genre
//! map against 266 real files rather than fixtures.
//!
//! `oracle/` is not committed: it names every file in a private library. These
//! skip when it is absent, which is what CI sees.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use stk::library::index::{default_extensions, LibraryIndex};
use stk::matching::MatchThresholds;
use stk::models::LibraryEntry;
use support::Scratch;

fn oracle(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../oracle")
        .join(name);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skipping: {} is absent", path.display());
        None
    }
}

/// The library root the oracle was captured from, read out of `tags.csv` itself
/// so nothing here needs the config layer.
fn library_root() -> Option<PathBuf> {
    let mut reader = csv::Reader::from_path(oracle("tags.csv")?).ok()?;
    let first = reader.records().next()?.ok()?;
    let path = PathBuf::from(first.get(0)?);
    // `<root>/<artist>/<file>`, so the root is two levels up.
    let root = path.parent()?.parent()?.to_path_buf();
    if root.is_dir() {
        Some(root)
    } else {
        eprintln!("skipping: the library at {} is not present", root.display());
        None
    }
}

/// Rebuild the index from scratch, so nothing is inherited from a previous run.
fn scanned(scratch: &Scratch, root: &Path) -> Vec<LibraryEntry> {
    let mut index = LibraryIndex::open(&scratch.join("index.db")).expect("an index");
    index
        .refresh(root, default_extensions(), false, None)
        .expect("a refresh");
    index.entries().expect("entries")
}

fn json(name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(oracle(name)?).ok()?;
    serde_json::from_str(&text).ok()
}

/// Python writes a float with one decimal; `230.3` must not become `230.3000001`.
fn duration_cell(entry: &LibraryEntry) -> String {
    entry
        .duration_seconds
        .map(|seconds| format!("{seconds:.1}"))
        .unwrap_or_default()
}

/// How far a duration may differ from the oracle's before it counts as drift.
///
/// lofty and mutagen disagree on MP3 length by about one frame. Measured on this
/// library: `01 Nekoichi.mp3` is 252.423s to mutagen and to ffprobe -- which
/// agree with each other, so mutagen is almost certainly right -- and 252.456s
/// to lofty; `07. Kuroneko - ...mp3` is 247.947s against 247.992s. Three files
/// of 266, all MP3, all under 50 ms out -- roughly one frame at 320 kbps.
///
/// Nothing the tool decides can turn on that: the tightest duration tolerance
/// anywhere is the duplicate prefilter's 2 seconds, and ranking allows 12. The
/// alternative is an ffprobe per file, which is exactly what the index exists to
/// avoid. So the field is held to a tolerance rather than to the byte, and
/// anything larger is a real regression.
const DURATION_TOLERANCE_SECONDS: f64 = 0.15;

#[test]
fn every_tag_row_matches_the_oracle() {
    let (Some(tags), Some(root)) = (oracle("tags.csv"), library_root()) else {
        return;
    };
    let scratch = Scratch::new("oracle-tags");
    let entries = scanned(&scratch, &root);

    let mut reader = csv::Reader::from_path(tags).expect("oracle/tags.csv is readable");
    let rows: Vec<csv::StringRecord> = reader
        .records()
        .map(|record| record.expect("a well-formed row"))
        .collect();

    assert_eq!(
        rows.len(),
        entries.len(),
        "the oracle has {} rows, the scan found {}",
        rows.len(),
        entries.len()
    );

    let mut differences = Vec::new();
    let mut tolerated = 0_usize;
    for (row, entry) in rows.iter().zip(&entries) {
        let ours = [
            entry.path.display().to_string(),
            entry.folder_artist.clone(),
            entry.tags.artist.clone(),
            entry.tags.album.clone(),
            entry.tags.title.clone(),
            entry.tags.genres.join("; "),
            duration_cell(entry),
        ];
        let names = [
            "path",
            "folder_artist",
            "artist",
            "album",
            "title",
            "genres",
            "duration_seconds",
        ];

        for (index, (name, ours)) in names.iter().zip(&ours).enumerate() {
            let theirs = row.get(index).unwrap_or_default();
            if theirs == ours {
                continue;
            }
            if *name == "duration_seconds" && within_tolerance(theirs, ours) {
                tolerated += 1;
                continue;
            }
            differences.push(format!(
                "{}\n    {name}: oracle {theirs:?}, ours {ours:?}",
                entry.path.display()
            ));
        }
    }

    assert!(
        differences.is_empty(),
        "{} of {} rows differ:\n{}",
        differences.len(),
        rows.len(),
        differences
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Visible rather than silent: if this starts climbing, the decoder changed.
    assert!(
        tolerated <= 3,
        "{tolerated} durations needed the tolerance, expected at most 3"
    );
}

/// Whether two one-decimal durations are the same reading, allowing for the
/// decoder difference described on [`DURATION_TOLERANCE_SECONDS`].
fn within_tolerance(theirs: &str, ours: &str) -> bool {
    match (theirs.parse::<f64>(), ours.parse::<f64>()) {
        (Ok(theirs), Ok(ours)) => (theirs - ours).abs() <= DURATION_TOLERANCE_SECONDS,
        _ => false,
    }
}

#[test]
fn the_library_statistics_match_the_oracle() {
    let (Some(expected), Some(root)) = (json("stats.json"), library_root()) else {
        return;
    };
    let scratch = Scratch::new("oracle-stats");
    let entries = scanned(&scratch, &root);

    assert_eq!(expected["total"], entries.len());
    assert_eq!(
        expected["tagged"],
        entries
            .iter()
            .filter(|entry| !entry.tags.artist.is_empty() || !entry.tags.title.is_empty())
            .count()
    );
    assert_eq!(
        expected["with_artwork"],
        entries.iter().filter(|entry| entry.has_artwork()).count()
    );
    assert_eq!(
        expected["with_genre"],
        entries
            .iter()
            .filter(|entry| !entry.tags.genres.is_empty())
            .count()
    );

    let expected_artists: BTreeMap<String, u64> = expected["artists"]
        .as_object()
        .expect("an artist table")
        .iter()
        .map(|(artist, count)| (artist.clone(), count.as_u64().unwrap_or(0)))
        .collect();
    let mut ours: BTreeMap<String, u64> = BTreeMap::new();
    for entry in &entries {
        *ours.entry(entry.folder_artist.clone()).or_default() += 1;
    }

    assert_eq!(ours, expected_artists);
}

#[test]
fn the_genre_totals_match_the_oracle() {
    let (Some(expected), Some(root)) = (json("genre.json"), library_root()) else {
        return;
    };
    let scratch = Scratch::new("oracle-genre");
    let entries = scanned(&scratch, &root);

    let expected_genres: BTreeMap<String, u64> = expected["genres"]
        .as_object()
        .expect("a genre table")
        .iter()
        .map(|(genre, count)| (genre.clone(), count.as_u64().unwrap_or(0)))
        .collect();

    let mut ours: BTreeMap<String, u64> = BTreeMap::new();
    for entry in &entries {
        for genre in &entry.tags.genres {
            *ours.entry(genre.clone()).or_default() += 1;
        }
    }

    assert_eq!(ours, expected_genres);
    assert_eq!(
        expected["untagged"],
        entries
            .iter()
            .filter(|entry| entry.tags.genres.is_empty())
            .count()
    );
}

#[test]
fn every_title_duplicate_matches_the_oracle() {
    let (Some(expected), Some(root)) = (json("dupes-titles.json"), library_root()) else {
        return;
    };
    let scratch = Scratch::new("oracle-dupes");
    let entries = scanned(&scratch, &root);

    let pairs = stk::duplicates::find_by_title(&entries, &MatchThresholds::default());
    let ours: Vec<(String, String, String)> = pairs
        .iter()
        .map(|pair| {
            (
                pair.larger().path.display().to_string(),
                pair.smaller().path.display().to_string(),
                pair.evidence.clone(),
            )
        })
        .collect();

    let theirs: Vec<(String, String, String)> = expected
        .as_array()
        .expect("a list of pairs")
        .iter()
        .map(|pair| {
            (
                pair["keep"].as_str().unwrap_or_default().to_owned(),
                pair["other"].as_str().unwrap_or_default().to_owned(),
                pair["evidence"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();

    // The Python sorts by score and the ordering of equal scores is not
    // meaningful, so the comparison is set-wise on the pairs themselves.
    let mut ours_sorted = ours.clone();
    let mut theirs_sorted = theirs.clone();
    ours_sorted.sort();
    theirs_sorted.sort();

    assert_eq!(
        ours_sorted.len(),
        theirs_sorted.len(),
        "the oracle found {} pairs, we found {}",
        theirs_sorted.len(),
        ours_sorted.len()
    );
    assert_eq!(ours_sorted, theirs_sorted);
}

#[test]
fn the_artwork_audit_matches_the_oracle() {
    let (Some(expected), Some(root)) = (json("art.json"), library_root()) else {
        return;
    };
    let scratch = Scratch::new("oracle-art");
    let entries = scanned(&scratch, &root);

    let statuses = stk::artwork::service::audit(&entries, &stk::config::ArtworkSettings::default());
    let needing: Vec<String> = statuses
        .iter()
        .filter(|status| status.needs_artwork)
        .map(|status| status.entry.path.display().to_string())
        .collect();

    // The capture is an empty list: every file already has usable artwork.
    let theirs = expected.as_array().map_or(0, Vec::len);
    assert_eq!(
        needing.len(),
        theirs,
        "the oracle wants artwork for {theirs} files, we want it for {}:\n{}",
        needing.len(),
        needing
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
