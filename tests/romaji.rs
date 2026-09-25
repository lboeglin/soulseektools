//! Reading Japanese names as romaji.

use stk::text::romaji::{contains_japanese, to_romaji};

fn assert_romanises(cases: &[(&str, &str)]) {
    for (japanese, romanised) in cases {
        assert_eq!(to_romaji(japanese), *romanised, "to_romaji({japanese:?})");
    }
}

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

/// Spaces in the name survive; IPADIC's word boundaries add none.
#[test]
fn only_the_spaces_in_the_name_survive() {
    assert_romanises(&[
        ("\u{5316}\u{3051}\u{306e}\u{82b1}", "bakenohana"),
        ("\u{5c71}\u{672c} \u{691b}", "yamamoto momiji"),
    ]);
}

/// IPADIC reads these as the ordinary words they are spelled like. See
/// `KNOWN_READINGS`.
#[test]
fn name_readings_survive_transliteration() {
    assert_romanises(&[
        ("\u{521d}\u{97f3}\u{30df}\u{30af}", "hatsunemiku"),
        ("\u{93e1}\u{97f3}\u{30ea}\u{30f3}", "kagaminerin"),
        ("\u{93e1}\u{97f3}\u{30ec}\u{30f3}", "kagamineren"),
        ("\u{91cd}\u{97f3}\u{30c6}\u{30c8}", "kasaneteto"),
        (
            "\u{661f}\u{8857}\u{3059}\u{3044}\u{305b}\u{3044}",
            "hoshimachisuisei",
        ),
    ]);
}

/// The name table must not disturb a reading of the same kanji elsewhere.
#[test]
fn readings_outside_the_name_table_are_untouched() {
    assert_romanises(&[
        ("\u{97f3}\u{697d}", "ongaku"),
        ("\u{91cd}\u{529b}", "juuryoku"),
    ]);
}

/// IPADIC has no entry for these kanji, so each takes its Unihan reading rather
/// than being left as it is.
#[test]
fn kanji_ipadic_cannot_read_take_their_unihan_reading() {
    assert_romanises(&[("\u{53f7}\u{54ed}", "goukoku"), ("\u{6a02}", "gaku")]);
}

#[test]
fn latin_and_digits_in_a_japanese_run_keep_their_spelling() {
    assert_romanises(&[("25\u{6642}", "25ji"), ("miku\u{306e}cd", "mikunocd")]);
}

#[test]
fn half_width_katakana_reads_as_full_width() {
    assert_romanises(&[("\u{ff76}\u{ff9e}\u{ff77}\u{ff9e}", "gagi")]);
}
