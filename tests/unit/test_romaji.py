import pytest

from soulseektools.matching import MatchThresholds, NameForms, score_track, similarity
from soulseektools.text.romaji import contains_japanese, to_romaji


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("千本桜", True),
        ("ラグトレイン", True),
        ("ひらがな", True),
        ("Senbonzakura", False),
        ("DECO*27", False),
        ("", False),
    ],
)
def test_contains_japanese(text, expected):
    assert contains_japanese(text) is expected


@pytest.mark.parametrize(
    ("japanese", "romanised"),
    [
        ("千本桜", "senbonzakura"),
        ("ラグトレイン", "ragutorein"),
        ("初音ミク", "hatsunemiku"),
        ("飴と鎖", "ametokusari"),
    ],
)
def test_to_romaji_matches_the_romanised_spelling_used_in_filenames(japanese, romanised):
    assert to_romaji(japanese).replace(" ", "") == romanised


def test_to_romaji_normalises_text_without_japanese_characters():
    assert to_romaji("DECO*27") == "deco 27"


def test_to_romaji_is_idempotent():
    once = to_romaji("千本桜")
    assert to_romaji(once) == once


def test_artist_readings_can_diverge_but_a_strong_title_still_matches():
    # 稲葉曇 romanises to the on-yomi "inabadon", while the producer romanises the
    # same name as "Inabakumori". Transliteration cannot know that, which is why
    # the artist half of a match is only ever required to clear `artist_weak`.
    kanji, romaji_alias = NameForms.of("稲葉曇"), NameForms.of("Inabakumori")
    thresholds = MatchThresholds()

    assert similarity(kanji, romaji_alias) < 1.0
    assert similarity(kanji, romaji_alias) >= thresholds.artist_weak

    title = NameForms.of("ラグトレイン")
    matched = score_track(kanji, title, romaji_alias, NameForms.of("Ragutorein"), thresholds)
    assert matched is not None
    assert matched.title >= thresholds.title_strong
