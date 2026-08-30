"""What the rest of the package needs from a Soulseek backend.

Everything above this line works against the protocol, not against slskd, so the
search and download flows are exercised in tests by a fake with no network,
no daemon and no Soulseek account.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from collections.abc import Sequence

    from soulseektools.models import Candidate


class TransferState(StrEnum):
    """Where a download has got to, collapsed from the backend's own vocabulary."""

    QUEUED = "queued"
    IN_PROGRESS = "in_progress"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"

    @property
    def finished(self) -> bool:
        return self in {TransferState.COMPLETED, TransferState.FAILED, TransferState.CANCELLED}


@dataclass(frozen=True, slots=True)
class Transfer:
    """One download, as the backend currently sees it."""

    id: str
    username: str
    remote_path: str
    state: TransferState
    size: int = 0
    bytes_transferred: int = 0
    place_in_queue: int | None = None
    local_path: str = ""
    error: str = ""

    @property
    def percent_complete(self) -> float:
        return 100.0 * self.bytes_transferred / self.size if self.size else 0.0


class SoulseekClient(Protocol):
    """A Soulseek backend that can search and download."""

    def is_connected(self) -> bool:
        """Whether the backend is logged in and able to search."""
        ...

    def search(self, text: str, *, timeout_seconds: int, response_limit: int) -> list[Candidate]:
        """Run one search to completion and return every file offered."""
        ...

    def enqueue(
        self, username: str, files: Sequence[Candidate], *, destination: str = ""
    ) -> list[Transfer]:
        """Queue files for download from one peer."""
        ...

    def transfers(self) -> list[Transfer]:
        """Every download the backend is tracking."""
        ...
