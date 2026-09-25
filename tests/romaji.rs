//! Reading Japanese names as romaji.

use stk::text::romaji::to_romaji;

fn assert_romanises(cases: &[(&str, &str)]) {
    for (japanese, romanised) in cases {
        assert_eq!(to_romaji(japanese), *romanised, "to_romaji({japanese:?})");
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
fn only_the_spaces_in_the_name_survive() {
    assert_romanises(&[
        ("\u{5316}\u{3051}\u{306e}\u{82b1}", "bakenohana"),
        ("\u{5c71}\u{672c} \u{691b}", "yamamoto momiji"),
    ]);
}

/// IPADIC reads these as ordinary words. See `KNOWN_READINGS`.
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

#[test]
fn kanji_ipadic_cannot_read_take_their_unihan_reading() {
    assert_romanises(&[("\u{53f7}\u{54ed}", "goukoku"), ("\u{6a02}", "gaku")]);
}
