"""Cover art from the iTunes Search API.

No authentication, direct image URLs and good coverage of commercial releases,
which makes it the first provider tried. Its match quality outside the
mainstream is poor, which is why every result goes through the artist check in
:func:`soulseektools.artwork.base.select`.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from soulseektools.artwork.base import ArtCandidate

if TYPE_CHECKING:
    from collections.abc import Sequence

    import httpx

SEARCH_URL = "https://itunes.apple.com/search"
_THUMBNAIL_SIZE = "100x100bb"


class ITunesProvider:
    """Looks up artwork through the public iTunes search endpoint."""

    name = "itunes"

    def __init__(self, limit: int = 5, preferred_size: int = 1200) -> None:
        self.limit = limit
        self.preferred_size = preferred_size

    def search(self, client: httpx.Client, artist: str, title: str) -> Sequence[ArtCandidate]:
        response = client.get(
            SEARCH_URL,
            params={"term": f"{artist} {title}", "media": "music", "limit": self.limit},
        )
        response.raise_for_status()

        candidates = []
        for result in response.json().get("results", []):
            thumbnail = result.get("artworkUrl100") or ""
            if not thumbnail:
                continue
            candidates.append(
                ArtCandidate(
                    url=self._upscale(thumbnail),
                    artist=result.get("artistName", ""),
                    title=result.get("trackName", ""),
                    album=result.get("collectionName", ""),
                    source=self.name,
                )
            )
        return candidates

    def _upscale(self, thumbnail_url: str) -> str:
        """Ask for a full-size image instead of the 100px thumbnail in the response."""
        size = f"{self.preferred_size}x{self.preferred_size}bb"
        return thumbnail_url.replace(_THUMBNAIL_SIZE, size)
