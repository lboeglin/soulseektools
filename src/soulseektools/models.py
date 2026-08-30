"""Domain types shared between the library, artwork, Soulseek and review layers."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import StrEnum
from typing import TYPE_CHECKING

from soulseektools.matching import NameForms

if TYPE_CHECKING:
    from pathlib import Path


@dataclass(frozen=True, slots=True)
class TrackTags:
    """The metadata fields this project reads or writes, container-independent."""

    artist: str = ""
    album_artist: str = ""
    album: str = ""
    title: str = ""
    genres: tuple[str, ...] = ()
    track_number: int | None = None


@dataclass(frozen=True, slots=True)
class Artwork:
    """An embedded cover image."""

    data: bytes
    mime: str = "image/jpeg"

    def __len__(self) -> int:
        return len(self.data)


@dataclass(frozen=True, slots=True)
class LibraryEntry:
    """One audio file on disk, with everything needed to match it against another.

    ``artist`` and ``title`` are resolved once at index time: tags take priority,
    falling back to the directory layout and the filename, since a sizeable part
    of any Soulseek-sourced library arrives untagged.
    """

    path: Path
    size: int
    mtime: float
    folder_artist: str
    tags: TrackTags
    artist: NameForms
    title: NameForms
    duration_seconds: float | None = None
    artwork_bytes: int = 0

    @property
    def has_artwork(self) -> bool:
        return self.artwork_bytes > 0


@dataclass(frozen=True, slots=True)
class Track:
    """A track wanted from a streaming-service export."""

    artist: str
    title: str
    album: str = ""
    duration_seconds: float | None = None
    source_row: dict[str, str] = field(default_factory=dict)

    @property
    def query(self) -> str:
        return f"{self.artist} - {self.title}"


class Verdict(StrEnum):
    """Outcome of the lossless-authenticity check."""

    OK = "OK"
    SUSPECT = "SUSPECT"
    FAKE = "FAKE"
    ERROR = "ERROR"


@dataclass(frozen=True, slots=True)
class VerifyResult:
    """What the spectral check concluded about one file, and why."""

    path: Path
    verdict: Verdict
    detail: str
    sample_rate: int | None = None
    bit_depth: int | None = None
    codec: str = ""
    cutoff_hz: int | None = None
    nyquist_hz: int | None = None
    steepness_db_per_khz: float | None = None
