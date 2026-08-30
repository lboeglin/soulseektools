"""Cover art from MusicBrainz and the Cover Art Archive.

The fallback when iTunes has nothing, which is common for doujin, Vocaloid and
game-soundtrack releases. MusicBrainz answers which release a recording belongs
to; the Cover Art Archive serves that release's front cover.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from soulseektools.artwork.base import ArtCandidate, RateLimiter

if TYPE_CHECKING:
    from collections.abc import Sequence

    import httpx

SEARCH_URL = "https://musicbrainz.org/ws/2/recording"
ARCHIVE_URL = "https://coverartarchive.org/release"
MIN_INTERVAL_SECONDS = 1.0
"""MusicBrainz enforces one request per second per address, then blocks.

Only the lookup is paced. The cover itself comes from the Internet Archive,
which does not share that budget.
"""


class CoverArtArchiveProvider:
    """Resolves a recording through MusicBrainz, then its cover through the archive."""

    name = "coverartarchive"

    def __init__(
        self,
        limit: int = 5,
        preferred_size: int = 1200,
        min_interval_seconds: float = MIN_INTERVAL_SECONDS,
    ) -> None:
        self.limit = limit
        self.size_segment = "1200" if preferred_size > 500 else "500"
        self.min_interval_seconds = min_interval_seconds
        self._limiter = RateLimiter(min_interval_seconds)

    def search(self, client: httpx.Client, artist: str, title: str) -> Sequence[ArtCandidate]:
        self._limiter.wait()
        response = client.get(
            SEARCH_URL,
            params={
                "query": f'recording:"{title}" AND artist:"{artist}"',
                "fmt": "json",
                "limit": self.limit,
            },
        )
        response.raise_for_status()

        candidates = []
        for recording in response.json().get("recordings", []):
            release_id = self._first_release_id(recording)
            if release_id is None:
                continue
            candidates.append(
                ArtCandidate(
                    url=f"{ARCHIVE_URL}/{release_id}/front-{self.size_segment}",
                    artist=self._credited_artist(recording),
                    title=recording.get("title", ""),
                    album=self._release_title(recording),
                    source=self.name,
                )
            )
        return candidates

    @staticmethod
    def _first_release_id(recording: dict[str, object]) -> str | None:
        releases = recording.get("releases")
        if not isinstance(releases, list) or not releases:
            return None
        first = releases[0]
        return str(first["id"]) if isinstance(first, dict) and first.get("id") else None

    @staticmethod
    def _release_title(recording: dict[str, object]) -> str:
        releases = recording.get("releases")
        if isinstance(releases, list) and releases and isinstance(releases[0], dict):
            return str(releases[0].get("title", ""))
        return ""

    @staticmethod
    def _credited_artist(recording: dict[str, object]) -> str:
        credits = recording.get("artist-credit")
        if not isinstance(credits, list):
            return ""
        names = [
            str(credit["name"])
            for credit in credits
            if isinstance(credit, dict) and credit.get("name")
        ]
        return " ".join(names)
