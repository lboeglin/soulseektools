"""Assigning a consistent genre taxonomy across a library.

The mapping lives in a TOML file rather than in this module. It is inherently
per-library data -- one person's artist roster says nothing about anyone else's
-- and editing it should not mean editing source.

Artists are matched on their normalised name, so a ``DECO*27`` entry also covers
a ``DECO_27`` folder, and an entry may name either one genre or several.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from soulseektools.config import read_toml
from soulseektools.errors import ConfigError, TagError, UnsupportedFormatError
from soulseektools.library import tags as tag_facade
from soulseektools.text.normalize import normalize

if TYPE_CHECKING:
    from collections.abc import Iterable, Mapping
    from pathlib import Path

    from soulseektools.models import LibraryEntry


@dataclass(frozen=True, slots=True)
class GenreMap:
    """Artist-to-genre assignments, looked up by normalised artist name."""

    by_artist: Mapping[str, tuple[str, ...]] = field(default_factory=dict)

    def __len__(self) -> int:
        return len(self.by_artist)

    def genres_for(self, artist: str) -> tuple[str, ...]:
        """The genres assigned to ``artist``, or an empty tuple when unmapped."""
        return self.by_artist.get(normalize(artist), ())

    def covers(self, artist: str) -> bool:
        return normalize(artist) in self.by_artist


@dataclass(frozen=True, slots=True)
class GenreChange:
    """One file's genre tag before and after."""

    entry: LibraryEntry
    current: tuple[str, ...]
    proposed: tuple[str, ...]

    @property
    def is_change(self) -> bool:
        return self.current != self.proposed


def load_genre_map(path: Path) -> GenreMap:
    """Read a genre map from TOML.

    The file is a flat table of ``"Artist" = "Genre"`` or
    ``"Artist" = ["Genre", "Other"]``, optionally nested under ``[artists]``.
    """
    data = read_toml(path)
    artists = data.get("artists", data)
    if not isinstance(artists, dict):
        raise ConfigError(f"{path}: expected a table of artist names")

    mapping: dict[str, tuple[str, ...]] = {}
    for artist, value in artists.items():
        if isinstance(value, str):
            genres: tuple[str, ...] = (value,)
        elif isinstance(value, list) and all(isinstance(item, str) for item in value):
            genres = tuple(value)
        else:
            raise ConfigError(f"{path}: '{artist}' must be a genre or a list of genres")
        mapping[normalize(artist)] = genres
    return GenreMap(by_artist=mapping)


def plan_changes(entries: Iterable[LibraryEntry], genre_map: GenreMap) -> list[GenreChange]:
    """What applying ``genre_map`` would do, without touching anything."""
    return [
        GenreChange(entry=entry, current=entry.tags.genres, proposed=genre_map.genres_for(artist))
        for entry in entries
        if genre_map.covers(artist := entry.folder_artist or entry.artist.raw)
    ]


def unmapped_artists(entries: Iterable[LibraryEntry], genre_map: GenreMap) -> dict[str, int]:
    """Artists present in the library but absent from the map, with file counts."""
    counts: dict[str, int] = {}
    for entry in entries:
        artist = entry.folder_artist or entry.artist.raw
        if artist and not genre_map.covers(artist):
            counts[artist] = counts.get(artist, 0) + 1
    return dict(sorted(counts.items(), key=lambda item: (-item[1], item[0])))


def apply_change(change: GenreChange) -> None:
    """Write one planned genre change to disk."""
    try:
        tag_facade.write_genres(change.entry.path, change.proposed)
    except (TagError, UnsupportedFormatError) as error:
        raise TagError(f"{change.entry.path}: {error}") from error
