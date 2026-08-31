import pytest

from soulseektools.text.normalize import (
    normalize,
    sanitize_folder_name,
    strip_artist_prefix,
    strip_track_number,
)


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("Senbonzakura", "senbonzakura"),
        ("DECO*27", "deco 27"),
        ("Wonderful\u2605opportunity!", "wonderful opportunity"),
        ("LUVORATORRRRRY!", "luvoratorrrrry"),
        ("  spaced   out  ", "spaced out"),
        (None, ""),
        ("", ""),
    ],
)
def test_normalize_folds_case_punctuation_and_whitespace(raw, expected):
    assert normalize(raw) == expected


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("Mesmerizer (feat. \u521d\u97f3\u30df\u30af&\u91cd\u97f3\u30c6\u30c8)", "mesmerizer"),
        ("Roki feat. Hatsune Miku", "roki"),
        ("Yowamushi Montblanc (sasakure.UK Remix)", "yowamushi montblanc"),
        ("Shiny Happy Days [16-44kHz]", "shiny happy days"),
    ],
)
def test_normalize_drops_featured_credits_and_bracketed_suffixes(raw, expected):
    assert normalize(raw) == expected


def test_normalize_applies_nfkc_so_fullwidth_matches_halfwidth():
    assert normalize("\uff2d\uff29\uff2b\uff35") == normalize("MIKU")


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("01 - Roki", "Roki"),
        (
            "15. \u30ca\u30a4\u30c8\u30fb\u30aa\u30d6\u30fb\u30ca\u30a4\u30c4",
            "\u30ca\u30a4\u30c8\u30fb\u30aa\u30d6\u30fb\u30ca\u30a4\u30c4",
        ),
        ("(04) Reisen Occultia", "Reisen Occultia"),
        ("4 - LUVORATORRRRRY!", "LUVORATORRRRRY!"),
        ("29 Rebirth the edge", "Rebirth the edge"),
        ("Roki", "Roki"),
    ],
)
def test_strip_track_number(raw, expected):
    assert strip_track_number(raw) == expected


def test_strip_track_number_keeps_a_title_that_is_only_a_number():
    assert strip_track_number("39") == "39"


@pytest.mark.parametrize(
    ("stem", "artist", "expected"),
    [
        ("DECO*27 - Roki", "DECO_27", "Roki"),
        ("06 DECO-27 - Yowamushi Montblanc", "DECO*27", "Yowamushi Montblanc"),
        ("beatMARIO - Night of Knights", "beatMARIO", "Night of Knights"),
        (
            "\u7a32\u8449\u66c7 - \u30e9\u30b0\u30c8\u30ec\u30a4\u30f3",
            "\u7a32\u8449\u66c7",
            "\u30e9\u30b0\u30c8\u30ec\u30a4\u30f3",
        ),
        ("Just A Title", "Some Artist", "Just A Title"),
    ],
)
def test_strip_artist_prefix(stem, artist, expected):
    assert strip_artist_prefix(stem, artist) == expected


def test_strip_artist_prefix_survives_spellings_of_differing_length():
    # A literal-length slice would cut "Melt" short here, because the folder name
    # is far longer than its normalised form.
    assert strip_artist_prefix("ryo (supercell) - Melt", "ryo (supercell)") == "Melt"


def test_strip_artist_prefix_keeps_the_name_when_it_is_the_whole_stem():
    assert strip_artist_prefix("ZUN", "ZUN") == "ZUN"


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("DECO*27", "DECO_27"),
        ("AC/DC", "AC_DC"),
        # A backslash is the separator this function exists to neutralise; see #5.
        ("AC\\DC", "AC_DC"),
        (
            "\u4e0a\u6d77\u30a2\u30ea\u30b9\u5e7b\u6a02\u56e3",
            "\u4e0a\u6d77\u30a2\u30ea\u30b9\u5e7b\u6a02\u56e3",
        ),
        ("trailing.", "trailing"),
    ],
)
def test_sanitize_folder_name(raw, expected):
    assert sanitize_folder_name(raw) == expected


def test_sanitize_folder_name_replaces_rather_than_discards_a_symbol_only_name():
    assert sanitize_folder_name("///") == "___"


def test_sanitize_folder_name_falls_back_when_nothing_survives():
    assert sanitize_folder_name("   ") == "Unknown Artist"
