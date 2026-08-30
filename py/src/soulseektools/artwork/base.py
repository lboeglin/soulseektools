"""Cover-art providers, and the guard against embedding the wrong artist's art.

The script this replaces embedded the first search result without checking who
it belonged to, and pasted the wrong artist's cover onto files twice. Search
match quality is inconsistent for anything outside the mainstream, so a result
is only accepted here when the artist it names actually matches the artist asked
for -- and a file keeps no artwork rather than the wrong artwork.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from importlib import metadata
from typing import TYPE_CHECKING, Protocol

from soulseektools.matching import NameForms, similarity

if TYPE_CHECKING:
    from collections.abc import Sequence

    import httpx


def user_agent(contact: str = "") -> str:
    """The User-Agent sent to artwork providers.

    MusicBrainz asks callers to identify themselves with a real contact -- a URL
    or an email address -- and throttles or blocks those that do not. Nothing is
    invented here: set ``artwork.contact`` and it is included, leave it unset and
    the header simply names the tool and its version.
    """
    version = metadata.version("soulseektools")
    return (
        f"soulseektools/{version} ( {contact} )" if contact.strip() else f"soulseektools/{version}"
    )


@dataclass(slots=True)
class RateLimiter:
    """Holds a caller to a minimum interval between requests.

    These APIs are free and publish limits; exceeding them earns 503s and then a
    block. One limiter lives on each provider instance, so the pacing is per
    service rather than global -- a slow provider must not slow a fast one.
    """

    min_interval_seconds: float
    _next_allowed_at: float = field(default=0.0, repr=False)

    def wait(self) -> None:
        """Block until the next request is allowed."""
        if self.min_interval_seconds <= 0:
            return
        delay = self._next_allowed_at - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        self._next_allowed_at = time.monotonic() + self.min_interval_seconds


@dataclass(frozen=True, slots=True)
class ArtCandidate:
    """A cover image a provider believes belongs to the requested track."""

    url: str
    artist: str
    title: str
    source: str
    album: str = ""

    def artist_similarity(self, wanted: str) -> float:
        return similarity(NameForms.of(self.artist), NameForms.of(wanted))


@dataclass(frozen=True, slots=True)
class ArtSelection:
    """A chosen candidate, or the reason nothing was chosen."""

    candidate: ArtCandidate | None
    reason: str

    def __bool__(self) -> bool:
        return self.candidate is not None


class ArtProvider(Protocol):
    """A source of cover art."""

    name: str
    min_interval_seconds: float
    """The provider's published request limit, in seconds between calls."""

    def search(self, client: httpx.Client, artist: str, title: str) -> Sequence[ArtCandidate]:
        """Candidates for one track, best first."""
        ...


class ArtProviderFactory(Protocol):
    """How a provider is constructed, so the registry can stay typed."""

    def __call__(
        self, *, preferred_size: int = ..., min_interval_seconds: float = ...
    ) -> ArtProvider: ...


def select(
    candidates: Sequence[ArtCandidate],
    artist: str,
    *,
    require_artist_match: bool = True,
    artist_threshold: float = 0.8,
) -> ArtSelection:
    """Pick the best candidate whose artist matches the one asked for.

    With ``require_artist_match`` disabled the first result wins, reproducing
    the original behaviour; it is off by default for the reason in the module
    docstring.
    """
    if not candidates:
        return ArtSelection(None, "no results")

    if not require_artist_match:
        return ArtSelection(candidates[0], f"unverified match from {candidates[0].source}")

    scored = sorted(
        ((candidate.artist_similarity(artist), candidate) for candidate in candidates),
        key=lambda pair: -pair[0],
    )
    best_score, best = scored[0]
    if best_score < artist_threshold:
        return ArtSelection(
            None,
            f"best result is by '{best.artist}' ({best_score:.2f} similarity), not '{artist}'",
        )
    return ArtSelection(best, f"matched '{best.artist}' ({best_score:.2f}) via {best.source}")
