"""Reading a streaming-service export and working out what is missing.

The ownership test is deliberately asymmetric. A track is dropped from the
wantlist only when a library file is convincingly the same song; anything
uncertain stays in the list, because downloading a duplicate costs a few
megabytes and silently skipping a track loses it.

Column names vary between exporters, so the reader accepts the common spellings
rather than requiring one exact header.
"""

from __future__ import annotations

import csv
from dataclasses import dataclass
from typing import TYPE_CHECKING

from soulseektools.errors import SoulseekToolsError
from soulseektools.matching import MatchThresholds, NameForms, blocking_keys, score_track
from soulseektools.models import Track

if TYPE_CHECKING:
    from collections.abc import Iterable, Sequence
    from pathlib import Path

    from soulseektools.models import LibraryEntry

ARTIST_COLUMNS = ("artist name", "artist name(s)", "artist", "artists", "album artist")
TITLE_COLUMNS = ("track name", "title", "name", "song")
ALBUM_COLUMNS = ("album name", "album")
DURATION_COLUMNS = ("duration (ms)", "duration_ms", "track duration (ms)", "length")


@dataclass(frozen=True, slots=True)
class OwnedTrack:
    """A wanted track matched to a file already in the library."""

    track: Track
    entry: LibraryEntry
    score: float


@dataclass(frozen=True, slots=True)
class WantlistDiff:
    """What an export contains that the library does not."""

    missing: tuple[Track, ...]
    owned: tuple[OwnedTrack, ...]

    @property
    def total(self) -> int:
        return len(self.missing) + len(self.owned)


def read_export(path: Path) -> list[Track]:
    """Read a streaming-service CSV export into tracks."""
    with path.open(encoding="utf-8-sig", newline="") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None:
            raise SoulseekToolsError(f"{path}: file is empty")

        columns = {(name or "").strip().lower(): name for name in reader.fieldnames}
        artist_column = _column(columns, ARTIST_COLUMNS)
        title_column = _column(columns, TITLE_COLUMNS)
        if artist_column is None or title_column is None:
            raise SoulseekToolsError(
                f"{path}: need an artist and a title column, found {', '.join(columns)}"
            )
        album_column = _column(columns, ALBUM_COLUMNS)
        duration_column = _column(columns, DURATION_COLUMNS)

        tracks = []
        for row in reader:
            artist = (row.get(artist_column) or "").strip()
            title = (row.get(title_column) or "").strip()
            if not artist and not title:
                continue
            tracks.append(
                Track(
                    artist=artist,
                    title=title,
                    album=(row.get(album_column) or "").strip() if album_column else "",
                    duration_seconds=_duration(
                        row.get(duration_column) if duration_column else None
                    ),
                    source_row={key: value for key, value in row.items() if key},
                )
            )
    return tracks


def diff(
    tracks: Iterable[Track],
    entries: Sequence[LibraryEntry],
    thresholds: MatchThresholds | None = None,
) -> WantlistDiff:
    """Split wanted tracks into those already owned and those still missing."""
    limits = thresholds or MatchThresholds()
    index = _by_gram(entries)

    missing: list[Track] = []
    owned: list[OwnedTrack] = []

    for track in tracks:
        artist = NameForms.of(track.artist)
        title = NameForms.of(track.title)
        best: OwnedTrack | None = None

        for position in _candidates(index, artist, title):
            entry = entries[position]
            score = score_track(artist, title, entry.artist, entry.title, limits)
            if score is None:
                continue
            if best is None or score.combined > best.score:
                best = OwnedTrack(track=track, entry=entry, score=score.combined)

        if best is None:
            missing.append(track)
        else:
            owned.append(best)

    return WantlistDiff(missing=tuple(missing), owned=tuple(owned))


def _by_gram(entries: Sequence[LibraryEntry]) -> dict[str, list[int]]:
    index: dict[str, list[int]] = {}
    for position, entry in enumerate(entries):
        for gram in blocking_keys(entry.artist, entry.title):
            index.setdefault(gram, []).append(position)
    return index


def _candidates(index: dict[str, list[int]], artist: NameForms, title: NameForms) -> set[int]:
    positions: set[int] = set()
    for gram in blocking_keys(artist, title):
        positions.update(index.get(gram, ()))
    return positions


def _column(columns: dict[str, str], names: Sequence[str]) -> str | None:
    for name in names:
        if name in columns:
            return columns[name]
    return None


def _duration(raw: str | None) -> float | None:
    """Read a duration, accepting both milliseconds and ``m:ss``."""
    if not raw:
        return None
    text = raw.strip()
    if ":" in text:
        parts = text.split(":")
        try:
            seconds = 0.0
            for part in parts:
                seconds = seconds * 60 + float(part)
        except ValueError:
            return None
        return seconds
    try:
        value = float(text)
    except ValueError:
        return None
    # Exporters write milliseconds; a track measured in seconds is never this long.
    return value / 1000.0 if value > 3600 else value
