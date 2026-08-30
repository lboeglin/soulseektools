"""Scoring Soulseek search results.

The scorer this replaces looked only at a result's *filename*, and that was the
root cause of most wrong downloads: a file named exactly right is still, often
enough, a live recording, an orchestral cover, or a medley track, and none of
that is visible from the filename. Only the folder says so --
``@@user\\Symphony Concert 2019\\03 Roki.flac`` is unmistakable and was
previously invisible.

So the whole remote path is scored. Terms are matched against folder and
filename together, and a term is ignored when the wanted track's own name
contains it: someone asking for "Live and Learn" is not asking to reject every
result containing "live".
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from soulseektools.config import RankingSettings
from soulseektools.library.scan import LOSSLESS_EXTENSIONS
from soulseektools.matching import NameForms, similarity
from soulseektools.models import ScoredCandidate
from soulseektools.text.normalize import flatten, strip_track_number

if TYPE_CHECKING:
    from collections.abc import Iterable, Sequence

    from soulseektools.models import Candidate, Track

LOSSY_EXTENSIONS = frozenset({"mp3", "ogg", "m4a", "aac", "opus", "wma"})
AUDIO_EXTENSIONS = frozenset(
    {extension.lstrip(".") for extension in LOSSLESS_EXTENSIONS} | LOSSY_EXTENSIONS
)

_TIER_WEIGHT = 10_000_000
_QUALITY_WEIGHT = 10
_TITLE_WEIGHT = 500
_FREE_SLOT_BONUS = 1000
_MAX_QUEUE_PENALTY = 500
_HIGH_BITRATE = 256
_ARTIST_WEIGHT_AGAINST_TITLE = 0.6

_REJECTED_SCORE = -1
_SEPARATORS = re.compile(r"[\\/]+")


def rank(
    track: Track,
    candidates: Iterable[Candidate],
    settings: RankingSettings | None = None,
    *,
    limit: int | None = None,
) -> list[ScoredCandidate]:
    """Score and order candidates, best first, dropping the unusable ones."""
    options = settings or RankingSettings()
    wanted = flatten(f"{track.artist} {track.title}")
    artist = NameForms.of(track.artist)
    title = NameForms.of(track.title)

    scored = [
        result
        for candidate in candidates
        if (result := score(candidate, track, artist, title, wanted, options)) is not None
    ]
    scored.sort(key=lambda item: -item.score)
    return scored[:limit] if limit is not None else scored


def score(
    candidate: Candidate,
    track: Track,
    artist: NameForms,
    title: NameForms,
    wanted: str,
    options: RankingSettings,
) -> ScoredCandidate | None:
    """Score one candidate, or return ``None`` when it is not worth offering."""
    if candidate.extension not in AUDIO_EXTENSIONS:
        return None

    stem = strip_track_number(candidate.filename.rsplit(".", 1)[0])
    name = NameForms.of(stem)
    title_ratio = similarity(title, name)
    artist_ratio = similarity(artist, name)

    if max(title_ratio, artist_ratio * _ARTIST_WEIGHT_AGAINST_TITLE) < options.min_title_ratio:
        return None

    # Flattened, not normalised: a bracketed "(Live)" is the whole point here.
    path_text = flatten(_SEPARATORS.sub(" ", candidate.remote_path))
    rejected = terms_in(path_text, options.reject_terms, wanted)
    if rejected:
        return ScoredCandidate(
            candidate=candidate,
            score=_REJECTED_SCORE,
            title_ratio=title_ratio,
            flags=tuple(f"reject:{term}" for term in rejected),
        )

    total = _quality_score(candidate)
    total += _FREE_SLOT_BONUS if candidate.free_upload_slots else 0
    total -= min(candidate.queue_length, _MAX_QUEUE_PENALTY)
    total += int(title_ratio * _TITLE_WEIGHT)

    flags = []
    penalised = terms_in(path_text, options.penalise_terms, wanted)
    total -= options.penalty_per_term * len(penalised)
    flags.extend(penalised)

    if _duration_differs(candidate, track, options.duration_tolerance_seconds):
        total -= options.penalty_per_term
        flags.append("duration")

    return ScoredCandidate(
        candidate=candidate,
        score=max(total, 0),
        title_ratio=title_ratio,
        flags=tuple(flags),
    )


def terms_in(path_text: str, terms: Sequence[str], wanted: str) -> list[str]:
    """Terms appearing in the remote path but not in the track's own name.

    A term the caller actually asked for is not evidence against a result: a
    wantlist entry for "Live and Learn" must not reject every path saying "live".
    """
    return [
        term
        for term in terms
        if (normalised := flatten(term)) and normalised in path_text and normalised not in wanted
    ]


def _quality_score(candidate: Candidate) -> int:
    is_lossless = (
        f".{candidate.extension}" in LOSSLESS_EXTENSIONS or candidate.bit_depth is not None
    )
    if is_lossless:
        tier, quality = 3, (candidate.sample_rate or 44100) * (candidate.bit_depth or 16)
    elif candidate.bitrate:
        tier = 2 if candidate.bitrate >= _HIGH_BITRATE else 1
        quality = candidate.bitrate
    else:
        tier, quality = 0, 0
    return tier * _TIER_WEIGHT + quality * _QUALITY_WEIGHT


def _duration_differs(candidate: Candidate, track: Track, tolerance_seconds: float) -> bool:
    """Whether the offered file is a different length from the track wanted.

    Catches an edit, an extended mix or an entirely different song sharing a
    title, which no amount of name matching can distinguish.
    """
    if not candidate.duration_seconds or not track.duration_seconds:
        return False
    return abs(candidate.duration_seconds - track.duration_seconds) > tolerance_seconds
