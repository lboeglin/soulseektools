//! The Rust port cannot reuse `\w` or `\s`; these hold its own classes to
//! Python's, from fixtures `scripts/word_chars.py` generates.
//!
//! Both differences are silent rather than loud. Rust's `\w` is `\p{Word}`,
//! which matches 10,549 combining marks Python folds to a space; Rust's `\s`
//! omits U+001C to U+001F, which Python matches. Every comparison in the project
//! flows through `normalize()`, so either would shift matching scores globally.

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;

fn codepoints(name: &str) -> BTreeSet<u32> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

    let mut set = BTreeSet::new();
    for line in text.lines().filter(|line| !line.starts_with('#')) {
        let (low, high) = line.split_once("..").expect("a hex range per line");
        let low = u32::from_str_radix(low, 16).expect("hex");
        let high = u32::from_str_radix(high, 16).expect("hex");
        set.extend(low..=high);
    }
    set
}

/// Report where `predicate` and the Python fixture disagree, over assigned
/// codepoints only.
///
/// Rust's Unicode tables are newer than the running Python's, so codepoints
/// Python does not yet know about are out of scope: nothing the oracle captured
/// can contain one.
fn disagreements(fixture: &str, predicate: impl Fn(char) -> bool) -> (Vec<u32>, Vec<u32>) {
    let expected = codepoints(fixture);
    let assigned = codepoints("python_assigned_chars.txt");

    let (mut python_only, mut rust_only) = (Vec::new(), Vec::new());
    for codepoint in assigned {
        let Some(character) = char::from_u32(codepoint) else {
            continue;
        };
        match (expected.contains(&codepoint), predicate(character)) {
            (true, false) => python_only.push(codepoint),
            (false, true) => rust_only.push(codepoint),
            _ => {}
        }
    }
    (python_only, rust_only)
}

fn describe(codepoints: &[u32]) -> String {
    codepoints
        .iter()
        .take(8)
        .map(|codepoint| format!("U+{codepoint:04X}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn the_word_class_matches_pythons() {
    let class = Regex::new(r"^[\p{L}\p{Nd}\p{Nl}\p{No}_]$").unwrap();
    let (python_only, rust_only) = disagreements("python_word_chars.txt", |character| {
        class.is_match(character.encode_utf8(&mut [0u8; 4]))
    });

    assert!(
        python_only.is_empty(),
        "{} codepoints are word characters to Python but not here: {}",
        python_only.len(),
        describe(&python_only)
    );
    assert!(
        rust_only.is_empty(),
        "{} codepoints are word characters here but not to Python: {}",
        rust_only.len(),
        describe(&rust_only)
    );
}

#[test]
fn the_whitespace_class_matches_pythons() {
    let (python_only, rust_only) = disagreements("python_space_chars.txt", |character| {
        character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
    });

    assert!(
        python_only.is_empty(),
        "{} codepoints are whitespace to Python but not here: {}",
        python_only.len(),
        describe(&python_only)
    );
    assert!(
        rust_only.is_empty(),
        "{} codepoints are whitespace here but not to Python: {}",
        rust_only.len(),
        describe(&rust_only)
    );
}

/// The reason the classes above are spelled out rather than written `\w` / `\s`.
#[test]
fn the_builtin_classes_would_not_have_matched() {
    let builtin = Regex::new(r"^\w$").unwrap();
    let (_, word_only) = disagreements("python_word_chars.txt", |character| {
        builtin.is_match(character.encode_utf8(&mut [0u8; 4]))
    });
    assert!(
        !word_only.is_empty(),
        "Rust's word class agrees with Python's now, so the spelled-out class \
         and this test can go"
    );

    let (space_only, _) = disagreements("python_space_chars.txt", char::is_whitespace);
    assert_eq!(
        space_only,
        vec![0x1C, 0x1D, 0x1E, 0x1F],
        "White_Space is expected to miss exactly the four separator controls"
    );
}
