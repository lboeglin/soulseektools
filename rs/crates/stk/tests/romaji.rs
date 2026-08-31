//! Ported from `py/tests/unit/test_romaji.py`.
//!
//! The test there that scores a diverging artist reading against a strong title
//! needs `matching`, and is ported with it in step 2.

use stk::text::romaji::{contains_japanese, to_romaji};

#[test]
fn contains_japanese_detects_the_scripts_worth_transliterating() {
    for (text, expected) in [
        ("\u{5343}\u{672c}\u{685c}", true),
        ("\u{30e9}\u{30b0}\u{30c8}\u{30ec}\u{30a4}\u{30f3}", true),
        ("\u{3072}\u{3089}\u{304c}\u{306a}", true),
        ("Senbonzakura", false),
        ("DECO*27", false),
        ("", false),
    ] {
        assert_eq!(
            contains_japanese(text),
            expected,
            "contains_japanese({text:?})"
        );
    }
}

#[test]
fn to_romaji_matches_the_romanised_spelling_used_in_filenames() {
    for (japanese, romanised) in [
        ("\u{5343}\u{672c}\u{685c}", "senbonzakura"),
        (
            "\u{30e9}\u{30b0}\u{30c8}\u{30ec}\u{30a4}\u{30f3}",
            "ragutorein",
        ),
        ("\u{521d}\u{97f3}\u{30df}\u{30af}", "hatsunemiku"),
        ("\u{98f4}\u{3068}\u{9396}", "ametokusari"),
    ] {
        assert_eq!(
            to_romaji(japanese).replace(' ', ""),
            romanised,
            "to_romaji({japanese:?})"
        );
    }
}

#[test]
fn to_romaji_normalises_text_without_japanese_characters() {
    assert_eq!(to_romaji("DECO*27"), "deco 27");
}

#[test]
fn to_romaji_is_idempotent() {
    let once = to_romaji("\u{5343}\u{672c}\u{685c}");
    assert_eq!(to_romaji(&once), once);
}

/// The `kakasi` crate reads these character by character; pykakasi, and the
/// oracle, carry the name reading. See `KNOWN_READINGS`.
#[test]
fn compound_name_readings_survive_transliteration() {
    for (japanese, romanised) in [
        ("\u{521d}\u{97f3}\u{30df}\u{30af}", "hatsunemiku"),
        ("\u{93e1}\u{97f3}\u{30ea}\u{30f3}", "kagaminerin"),
        ("\u{93e1}\u{97f3}\u{30ec}\u{30f3}", "kagamineren"),
    ] {
        assert_eq!(to_romaji(japanese), romanised, "to_romaji({japanese:?})");
    }
}

/// The compound table must not disturb a reading the crate already gets right.
#[test]
fn readings_the_transliterator_already_agrees_on_are_untouched() {
    for (japanese, romanised) in [
        ("\u{91cd}\u{97f3}\u{30c6}\u{30c8}", "juuonteto"),
        ("\u{97f3}\u{697d}", "ongaku"),
    ] {
        assert_eq!(to_romaji(japanese), romanised, "to_romaji({japanese:?})");
    }
}
