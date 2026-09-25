//! `normalize` and the filename helpers built on it.

use stk::text::normalize::{
    normalize, sanitize_folder_name, strip_artist_prefix, track_number, UNKNOWN_ARTIST,
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
fn a_capital_dotted_i_normalises_to_a_form_that_stays_put() {
    // Lowercasing last left the combining dot `İ` lowercases to, so the form
    // normalised to a second form: `i\u{307}stanbul`, then `i stanbul`.
    for raw in ["\u{130}stanbul", "A\u{130}B"] {
        let once = normalize(raw);
        assert_eq!(normalize(&once), once, "normalize({raw:?})");
    }
    assert_eq!(normalize("\u{130}stanbul"), "i stanbul");
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
fn strip_artist_prefix_survives_spellings_of_differing_length() {
    // A literal-length slice would cut "Melt" short here, because the folder name
    // is far longer than its normalised form.
    assert_eq!(
        strip_artist_prefix("ryo (supercell) - Melt", "ryo (supercell)"),
        "Melt"
    );
}

#[test]
fn sanitize_folder_name_replaces_characters_a_filesystem_rejects() {
    for (raw, expected) in [
        ("DECO*27", "DECO_27"),
        ("AC/DC", "AC_DC"),
        // A backslash is the separator this function exists to neutralise.
        (r"AC\DC", "AC_DC"),
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

/// `1-05` is the fifth track of the first disc, and what a tag calls the track
/// number is the track. Taking the leading group would file every disc's opener
/// as track one.
#[test]
fn a_disc_prefixed_filename_yields_the_track_not_the_disc() {
    assert_eq!(track_number("1-05 Roki.flac"), Some(5));
    assert_eq!(track_number("2.03 - Ghost Rule.flac"), Some(3));
}

#[test]
fn a_combining_mark_is_punctuation_not_part_of_a_word() {
    // Rust's `\w` would keep it; only L, Nd, Nl and No are word characters here.
    assert_eq!(normalize("a\u{20dd}b"), "a b");
}

#[test]
fn the_separator_controls_count_as_whitespace() {
    for separator in '\u{1c}'..='\u{1f}' {
        assert_eq!(normalize(&format!("a{separator}b")), "a b", "{separator:?}");
    }
}
