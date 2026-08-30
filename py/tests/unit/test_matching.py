import random
import string

import pytest

from soulseektools.matching import (
    MatchThresholds,
    NameForms,
    blocking_keys,
    candidate_pairs,
    score_track,
    similarity,
)

THRESHOLDS = MatchThresholds()


def forms(text):
    return NameForms.of(text)


def test_name_forms_carry_the_raw_normalised_and_romanised_spellings():
    name = forms("千本桜")
    assert name.raw == "千本桜"
    assert name.normalized == "千本桜"
    assert name.romaji == "senbonzakura"


def test_empty_names_are_falsy_and_never_match():
    assert not forms("")
    assert similarity(forms(""), forms("anything")) == 0.0


def test_identical_names_score_one():
    assert similarity(forms("Roki"), forms("Roki")) == pytest.approx(1.0)


def test_kanji_matches_its_romanised_filename():
    # The case the whole romaji layer exists for: the export says 千本桜, the file
    # on disk is Senbonzakura.flac.
    assert similarity(forms("千本桜"), forms("Senbonzakura")) >= THRESHOLDS.title_strong


def test_two_kanji_spellings_sharing_a_reading_match_through_romaji():
    # Neither side is romanised, so this only works because romaji is compared
    # against romaji as well as against the normalised form.
    assert similarity(forms("初音ミク"), forms("はつねみく")) >= THRESHOLDS.title_strong


def test_containment_rescues_a_truncated_title():
    assert similarity(forms("Senbonzakura"), forms("Senbonzakura -Long Ver-")) >= 0.85


def test_reordered_words_still_match():
    assert (
        similarity(forms("Hatsune Miku Senbonzakura"), forms("Senbonzakura Hatsune Miku")) >= 0.85
    )


def test_unrelated_titles_score_low():
    assert similarity(forms("Night of Knights"), forms("Heian Alien")) < THRESHOLDS.title


class TestScoreTrack:
    def test_matching_artist_and_title_are_accepted(self):
        score = score_track(
            forms("DECO*27"), forms("Roki"), forms("DECO_27"), forms("Roki"), THRESHOLDS
        )
        assert score is not None
        assert score.artist == pytest.approx(1.0)
        assert score.title == pytest.approx(1.0)
        assert score.combined == pytest.approx(1.0)

    def test_a_strong_title_carries_a_weak_artist(self):
        score = score_track(
            forms("Mikito P"), forms("Roki"), forms("mikitoP"), forms("Roki"), THRESHOLDS
        )
        assert score is not None

    def test_a_strong_artist_never_carries_a_weak_title(self):
        assert (
            score_track(
                forms("DECO*27"),
                forms("Roki"),
                forms("DECO*27"),
                forms("Yowamushi Montblanc"),
                THRESHOLDS,
            )
            is None
        )

    def test_an_uncertain_pair_returns_none_rather_than_a_low_score(self):
        assert (
            score_track(
                forms("beatMARIO"),
                forms("Night of Knights"),
                forms("ZUN"),
                forms("Heian Alien"),
                THRESHOLDS,
            )
            is None
        )

    def test_the_title_is_weighted_above_the_artist(self):
        assert THRESHOLDS.title_weight > THRESHOLDS.artist_weight
        assert THRESHOLDS.title_weight + THRESHOLDS.artist_weight == pytest.approx(1.0)


class TestBlocking:
    def test_a_name_and_its_romanisation_share_a_key(self):
        assert blocking_keys(forms("千本桜")) & blocking_keys(forms("Senbonzakura"))

    def test_unrelated_names_share_no_key(self):
        assert not (blocking_keys(forms("Night of Knights")) & blocking_keys(forms("Heian Alien")))

    def test_short_names_still_produce_a_key(self):
        assert blocking_keys(forms("Ado")) == frozenset({"ado"})

    def test_keys_combine_every_supplied_name(self):
        combined = blocking_keys(forms("DECO*27"), forms("Roki"))
        assert combined >= blocking_keys(forms("Roki"))

    def test_candidate_pairs_yields_only_names_sharing_a_key(self):
        entries = [forms("Senbonzakura"), forms("千本桜"), forms("Heian Alien")]
        keyed = [(index, blocking_keys(name)) for index, name in enumerate(entries)]
        assert list(candidate_pairs(keyed)) == [(0, 1)]

    def test_candidate_pairs_yields_each_pair_once(self):
        entries = [forms("Night of Knights"), forms("Night of Knights (Extended)")]
        keyed = [(index, blocking_keys(name)) for index, name in enumerate(entries)]
        assert list(candidate_pairs(keyed)) == [(0, 1)]

    def test_candidate_pairs_skips_almost_every_comparison_in_a_distinct_library(self):
        rng = random.Random(20260830)
        entries = [forms("".join(rng.choices(string.ascii_lowercase, k=14))) for _ in range(200)]
        keyed = [(index, blocking_keys(name)) for index, name in enumerate(entries)]
        exhaustive = 200 * 199 // 2
        assert len(list(candidate_pairs(keyed))) < exhaustive * 0.01
