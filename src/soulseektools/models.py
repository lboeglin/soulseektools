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


@dataclass(frozen=True, slots=True)
class Candidate:
    """One file offered by a Soulseek peer, as the search results describe it."""

    username: str
    remote_path: str
    size: int = 0
    bitrate: int | None = None
    sample_rate: int | None = None
    bit_depth: int | None = None
    is_vbr: bool = False
    free_upload_slots: bool = False
    queue_length: int = 0
    duration_seconds: float | None = None

    @property
    def filename(self) -> str:
        return self.remote_path.replace("/", "\\").rsplit("\\", 1)[-1]

    @property
    def folder(self) -> str:
        """The remote directory holding the file, which often says what it really is."""
        return self.remote_path.replace("/", "\\").rsplit("\\", 1)[0]

    @property
    def extension(self) -> str:
        _, _, suffix = self.filename.rpartition(".")
        return suffix.lower()


@dataclass(frozen=True, slots=True)
class ScoredCandidate:
    """A candidate with its ranking, and any warnings raised about it."""

    candidate: Candidate
    score: int
    title_ratio: float
    flags: tuple[str, ...] = ()

    @property
    def rejected(self) -> bool:
        return self.score < 0

    def describe(self) -> str:
        parts = [self.candidate.filename, f"[{self.candidate.extension or '?'}"]
        if self.candidate.bit_depth and self.candidate.sample_rate:
            parts[-1] += (
                f", {self.candidate.sample_rate / 1000:.1f}kHz/{self.candidate.bit_depth}bit"
            )
        elif self.candidate.bitrate:
            parts[-1] += f", {self.candidate.bitrate}kbps{'VBR' if self.candidate.is_vbr else ''}"
        parts[-1] += "]"
        parts.append(f"user={self.candidate.username}")
        parts.append(f"queue={self.candidate.queue_length}")
        parts.append("slot=free" if self.candidate.free_upload_slots else "slot=busy")
        if self.flags:
            parts.append(f"flags={','.join(self.flags)}")
        return " ".join(parts)


class TrackStatus(StrEnum):
    """Where a wanted track has got to."""

    PENDING = "pending"
    NO_CANDIDATES = "no_candidates"
    AWAITING_REVIEW = "awaiting_review"
    APPROVED = "approved"
    REJECTED = "rejected"
    QUEUED = "queued"
    DOWNLOADED = "downloaded"
    FAILED = "failed"


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
