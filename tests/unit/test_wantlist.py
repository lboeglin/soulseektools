from pathlib import Path

import pytest

from soulseektools.errors import SoulseekToolsError
from soulseektools.matching import NameForms
from soulseektools.models import LibraryEntry, TrackTags
from soulseektools.wantlist import diff, read_export


def entry(artist, title) -> LibraryEntry:
    return LibraryEntry(
        path=Path(artist) / f"{title}.flac",
        size=1,
        mtime=1.0,
        folder_artist=artist,
        tags=TrackTags(artist=artist, title=title),
        artist=NameForms.of(artist),
        title=NameForms.of(title),
    )


def write_csv(tmp_path, body) -> Path:
    path = tmp_path / "export.csv"
    path.write_text(body, encoding="utf-8-sig")
    return path


class TestReadExport:
    def test_reads_the_spotify_column_names(self, tmp_path):
        path = write_csv(
            tmp_path,
            "Track name,Artist name,Album name,Duration (ms)\nRoki,DECO*27,Ghost,215000\n",
        )

        tracks = read_export(path)

        assert len(tracks) == 1
        assert tracks[0].artist == "DECO*27"
        assert tracks[0].title == "Roki"
        assert tracks[0].album == "Ghost"
        assert tracks[0].duration_seconds == pytest.approx(215.0)

    def test_reads_alternative_column_names(self, tmp_path):
        path = write_csv(tmp_path, "Title,Artist\nRoki,DECO*27\n")

        assert read_export(path)[0].title == "Roki"

    def test_reads_a_duration_written_as_minutes_and_seconds(self, tmp_path):
        path = write_csv(tmp_path, "Title,Artist,Length\nRoki,DECO*27,3:35\n")

        assert read_export(path)[0].duration_seconds == pytest.approx(215.0)

    def test_an_unreadable_duration_is_left_unset(self, tmp_path):
        path = write_csv(tmp_path, "Title,Artist,Length\nRoki,DECO*27,soon\n")

        assert read_export(path)[0].duration_seconds is None

    def test_the_original_row_is_kept(self, tmp_path):
        path = write_csv(tmp_path, "Track name,Artist name,Added at\nRoki,DECO*27,2026-01-01\n")

        assert read_export(path)[0].source_row["Added at"] == "2026-01-01"

    def test_blank_rows_are_skipped(self, tmp_path):
        path = write_csv(tmp_path, "Track name,Artist name\nRoki,DECO*27\n,\n")

        assert len(read_export(path)) == 1

    def test_a_file_without_the_needed_columns_is_rejected(self, tmp_path):
        path = write_csv(tmp_path, "Foo,Bar\n1,2\n")

        with pytest.raises(SoulseekToolsError, match="artist and a title"):
            read_export(path)

    def test_an_empty_file_is_rejected(self, tmp_path):
        with pytest.raises(SoulseekToolsError, match="empty"):
            read_export(write_csv(tmp_path, ""))

    def test_the_query_used_for_searching_joins_artist_and_title(self, tmp_path):
        path = write_csv(tmp_path, "Track name,Artist name\nRoki,DECO*27\n")

        assert read_export(path)[0].query == "DECO*27 - Roki"


class TestDiff:
    def test_an_owned_track_is_not_missing(self):
        tracks = read_export_like("DECO*27", "Roki")
        library = [entry("DECO*27", "Roki")]

        result = diff(tracks, library)

        assert result.missing == ()
        assert len(result.owned) == 1
        assert result.owned[0].score > 0.9

    def test_a_track_absent_from_the_library_is_missing(self):
        result = diff(read_export_like("ZUN", "Heian Alien"), [entry("DECO*27", "Roki")])

        assert len(result.missing) == 1

    def test_a_kanji_title_matches_its_romanised_file(self):
        # Without romaji these share not a single character.
        result = diff(
            read_export_like("Hatsune Miku", "千本桜"), [entry("Hatsune Miku", "Senbonzakura")]
        )

        assert result.missing == ()

    def test_an_uncertain_match_stays_in_the_wantlist(self):
        # Deliberate: a duplicate download costs megabytes, a dropped track is lost.
        result = diff(
            read_export_like("DECO*27", "Roki"),
            [entry("Some Other Artist", "A Different Song Entirely")],
        )

        assert len(result.missing) == 1

    def test_the_best_of_several_matches_is_reported(self):
        library = [
            entry("Hatsune Miku", "Senbonzakura -Long Ver-"),
            entry("Hatsune Miku", "Senbonzakura"),
        ]

        result = diff(read_export_like("Hatsune Miku", "Senbonzakura"), library)

        assert result.owned[0].entry.title.raw == "Senbonzakura"

    def test_every_track_is_accounted_for(self):
        tracks = read_export_like("DECO*27", "Roki") + read_export_like("ZUN", "Heian Alien")

        result = diff(tracks, [entry("DECO*27", "Roki")])

        assert result.total == 2

    def test_an_empty_library_leaves_everything_missing(self):
        result = diff(read_export_like("DECO*27", "Roki"), [])

        assert len(result.missing) == 1

    def test_blocking_finds_a_match_inside_a_large_library(self):
        library = [entry(f"Artist {i}", f"Some Song {i}") for i in range(300)]
        library.append(entry("DECO*27", "Roki"))

        result = diff(read_export_like("DECO_27", "Roki"), library)

        assert result.missing == ()


def read_export_like(artist, title):
    from soulseektools.models import Track

    return [Track(artist=artist, title=title)]
