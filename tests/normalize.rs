//! `normalize` and the filename helpers built on it.

use stk::text::normalize::{
    normalize, sanitize_folder_name, strip_artist_prefix, strip_track_number, track_number,
    UNKNOWN_ARTIST,
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
    // Regression: lowercasing last left `İ`'s combining dot for a second pass to strip.
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
    // A slice by the folder name's length would cut "Melt" short.
    assert_eq!(
        strip_artist_prefix("ryo (supercell) - Melt", "ryo (supercell)"),
        "Melt"
    );
}

#[test]
fn strip_artist_prefix_keeps_a_credit_the_artist_only_starts() {
    for (stem, artist) in [
        ("32ki, Hatsune Miku, 重音テト - メズマライザー", "32ki"),
        ("DECO_27, Hatsune Miku - Rabbit Hole", "DECO_27"),
        ("ZUN & Taishi - Bad Apple", "ZUN"),
    ] {
        assert_eq!(strip_artist_prefix(stem, artist), stem);
    }
    assert_eq!(
        strip_artist_prefix(
            "08. Chris Stanton - A Perfect Hero (Extended Mix)",
            "Chris Stanton"
        ),
        "A Perfect Hero (Extended Mix)"
    );
}

#[test]
fn sanitize_folder_name_replaces_characters_a_filesystem_rejects() {
    for (raw, expected) in [
        ("DECO*27", "DECO_27"),
        ("AC/DC", "AC_DC"),
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

#[test]
fn a_disc_prefixed_filename_yields_the_track_not_the_disc() {
    assert_eq!(track_number("1-05 Roki.flac"), Some(5));
    assert_eq!(track_number("2.03 - Ghost Rule.flac"), Some(3));
}

#[test]
fn a_number_glued_to_a_word_is_not_a_track_number() {
    for name in [
        "32ki, Hatsune Miku, 重音テト - メズマライザー",
        "2gether",
        "10cc - I'm Not in Love",
        "99Luftballons",
    ] {
        assert_eq!(strip_track_number(name), name);
        assert_eq!(track_number(name), None, "track_number({name:?})");
    }
}

#[test]
fn a_track_number_before_a_separator_bracket_or_space_is_stripped() {
    for (name, track) in [
        ("01 - Title", 1),
        ("01. Title", 1),
        ("(04) Title", 4),
        ("[04] Title", 4),
        ("1-05 Title", 5),
        ("08 Title", 8),
        ("01_Title", 1),
        ("01-Title", 1),
        ("03 – Title", 3),
    ] {
        assert_eq!(
            strip_track_number(name),
            "Title",
            "strip_track_number({name:?})"
        );
        assert_eq!(track_number(name), Some(track), "track_number({name:?})");
    }
}

#[test]
fn a_bare_number_is_a_track_number_and_still_a_name() {
    assert_eq!(track_number("01"), Some(1));
    assert_eq!(strip_track_number("01"), "01");
}

#[test]
fn a_combining_mark_is_punctuation_not_part_of_a_word() {
    // Rust's `\w` would keep a combining mark.
    assert_eq!(normalize("a\u{20dd}b"), "a b");
}

#[test]
fn the_separator_controls_count_as_whitespace() {
    for separator in '\u{1c}'..='\u{1f}' {
        assert_eq!(normalize(&format!("a{separator}b")), "a b", "{separator:?}");
    }
}
