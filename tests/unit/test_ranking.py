"""Ranking tests, built around the wrong downloads the original scorer allowed.

Each case in `TestTheFolderDecides` is a shape the old filename-only scorer could
not distinguish: two results whose filenames are identical, differing only in the
remote folder they sit in.
"""

import pytest

from soulseektools.config import RankingSettings
from soulseektools.models import Candidate, Track
from soulseektools.soulseek.ranking import rank

TRACK = Track(artist="DECO*27", title="Roki", duration_seconds=215.0)


def candidate(path, **overrides) -> Candidate:
    defaults = {
        "username": "peer",
        "remote_path": path,
        "size": 30_000_000,
        "bit_depth": 16,
        "sample_rate": 44100,
        "free_upload_slots": True,
        "queue_length": 0,
    }
    return Candidate(**{**defaults, **overrides})


def studio(name="03 - Roki.flac") -> Candidate:
    return candidate(f"@@peer\\Music\\DECO27 - Ghost\\{name}")


class TestTheFolderDecides:
    @pytest.mark.parametrize(
        "folder",
        [
            "Symphony Concert 2019",
            "Tokyo Orchestra Live",
            "DECO27 Live at Budokan",
            "Vocaloid Medley Collection",
        ],
    )
    def test_a_suspicious_folder_ranks_below_an_identical_studio_file(self, folder):
        # Identical filenames. The old scorer saw no difference between these.
        suspicious = candidate(f"@@peer\\Music\\{folder}\\03 - Roki.flac")

        ranked = rank(TRACK, [suspicious, studio()])

        assert ranked[0].candidate is not suspicious
        assert ranked[-1].candidate is suspicious
        assert ranked[-1].flags

    def test_the_flag_names_what_was_found_in_the_path(self):
        ranked = rank(TRACK, [candidate("@@peer\\Symphony Concert\\03 - Roki.flac")])

        assert "symphony" in ranked[0].flags

    def test_a_rejected_term_in_the_folder_drops_a_result_to_the_bottom(self):
        karaoke = candidate("@@peer\\Karaoke Versions\\03 - Roki.flac")

        ranked = rank(TRACK, [karaoke, studio()])

        assert ranked[-1].candidate is karaoke
        assert ranked[-1].rejected
        assert ranked[-1].flags == ("reject:karaoke",)

    def test_a_term_in_the_filename_is_caught_too(self):
        ranked = rank(TRACK, [candidate("@@peer\\Music\\03 - Roki (Live).flac")])

        assert "live" in ranked[0].flags

    def test_several_warnings_compound(self):
        one = candidate("@@peer\\Live Symphony\\03 - Roki.flac")
        few = candidate("@@peer\\Live\\03 - Roki.flac")

        ranked = rank(TRACK, [one, few])

        assert ranked[0].candidate is few
        assert len(ranked[1].flags) > len(ranked[0].flags)

    def test_a_term_the_track_itself_contains_is_not_held_against_it(self):
        # Someone wanting "Live and Learn" is not asking to reject every
        # result whose path says "live".
        wanted = Track(artist="Crush 40", title="Live and Learn")
        result = candidate("@@peer\\Sonic Adventure 2\\01 Live and Learn.flac")

        ranked = rank(wanted, [result])

        assert ranked[0].flags == ()


class TestQuality:
    def test_lossless_outranks_lossy_whatever_the_bitrate(self):
        lossy = candidate("@@peer\\Music\\03 - Roki.mp3", bit_depth=None, bitrate=320)

        ranked = rank(TRACK, [lossy, studio()])

        assert ranked[0].candidate.extension == "flac"

    def test_a_higher_bitrate_wins_between_lossy_results(self):
        low = candidate("@@a\\Music\\03 - Roki.mp3", bit_depth=None, bitrate=128)
        high = candidate("@@b\\Music\\03 - Roki.mp3", bit_depth=None, bitrate=320)

        assert rank(TRACK, [low, high])[0].candidate is high

    def test_hi_res_outranks_cd_quality(self):
        cd = studio()
        hires = candidate("@@b\\Music\\03 - Roki.flac", sample_rate=96000, bit_depth=24)

        assert rank(TRACK, [cd, hires])[0].candidate is hires

    def test_a_free_slot_breaks_a_tie(self):
        busy = candidate("@@a\\Music\\03 - Roki.flac", free_upload_slots=False)
        free = candidate("@@b\\Music\\03 - Roki.flac", free_upload_slots=True)

        assert rank(TRACK, [busy, free])[0].candidate is free

    def test_a_long_queue_costs_a_result(self):
        short = candidate("@@a\\Music\\03 - Roki.flac", queue_length=0)
        long = candidate("@@b\\Music\\03 - Roki.flac", queue_length=400)

        assert rank(TRACK, [long, short])[0].candidate is short

    def test_a_result_with_no_quality_information_still_ranks(self):
        unknown = candidate("@@a\\Music\\03 - Roki.mp3", bit_depth=None, sample_rate=None)

        assert rank(TRACK, [unknown])


class TestFiltering:
    def test_non_audio_files_are_dropped(self):
        assert rank(TRACK, [candidate("@@peer\\Music\\Roki.cue")]) == []
        assert rank(TRACK, [candidate("@@peer\\Music\\cover.jpg")]) == []

    def test_a_file_named_after_another_song_is_dropped(self):
        assert rank(TRACK, [candidate("@@peer\\Music\\Night of Knights.flac")]) == []

    def test_a_file_named_after_the_artist_alone_is_kept(self):
        # Peers file untitled rips under the artist's name; a human can still judge it.
        assert rank(TRACK, [candidate("@@peer\\Music\\DECO 27.flac")])

    def test_punctuation_differences_do_not_lose_a_match(self):
        assert rank(TRACK, [candidate("@@peer\\Music\\DECO_27 - Roki.flac")])

    def test_a_track_number_prefix_does_not_lose_a_match(self):
        assert rank(TRACK, [candidate("@@peer\\Music\\03. Roki.flac")])

    def test_the_minimum_ratio_is_configurable(self):
        loose = candidate("@@peer\\Music\\Rokii.flac")

        assert rank(TRACK, [loose], RankingSettings(min_title_ratio=0.99)) == []
        assert rank(TRACK, [loose], RankingSettings(min_title_ratio=0.3))


class TestDuration:
    def test_a_result_of_the_wrong_length_is_penalised(self):
        right = candidate("@@a\\Music\\03 - Roki.flac", duration_seconds=215.0)
        wrong = candidate("@@b\\Music\\03 - Roki.flac", duration_seconds=480.0)

        ranked = rank(TRACK, [wrong, right])

        assert ranked[0].candidate is right
        assert "duration" in ranked[1].flags

    def test_a_small_difference_is_tolerated(self):
        close = candidate("@@a\\Music\\03 - Roki.flac", duration_seconds=218.0)

        assert rank(TRACK, [close])[0].flags == ()

    def test_nothing_is_penalised_when_a_length_is_unknown(self):
        unknown = candidate("@@a\\Music\\03 - Roki.flac", duration_seconds=None)
        wanted = Track(artist="DECO*27", title="Roki")

        assert rank(wanted, [unknown])[0].flags == ()
        assert rank(TRACK, [unknown])[0].flags == ()


class TestOrdering:
    def test_results_come_back_best_first(self):
        results = rank(
            TRACK,
            [
                candidate("@@a\\Live\\03 - Roki.flac"),
                candidate("@@b\\Music\\03 - Roki.mp3", bit_depth=None, bitrate=128),
                studio(),
            ],
        )

        assert [result.score for result in results] == sorted(
            (result.score for result in results), reverse=True
        )

    def test_the_number_of_results_can_be_capped(self):
        many = [candidate(f"@@peer{i}\\Music\\03 - Roki.flac") for i in range(10)]

        assert len(rank(TRACK, many, limit=3)) == 3

    def test_no_candidates_yields_no_results(self):
        assert rank(TRACK, []) == []
