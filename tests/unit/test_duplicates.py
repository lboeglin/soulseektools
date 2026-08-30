import random
import string
from pathlib import Path

import numpy as np
import pytest

from soulseektools import duplicates
from soulseektools.audio.fingerprint import Fingerprint, FingerprintKind
from soulseektools.matching import NameForms
from soulseektools.models import LibraryEntry, TrackTags


def entry(path, artist, title, *, size=1_000_000, duration=200.0, mtime=1.0) -> LibraryEntry:
    return LibraryEntry(
        path=Path(path),
        size=size,
        mtime=mtime,
        folder_artist=artist,
        tags=TrackTags(artist=artist, title=title),
        artist=NameForms.of(artist),
        title=NameForms.of(title),
        duration_seconds=duration,
    )


class TestFindByTitle:
    def test_the_same_song_under_two_artist_spellings_is_found(self):
        # The pairing the whole romaji layer exists for: one producer, two
        # folders, no shared characters between the spellings.
        entries = [
            entry("Inabakumori/Lagtrain.flac", "Inabakumori", "Ragutorein"),
            entry("稲葉曇/ラグトレイン.flac", "稲葉曇", "ラグトレイン"),
        ]

        pairs = duplicates.find_by_title(entries)

        assert len(pairs) == 1
        assert pairs[0].score > 0.8

    def test_unrelated_tracks_are_not_paired(self):
        entries = [
            entry("ZUN/Heian Alien.flac", "ZUN", "Heian Alien"),
            entry("beatMARIO/Night of Knights.flac", "beatMARIO", "Night of Knights"),
        ]

        assert duplicates.find_by_title(entries) == []

    def test_the_same_title_by_a_clearly_different_artist_is_not_paired(self):
        entries = [
            entry("YOASOBI/Idol.flac", "YOASOBI", "Idol"),
            entry("beatMARIO/Idol.flac", "beatMARIO", "Idol"),
        ]

        assert duplicates.find_by_title(entries) == []

    def test_a_strong_title_match_pairs_similar_artist_names(self):
        # The rule that lets a kanji artist match its romanisation also pairs two
        # merely similar names when the title is identical. Deliberate: the
        # command reports candidates for review and deletes nothing.
        entries = [
            entry("Ado/Odo.flac", "Ado", "Odo"),
            entry("Syudou/Odo.flac", "Syudou", "Odo"),
        ]

        pairs = duplicates.find_by_title(entries)

        assert len(pairs) == 1
        assert pairs[0].score < 1.0

    def test_a_file_is_never_paired_with_itself(self):
        single = entry("Ado/Odo.flac", "Ado", "Odo")

        assert duplicates.find_by_title([single, single]) == []

    def test_pairs_are_ordered_by_confidence(self):
        entries = [
            entry("A/Senbonzakura.flac", "Hatsune Miku", "Senbonzakura"),
            entry("B/Senbonzakura.flac", "Hatsune Miku", "Senbonzakura"),
            entry("C/Senbonzakura Long.flac", "Hatsune Miku", "Senbonzakura -Long Ver-"),
        ]

        scores = [pair.score for pair in duplicates.find_by_title(entries)]

        assert scores == sorted(scores, reverse=True)

    def test_the_larger_copy_is_identified_as_the_one_to_keep(self):
        entries = [
            entry("A/Odo.mp3", "Ado", "Odo", size=5_000_000),
            entry("B/Odo.flac", "Ado", "Odo", size=40_000_000),
        ]

        pair = duplicates.find_by_title(entries)[0]

        assert pair.larger.path.suffix == ".flac"
        assert pair.smaller.path.suffix == ".mp3"

    def test_blocking_does_not_lose_a_match_hidden_in_a_larger_library(self):
        rng = random.Random(20260830)

        def distinct():
            return "".join(rng.choices(string.ascii_lowercase, k=12))

        entries = [
            entry(f"{a}/{t}.flac", a, t) for a, t in ((distinct(), distinct()) for _ in range(120))
        ]
        duplicated = entries[42]
        entries.append(entry("Elsewhere/copy.flac", duplicated.artist.raw, duplicated.title.raw))

        pairs = duplicates.find_by_title(entries)

        assert len(pairs) == 1
        found = pairs[0]
        assert {found.left.path, found.right.path} == {
            duplicated.path,
            Path("Elsewhere/copy.flac"),
        }


class TestFindByAudio:
    @pytest.fixture
    def fingerprints(self, monkeypatch):
        """Fingerprints keyed by filename, so no decoding happens in these tests."""
        table: dict[str, Fingerprint] = {}

        def fake(path):
            return table[Path(path).name]

        monkeypatch.setattr(duplicates.fingerprints, "fingerprint", fake)
        return table

    @staticmethod
    def curve(seed: int, length: int = 40) -> Fingerprint:
        """A loudness curve with a shape unique to ``seed``.

        Random rather than a phase-shifted sine: a shifted sine is the same
        signal, and the alignment search would rightly say so.
        """
        generator = np.random.default_rng(seed)
        values = 10.0 * generator.standard_normal(length)
        return Fingerprint(kind=FingerprintKind.ENVELOPE, values=tuple(values.tolist()))

    def test_identical_audio_is_paired_whatever_the_names_say(self, fingerprints):
        fingerprints["a.flac"] = fingerprints["b.flac"] = self.curve(0)
        entries = [
            entry("X/a.flac", "Unknown", "track01"),
            entry("Y/b.flac", "Some Other Artist", "18"),
        ]

        pairs = duplicates.find_by_audio(entries)

        assert len(pairs) == 1
        assert pairs[0].score == pytest.approx(1.0)

    def test_different_audio_of_the_same_length_is_not_paired(self, fingerprints):
        # Exactly the coincidence that made a duration-and-size fingerprint
        # unreliable: same length, same size, different recordings.
        fingerprints["a.flac"] = self.curve(0)
        fingerprints["b.flac"] = self.curve(3)
        entries = [
            entry("X/a.flac", "One", "Track", duration=200.0, size=1_000_000),
            entry("Y/b.flac", "Two", "Other", duration=200.0, size=1_000_000),
        ]

        assert duplicates.find_by_audio(entries) == []

    def test_tracks_of_very_different_length_are_never_fingerprinted(self, fingerprints):
        entries = [
            entry("X/a.flac", "One", "Track", duration=100.0),
            entry("Y/b.flac", "Two", "Other", duration=400.0),
        ]

        assert duplicates.find_by_audio(entries) == []

    def test_the_duration_tolerance_is_configurable(self, fingerprints):
        fingerprints["a.flac"] = fingerprints["b.flac"] = self.curve(0)
        entries = [
            entry("X/a.flac", "One", "Track", duration=200.0),
            entry("Y/b.flac", "Two", "Other", duration=205.0),
        ]

        assert duplicates.find_by_audio(entries) == []
        assert duplicates.find_by_audio(entries, duration_tolerance_seconds=10.0)

    def test_files_without_a_known_duration_are_skipped(self, fingerprints):
        entries = [
            entry("X/a.flac", "One", "Track", duration=None),
            entry("Y/b.flac", "Two", "Other", duration=None),
        ]

        assert duplicates.find_by_audio(entries) == []

    def test_the_threshold_is_configurable(self, fingerprints):
        fingerprints["a.flac"] = self.curve(0)
        fingerprints["b.flac"] = self.curve(3)
        entries = [entry("X/a.flac", "One", "Track"), entry("Y/b.flac", "Two", "Other")]

        assert duplicates.find_by_audio(entries, threshold=0.1)

    def test_an_undecodable_file_is_skipped_rather_than_aborting_the_run(
        self, fingerprints, monkeypatch
    ):
        from soulseektools.errors import ExternalToolError

        def explode(path):
            if Path(path).name == "broken.flac":
                raise ExternalToolError("cannot decode")
            return self.curve(0)

        monkeypatch.setattr(duplicates.fingerprints, "fingerprint", explode)
        entries = [
            entry("X/a.flac", "One", "Track"),
            entry("Y/broken.flac", "Two", "Other"),
            entry("Z/c.flac", "Three", "Third"),
        ]

        pairs = duplicates.find_by_audio(entries)

        assert {pair.left.path.name for pair in pairs} == {"a.flac"}

    def test_fingerprints_are_cached_in_the_index(self, fingerprints, tmp_path, library_root):
        from soulseektools.library.index import LibraryIndex

        calls = []

        def counted(path):
            calls.append(path)
            return self.curve(0)

        entries = [entry("X/a.flac", "One", "Track"), entry("Y/b.flac", "Two", "Other")]
        with LibraryIndex(tmp_path / "index.db") as index:
            import unittest.mock

            with unittest.mock.patch.object(duplicates.fingerprints, "fingerprint", counted):
                duplicates.find_by_audio(entries, index=index)
                first_pass = len(calls)
                duplicates.find_by_audio(entries, index=index)

        assert first_pass == 2
        assert len(calls) == first_pass
