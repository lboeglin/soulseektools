"""Finding, verifying and embedding cover art for a library."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

import httpx

from soulseektools.artwork.base import (
    ArtCandidate,
    ArtProvider,
    ArtProviderFactory,
    ArtSelection,
    select,
    user_agent,
)
from soulseektools.artwork.coverartarchive import CoverArtArchiveProvider
from soulseektools.artwork.itunes import ITunesProvider
from soulseektools.errors import ConfigError, TagError, UnsupportedFormatError
from soulseektools.library import tags as tag_facade
from soulseektools.models import Artwork

if TYPE_CHECKING:
    from collections.abc import Iterable, Sequence

    from soulseektools.config import ArtworkSettings
    from soulseektools.models import LibraryEntry

_PROVIDERS: dict[str, ArtProviderFactory] = {
    ITunesProvider.name: ITunesProvider,
    CoverArtArchiveProvider.name: CoverArtArchiveProvider,
}

_MIME_BY_MAGIC = ((b"\x89PNG\r\n\x1a\n", "image/png"), (b"\xff\xd8\xff", "image/jpeg"))


@dataclass(frozen=True, slots=True)
class ArtworkStatus:
    """Why a file needs artwork, or why it does not."""

    entry: LibraryEntry
    needs_artwork: bool
    reason: str


@dataclass(frozen=True, slots=True)
class ArtworkResult:
    """The outcome of looking artwork up for one file."""

    entry: LibraryEntry
    selection: ArtSelection
    artwork: Artwork | None = None
    error: str = ""

    @property
    def ok(self) -> bool:
        return self.artwork is not None and not self.error


def build_providers(settings: ArtworkSettings) -> list[ArtProvider]:
    """Instantiate the configured providers, in the configured order."""
    providers: list[ArtProvider] = []
    for name in settings.providers:
        provider = _PROVIDERS.get(name)
        if provider is None:
            raise ConfigError(
                f"unknown artwork provider '{name}'; choose from {', '.join(_PROVIDERS)}"
            )
        interval = settings.min_request_interval_seconds
        providers.append(
            provider(preferred_size=settings.preferred_size)
            if interval is None
            else provider(preferred_size=settings.preferred_size, min_interval_seconds=interval)
        )
    return providers


def audit(entries: Iterable[LibraryEntry], settings: ArtworkSettings) -> list[ArtworkStatus]:
    """Classify each file as having usable artwork, or needing some.

    Artwork smaller than ``min_bytes`` counts as missing: a cover that small is a
    broken download or a placeholder, not an image.
    """
    statuses = []
    for entry in entries:
        if not entry.has_artwork:
            statuses.append(ArtworkStatus(entry, True, "no embedded cover"))
        elif entry.artwork_bytes < settings.min_bytes:
            statuses.append(
                ArtworkStatus(entry, True, f"cover is only {entry.artwork_bytes} bytes")
            )
        else:
            statuses.append(ArtworkStatus(entry, False, f"{entry.artwork_bytes} bytes"))
    return statuses


def find_artwork(
    entry: LibraryEntry,
    providers: Sequence[ArtProvider],
    settings: ArtworkSettings,
    client: httpx.Client | None = None,
) -> ArtworkResult:
    """Search each provider in turn and download the first verified match."""
    owned = client is None
    session = client or new_client(settings)
    artist = entry.tags.artist or entry.folder_artist or entry.artist.raw
    title = entry.tags.title or entry.title.raw

    try:
        selection = ArtSelection(None, "no providers configured")
        for provider in providers:
            try:
                candidates = provider.search(session, artist, title)
            except httpx.HTTPError as error:
                selection = ArtSelection(None, f"{provider.name} unavailable: {error}")
                continue

            selection = select(
                candidates,
                artist,
                require_artist_match=settings.require_artist_match,
                artist_threshold=settings.artist_match_threshold,
            )
            if selection:
                break

        if selection.candidate is None:
            return ArtworkResult(entry=entry, selection=selection)

        try:
            artwork = download(session, selection.candidate.url)
        except httpx.HTTPError as error:
            return ArtworkResult(entry=entry, selection=selection, error=str(error))

        if len(artwork) < settings.min_bytes:
            return ArtworkResult(
                entry=entry,
                selection=selection,
                error=f"downloaded image is only {len(artwork)} bytes",
            )
        return ArtworkResult(entry=entry, selection=selection, artwork=artwork)
    finally:
        if owned:
            session.close()


def download(client: httpx.Client, url: str) -> Artwork:
    """Fetch an image, taking its type from the bytes rather than the headers."""
    response = client.get(url, follow_redirects=True)
    response.raise_for_status()
    return Artwork(data=response.content, mime=detect_mime(response.content))


def detect_mime(data: bytes) -> str:
    for magic, mime in _MIME_BY_MAGIC:
        if data.startswith(magic):
            return mime
    return "image/jpeg"


def embed(result: ArtworkResult) -> None:
    """Write a verified cover into its file."""
    if result.artwork is None:
        raise ValueError(f"{result.entry.path}: nothing to embed")
    try:
        tag_facade.write_artwork(result.entry.path, result.artwork)
    except (TagError, UnsupportedFormatError) as error:
        raise TagError(f"{result.entry.path}: {error}") from error


def new_client(settings: ArtworkSettings) -> httpx.Client:
    return httpx.Client(
        timeout=settings.request_timeout_seconds,
        headers={"User-Agent": user_agent(settings.contact)},
        follow_redirects=True,
    )


__all__ = [
    "ArtCandidate",
    "ArtworkResult",
    "ArtworkStatus",
    "audit",
    "build_providers",
    "detect_mime",
    "download",
    "embed",
    "find_artwork",
    "new_client",
]
