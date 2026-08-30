from pathlib import Path

import pytest

from soulseektools.library.index import LibraryIndex, describe
from soulseektools.library.scan import folder_artist, iter_audio_files


class TestScan:
    def test_walks_nested_album_folders(self, library_root):
        found = list(iter_audio_files(library_root))

        assert library_root / "ZUN" / "Nested" / "15 Heian Alien.flac" in found
        assert len(found) == 4

    def test_honours_the_extension_filter(self, library_root):
        found = list(iter_audio_files(library_root, {".flac"}))

        assert {path.suffix for path in found} == {".flac"}

    def test_ignores_non_audio_files(self, library_root):
        (library_root / "DECO_27" / "cover.jpg").write_bytes(b"not audio")

        assert all(path.suffix != ".jpg" for path in iter_audio_files(library_root))

    def test_folder_artist_is_the_first_component_below_the_root(self, library_root):
        nested = library_root / "ZUN" / "Nested" / "15 Heian Alien.flac"

        assert folder_artist(nested, library_root) == "ZUN"

    def test_a_file_in_the_root_has_no_implied_artist(self, library_root):
        assert folder_artist(library_root / "loose.flac", library_root) == ""

    def test_a_path_outside_the_root_has_no_implied_artist(self, library_root):
        assert folder_artist(Path("/elsewhere/track.flac"), library_root) == ""


class TestDescribe:
    def test_prefers_tags_over_the_folder_layout(self, library_root):
        entry = describe(library_root / "DECO_27" / "01 - Roki.flac", library_root)

        assert entry.tags.artist == "DECO*27"
        assert entry.artist.raw == "DECO*27"
        assert entry.title.raw == "Roki"
        assert entry.folder_artist == "DECO_27"

    def test_falls_back_to_the_folder_and_filename_when_untagged(self, library_root):
        entry = describe(library_root / "beatMARIO" / "Night of Knights.m4a", library_root)

        assert entry.artist.raw == "beatMARIO"
        assert entry.title.raw == "Night of Knights"

    def test_strips_the_track_number_from_a_filename_derived_title(self, library_root):
        entry = describe(library_root / "ZUN" / "Nested" / "15 Heian Alien.flac", library_root)

        assert entry.title.raw == "Heian Alien"

    def test_records_size_duration_and_artwork(self, library_root):
        entry = describe(library_root / "DECO_27" / "01 - Roki.flac", library_root)

        assert entry.size > 0
        assert entry.duration_seconds == pytest.approx(1.0, abs=0.15)
        assert entry.artwork_bytes == 0
        assert entry.has_artwork is False

    def test_precomputes_the_name_forms_used_for_matching(self, library_root):
        entry = describe(library_root / "DECO_27" / "01 - Roki.flac", library_root)

        assert entry.artist.normalized == "deco 27"
        assert entry.title.romaji == "roki"


class TestLibraryIndex:
    def test_refresh_indexes_every_file(self, tmp_path, library_root):
        with LibraryIndex(tmp_path / "index.db") as index:
            result = index.refresh(library_root)

            assert result.added == 4
            assert result.unchanged == 0
            assert index.count() == 4

    def test_a_second_refresh_rereads_nothing(self, tmp_path, library_root):
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)

            result = index.refresh(library_root)

            assert result.unchanged == 4
            assert result.added == 0
            assert result.updated == 0

    def test_full_refresh_rereads_everything(self, tmp_path, library_root):
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)

            result = index.refresh(library_root, full=True)

            assert result.updated == 4
            assert result.unchanged == 0

    def test_a_changed_file_is_reread(self, tmp_path, library_root, encode_audio):
        track = library_root / "DECO_27" / "01 - Roki.flac"
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)
            encode_audio(track, seconds=2.0, metadata={"artist": "DECO*27", "title": "Roki"})

            result = index.refresh(library_root)

            assert result.updated == 1
            assert result.unchanged == 3
            entry = index.entry(track)
            assert entry is not None
            assert entry.duration_seconds == pytest.approx(2.0, abs=0.15)

    def test_a_deleted_file_is_dropped(self, tmp_path, library_root):
        track = library_root / "beatMARIO" / "Night of Knights.m4a"
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)
            track.unlink()

            result = index.refresh(library_root)

            assert result.removed == 1
            assert index.count() == 3
            assert index.entry(track) is None

    def test_entries_round_trip_through_sqlite(self, tmp_path, library_root):
        track = library_root / "DECO_27" / "01 - Roki.flac"
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)

            stored = index.entry(track)

            assert stored == describe(track, library_root)

    def test_entries_are_returned_in_a_deterministic_order(self, tmp_path, library_root):
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)

            paths = [str(entry.path) for entry in index.entries()]

            assert paths == sorted(paths)

    def test_the_index_survives_being_reopened(self, tmp_path, library_root):
        database = tmp_path / "index.db"
        with LibraryIndex(database) as index:
            index.refresh(library_root)

        with LibraryIndex(database) as reopened:
            assert reopened.count() == 4
            assert reopened.refresh(library_root).unchanged == 4

    def test_artist_counts_group_by_folder(self, tmp_path, library_root):
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)

            assert index.artist_counts() == {"DECO_27": 2, "ZUN": 1, "beatMARIO": 1}

    def test_refresh_reports_progress_per_file(self, tmp_path, library_root):
        seen: list[Path] = []
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root, on_file=seen.append)

        assert len(seen) == 4

    def test_creates_the_database_directory(self, tmp_path, library_root):
        with LibraryIndex(tmp_path / "nested" / "deeper" / "index.db") as index:
            assert index.refresh(library_root).added == 4

    def test_reindex_picks_up_a_tag_this_tool_just_wrote(self, tmp_path, library_root):
        # Without this, a command that edits tags leaves the next command
        # reporting what was true before the edit.
        from soulseektools.library import tags

        track = library_root / "ZUN" / "Nested" / "15 Heian Alien.flac"
        with LibraryIndex(tmp_path / "index.db") as index:
            index.refresh(library_root)
            assert index.entry(track).tags.genres == ()

            tags.write_genres(track, ["Touhou"])
            index.reindex(track, library_root)

            assert index.entry(track).tags.genres == ("Touhou",)

    def test_reindexing_a_file_that_is_gone_does_nothing(self, tmp_path, library_root):
        with LibraryIndex(tmp_path / "index.db") as index:
            assert index.reindex(library_root / "absent.flac", library_root) is None
