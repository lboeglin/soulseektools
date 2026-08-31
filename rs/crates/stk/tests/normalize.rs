//! Ported from `py/tests/unit/test_normalize.py`.

use stk::text::normalize::{
    normalize, sanitize_folder_name, strip_artist_prefix, strip_track_number, UNKNOWN_ARTIST,
};

#[test]
fn normalize_folds_case_punctuation_and_whitespace() {
    for (raw, expected) in [
        ("Senbonzakura", "senbonzakura"),
        ("DECO*27", "deco 27"),
        ("Wonderful\u{2605}opportunity!", "wonderful opportunity"),
        ("LUVORATORRRRRY!", "luvoratorrrrry"),
        ("  spaced   out  ", "spaced out"),
        ("", ""),
    ] {
        assert_eq!(normalize(raw), expected, "normalize({raw:?})");
    }
}

#[test]
fn normalize_drops_featured_credits_and_bracketed_suffixes() {
    for (raw, expected) in [
        (
            "Mesmerizer (feat. \u{521d}\u{97f3}\u{30df}\u{30af}&\u{91cd}\u{97f3}\u{30c6}\u{30c8})",
            "mesmerizer",
        ),
        ("Roki feat. Hatsune Miku", "roki"),
        (
            "Yowamushi Montblanc (sasakure.UK Remix)",
            "yowamushi montblanc",
        ),
        ("Shiny Happy Days [16-44kHz]", "shiny happy days"),
    ] {
        assert_eq!(normalize(raw), expected, "normalize({raw:?})");
    }
}

#[test]
fn normalize_applies_nfkc_so_fullwidth_matches_halfwidth() {
    assert_eq!(
        normalize("\u{ff2d}\u{ff29}\u{ff2b}\u{ff35}"),
        normalize("MIKU")
    );
}

#[test]
fn strip_track_number_drops_a_leading_number() {
    for (raw, expected) in [
        ("01 - Roki", "Roki"),
        (
            "15. \u{30ca}\u{30a4}\u{30c8}\u{30fb}\u{30aa}\u{30d6}\u{30fb}\u{30ca}\u{30a4}\u{30c4}",
            "\u{30ca}\u{30a4}\u{30c8}\u{30fb}\u{30aa}\u{30d6}\u{30fb}\u{30ca}\u{30a4}\u{30c4}",
        ),
        ("(04) Reisen Occultia", "Reisen Occultia"),
        ("4 - LUVORATORRRRRY!", "LUVORATORRRRRY!"),
        ("29 Rebirth the edge", "Rebirth the edge"),
        ("Roki", "Roki"),
    ] {
        assert_eq!(
            strip_track_number(raw),
            expected,
            "strip_track_number({raw:?})"
        );
    }
}

#[test]
fn strip_track_number_keeps_a_title_that_is_only_a_number() {
    assert_eq!(strip_track_number("39"), "39");
}

#[test]
fn strip_artist_prefix_removes_a_leading_artist() {
    for (stem, artist, expected) in [
        ("DECO*27 - Roki", "DECO_27", "Roki"),
        (
            "06 DECO-27 - Yowamushi Montblanc",
            "DECO*27",
            "Yowamushi Montblanc",
        ),
        (
            "beatMARIO - Night of Knights",
            "beatMARIO",
            "Night of Knights",
        ),
        (
            "\u{7a32}\u{8449}\u{66c7} - \u{30e9}\u{30b0}\u{30c8}\u{30ec}\u{30a4}\u{30f3}",
            "\u{7a32}\u{8449}\u{66c7}",
            "\u{30e9}\u{30b0}\u{30c8}\u{30ec}\u{30a4}\u{30f3}",
        ),
        ("Just A Title", "Some Artist", "Just A Title"),
    ] {
        assert_eq!(
            strip_artist_prefix(stem, artist),
            expected,
            "strip_artist_prefix({stem:?}, {artist:?})"
        );
    }
}

#[test]
fn strip_artist_prefix_survives_spellings_of_differing_length() {
    // A literal-length slice would cut "Melt" short here, because the folder name
    // is far longer than its normalised form.
    assert_eq!(
        strip_artist_prefix("ryo (supercell) - Melt", "ryo (supercell)"),
        "Melt"
    );
}

#[test]
fn strip_artist_prefix_keeps_the_name_when_it_is_the_whole_stem() {
    assert_eq!(strip_artist_prefix("ZUN", "ZUN"), "ZUN");
}

#[test]
fn sanitize_folder_name_replaces_characters_a_filesystem_rejects() {
    for (raw, expected) in [
        ("DECO*27", "DECO_27"),
        ("AC/DC", "AC_DC"),
        (
            "\u{4e0a}\u{6d77}\u{30a2}\u{30ea}\u{30b9}\u{5e7b}\u{6a02}\u{56e3}",
            "\u{4e0a}\u{6d77}\u{30a2}\u{30ea}\u{30b9}\u{5e7b}\u{6a02}\u{56e3}",
        ),
        ("trailing.", "trailing"),
    ] {
        assert_eq!(
            sanitize_folder_name(raw, UNKNOWN_ARTIST),
            expected,
            "sanitize_folder_name({raw:?})"
        );
    }
}

#[test]
fn sanitize_folder_name_replaces_rather_than_discards_a_symbol_only_name() {
    assert_eq!(sanitize_folder_name("///", UNKNOWN_ARTIST), "___");
}

#[test]
fn sanitize_folder_name_falls_back_when_nothing_survives() {
    assert_eq!(
        sanitize_folder_name("   ", UNKNOWN_ARTIST),
        "Unknown Artist"
    );
}
