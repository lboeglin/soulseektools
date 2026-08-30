from pathlib import Path

import pytest

from soulseektools.errors import ConfigError
from soulseektools.genres import (
    GenreMap,
    load_genre_map,
    plan_changes,
    unmapped_artists,
)
from soulseektools.matching import NameForms
from soulseektools.models import LibraryEntry, TrackTags

EXAMPLE_MAP = Path(__file__).parents[2] / "data" / "genres.example.toml"


def entry(folder_artist, title, genres=()) -> LibraryEntry:
    return LibraryEntry(
        path=Path(folder_artist) / f"{title}.flac",
        size=1,
        mtime=1.0,
        folder_artist=folder_artist,
        tags=TrackTags(artist=folder_artist, title=title, genres=tuple(genres)),
        artist=NameForms.of(folder_artist),
        title=NameForms.of(title),
    )


def write_map(tmp_path, body) -> Path:
    path = tmp_path / "genres.toml"
    path.write_text(body, encoding="utf-8")
    return path


class TestLoadGenreMap:
    def test_reads_a_flat_table(self, tmp_path):
        path = write_map(tmp_path, 'ZUN = "Touhou"\nAdo = "J-Pop"\n')

        genre_map = load_genre_map(path)

        assert genre_map.genres_for("ZUN") == ("Touhou",)
        assert len(genre_map) == 2

    def test_reads_a_table_nested_under_artists(self, tmp_path):
        path = write_map(tmp_path, '[artists]\nZUN = "Touhou"\n')

        assert load_genre_map(path).genres_for("ZUN") == ("Touhou",)

    def test_an_artist_may_have_several_genres(self, tmp_path):
        path = write_map(tmp_path, '"SOUND HOLIC" = ["Touhou", "Eurobeat"]\n')

        assert load_genre_map(path).genres_for("SOUND HOLIC") == ("Touhou", "Eurobeat")

    def test_artists_match_on_their_normalised_name(self, tmp_path):
        # A "DECO*27" entry has to cover the "DECO_27" folder it is filed under.
        path = write_map(tmp_path, '"DECO*27" = "Vocaloid"\n')

        genre_map = load_genre_map(path)

        assert genre_map.genres_for("DECO_27") == ("Vocaloid",)
        assert genre_map.genres_for("deco 27") == ("Vocaloid",)

    def test_an_unmapped_artist_yields_nothing(self, tmp_path):
        genre_map = load_genre_map(write_map(tmp_path, 'ZUN = "Touhou"\n'))

        assert genre_map.genres_for("Nobody") == ()
        assert not genre_map.covers("Nobody")

    def test_a_non_string_value_is_rejected(self, tmp_path):
        path = write_map(tmp_path, "ZUN = 42\n")

        with pytest.raises(ConfigError, match="must be a genre"):
            load_genre_map(path)

    def test_a_missing_file_is_reported_as_a_config_error(self, tmp_path):
        with pytest.raises(ConfigError, match="cannot read"):
            load_genre_map(tmp_path / "absent.toml")


class TestShippedExample:
    def test_the_example_map_loads(self):
        genre_map = load_genre_map(EXAMPLE_MAP)

        assert len(genre_map) > 100

    def test_it_preserves_the_entries_it_was_converted_from(self):
        genre_map = load_genre_map(EXAMPLE_MAP)

        assert genre_map.genres_for("DECO_27") == ("Vocaloid",)
        assert genre_map.genres_for("ZUN") == ("Touhou",)
        assert genre_map.genres_for("SOUND HOLIC") == ("Touhou", "Eurobeat")
        assert genre_map.genres_for("稲葉曇") == ("Vocaloid",)


class TestPlanChanges:
    def test_only_mapped_artists_are_planned(self):
        genre_map = GenreMap({"zun": ("Touhou",)})
        entries = [entry("ZUN", "Heian Alien"), entry("Nobody", "Unknown")]

        planned = plan_changes(entries, genre_map)

        assert len(planned) == 1
        assert planned[0].proposed == ("Touhou",)

    def test_a_file_already_carrying_its_genre_is_not_a_change(self):
        genre_map = GenreMap({"zun": ("Touhou",)})

        planned = plan_changes([entry("ZUN", "Heian Alien", ["Touhou"])], genre_map)

        assert planned[0].is_change is False

    def test_a_differing_genre_is_a_change(self):
        genre_map = GenreMap({"zun": ("Touhou",)})

        planned = plan_changes([entry("ZUN", "Heian Alien", ["Game"])], genre_map)

        assert planned[0].is_change is True
        assert planned[0].current == ("Game",)


class TestUnmappedArtists:
    def test_lists_artists_absent_from_the_map_with_counts(self):
        genre_map = GenreMap({"zun": ("Touhou",)})
        entries = [
            entry("ZUN", "Heian Alien"),
            entry("Nobody", "One"),
            entry("Nobody", "Two"),
            entry("Someone", "Three"),
        ]

        assert unmapped_artists(entries, genre_map) == {"Nobody": 2, "Someone": 1}

    def test_the_largest_folders_come_first(self):
        entries = [entry("Big", str(i)) for i in range(3)] + [entry("Small", "x")]

        assert next(iter(unmapped_artists(entries, GenreMap()))) == "Big"


class TestApplyChange:
    def test_writes_the_genre_to_disk(self, tmp_path, encode_audio):
        from soulseektools.genres import apply_change
        from soulseektools.library import tags
        from soulseektools.library.index import describe

        track = encode_audio(tmp_path / "ZUN" / "Heian Alien.flac")
        genre_map = GenreMap({"zun": ("Touhou",)})
        change = plan_changes([describe(track, tmp_path)], genre_map)[0]

        apply_change(change)

        assert tags.read_tags(track).genres == ("Touhou",)

    def test_writes_several_genres(self, tmp_path, encode_audio):
        from soulseektools.genres import apply_change
        from soulseektools.library import tags
        from soulseektools.library.index import describe

        track = encode_audio(tmp_path / "SOUND HOLIC" / "Song.flac")
        genre_map = GenreMap({"sound holic": ("Touhou", "Eurobeat")})
        change = plan_changes([describe(track, tmp_path)], genre_map)[0]

        apply_change(change)

        assert tags.read_tags(track).genres == ("Touhou", "Eurobeat")
