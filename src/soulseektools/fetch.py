"""Driving a batch: search for what is wanted, queue what was approved, check what arrived.

Nothing downloads without a human choosing a candidate. That was true of the
plugin this replaces and stays true here; the difference is that the choice is
recorded in the review queue rather than typed into a spreadsheet column.

Completed downloads are run through the spectral check in-process, rather than
by shelling out to a script at a hardcoded path as the plugin did.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from soulseektools.audio.verify import verify_file
from soulseektools.errors import SoulseekClientError
from soulseektools.library.scan import LOSSLESS_EXTENSIONS
from soulseektools.models import TrackStatus, Verdict
from soulseektools.soulseek import ranking
from soulseektools.soulseek.base import TransferState
from soulseektools.text.normalize import normalize, sanitize_folder_name

if TYPE_CHECKING:
    from collections.abc import Callable
    from pathlib import Path

    from soulseektools.config import Settings
    from soulseektools.models import VerifyResult
    from soulseektools.review.store import ReviewStore, ReviewTrack
    from soulseektools.soulseek.base import SoulseekClient, Transfer


@dataclass(frozen=True, slots=True)
class SearchOutcome:
    """What one track's search produced."""

    track_id: int
    query: str
    found: int
    error: str = ""


@dataclass(frozen=True, slots=True)
class EnqueueOutcome:
    """What happened when an approved candidate was queued."""

    track_id: int
    query: str
    destination: str
    queued: bool
    error: str = ""


@dataclass(frozen=True, slots=True)
class CollectOutcome:
    """A finished transfer, and what the file turned out to be."""

    track_id: int
    query: str
    state: TransferState
    local_path: Path | None = None
    verdict: VerifyResult | None = None
    error: str = ""


def search_pending(
    store: ReviewStore,
    client: SoulseekClient,
    settings: Settings,
    *,
    limit: int | None = None,
    on_track: Callable[[ReviewTrack], None] | None = None,
) -> list[SearchOutcome]:
    """Search Soulseek for every track still waiting, and record the candidates."""
    outcomes = []
    for entry in store.tracks(TrackStatus.PENDING, limit=limit):
        if on_track is not None:
            on_track(entry)

        query = normalize(f"{entry.track.artist} {entry.track.title}")
        try:
            candidates = client.search(
                query,
                timeout_seconds=settings.slskd.search_timeout_seconds,
                response_limit=settings.slskd.search_response_limit,
            )
        except SoulseekClientError as error:
            store.mark(entry.id, TrackStatus.FAILED, str(error))
            outcomes.append(SearchOutcome(entry.id, entry.track.query, 0, str(error)))
            continue

        scored = ranking.rank(
            entry.track,
            candidates,
            settings.ranking,
            limit=settings.review.candidates_per_track,
        )
        store.set_candidates(entry.id, scored)
        outcomes.append(SearchOutcome(entry.id, entry.track.query, len(scored)))
    return outcomes


def enqueue_approved(
    store: ReviewStore,
    client: SoulseekClient,
    *,
    limit: int | None = None,
) -> list[EnqueueOutcome]:
    """Queue every approved pick, sorting each into a folder named for its artist."""
    outcomes = []
    for entry in store.tracks(TrackStatus.APPROVED, limit=limit):
        approved = entry.approved
        if approved is None:
            store.mark(entry.id, TrackStatus.FAILED, "approved candidate is missing")
            outcomes.append(EnqueueOutcome(entry.id, entry.track.query, "", False, "no candidate"))
            continue

        destination = sanitize_folder_name(entry.track.artist)
        try:
            client.enqueue(
                approved.candidate.username, [approved.candidate], destination=destination
            )
        except SoulseekClientError as error:
            store.mark(entry.id, TrackStatus.FAILED, str(error))
            outcomes.append(
                EnqueueOutcome(entry.id, entry.track.query, destination, False, str(error))
            )
            continue

        store.mark(entry.id, TrackStatus.QUEUED, f"from {approved.candidate.username}")
        outcomes.append(EnqueueOutcome(entry.id, entry.track.query, destination, True))
    return outcomes


def collect(
    store: ReviewStore,
    client: SoulseekClient,
    settings: Settings,
    *,
    verify: bool = True,
) -> list[CollectOutcome]:
    """Update queued tracks from the backend's transfer list, verifying what arrived."""
    finished = {
        transfer.remote_path: transfer for transfer in client.transfers() if transfer.state.finished
    }
    if not finished:
        return []

    outcomes = []
    for entry in store.tracks(TrackStatus.QUEUED):
        approved = entry.approved
        transfer = finished.get(approved.candidate.remote_path) if approved else None
        if transfer is None:
            continue

        if transfer.state is not TransferState.COMPLETED:
            store.mark(entry.id, TrackStatus.FAILED, transfer.error or str(transfer.state))
            outcomes.append(
                CollectOutcome(entry.id, entry.track.query, transfer.state, error=transfer.error)
            )
            continue

        local_path = downloaded_path(settings, entry.track.artist, transfer)
        verdict = _verify(local_path, settings) if verify else None
        store.mark(entry.id, TrackStatus.DOWNLOADED, _describe(verdict))
        outcomes.append(
            CollectOutcome(
                track_id=entry.id,
                query=entry.track.query,
                state=transfer.state,
                local_path=local_path,
                verdict=verdict,
            )
        )
    return outcomes


def downloaded_path(settings: Settings, artist: str, transfer: Transfer) -> Path | None:
    """Where the finished file should be on disk.

    slskd reports the remote path, not the local one, so the destination is
    rebuilt from the configured download directory, the artist folder the batch
    was queued into, and the file's own name.
    """
    root = settings.slskd.download_directory
    if root is None:
        return None
    filename = transfer.remote_path.replace("/", "\\").rsplit("\\", 1)[-1]
    return root / sanitize_folder_name(artist) / filename


def _verify(local_path: Path | None, settings: Settings) -> VerifyResult | None:
    if local_path is None or not local_path.is_file():
        return None
    if local_path.suffix.lower() not in LOSSLESS_EXTENSIONS:
        return None
    return verify_file(local_path, settings.verify)


def _describe(verdict: VerifyResult | None) -> str:
    if verdict is None:
        return "downloaded"
    if verdict.verdict is Verdict.OK:
        return "downloaded, verified lossless"
    return f"downloaded, {verdict.verdict}: {verdict.detail}"
