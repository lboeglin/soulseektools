"""Finding the same track twice in a library.

Two independent passes, because they fail in different directions:

- **by title**, which catches the same song filed under two spellings of the
  artist (``Inabakumori`` and ``稲葉曇``) but is blind to untagged files with
  meaningless names;
- **by audio**, which catches those but needs decoding, so a cheap duration
  prefilter narrows the field before any fingerprint is computed.

The title pass compares only entries sharing an n-gram (see
:func:`soulseektools.matching.blocking_keys`) rather than every pair against
every other, which is what makes it usable on a library of any size.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from soulseektools.audio import fingerprint as fingerprints
from soulseektools.errors import ExternalToolError
from soulseektools.matching import MatchThresholds, blocking_keys, candidate_pairs, score_track

if TYPE_CHECKING:
    from collections.abc import Callable, Sequence

    from soulseektools.library.index import LibraryIndex
    from soulseektools.models import LibraryEntry

DEFAULT_AUDIO_THRESHOLD = 0.9
DEFAULT_DURATION_TOLERANCE_SECONDS = 2.0


@dataclass(frozen=True, slots=True)
class DuplicatePair:
    """Two files that look like the same recording, and the evidence for it."""

    left: LibraryEntry
    right: LibraryEntry
    score: float
    evidence: str

    @property
    def larger(self) -> LibraryEntry:
        """The copy worth keeping, all else being equal."""
        return self.left if self.left.size >= self.right.size else self.right

    @property
    def smaller(self) -> LibraryEntry:
        return self.right if self.left.size >= self.right.size else self.left


def find_by_title(
    entries: Sequence[LibraryEntry], thresholds: MatchThresholds | None = None
) -> list[DuplicatePair]:
    """Pairs whose artist and title match, across the whole library."""
    limits = thresholds or MatchThresholds()
    keyed = [
        (index, blocking_keys(entry.artist, entry.title)) for index, entry in enumerate(entries)
    ]

    pairs = []
    for left_index, right_index in candidate_pairs(keyed):
        left, right = entries[left_index], entries[right_index]
        if left.path == right.path:
            continue
        score = score_track(left.artist, left.title, right.artist, right.title, limits)
        if score is None:
            continue
        pairs.append(
            DuplicatePair(
                left=left,
                right=right,
                score=score.combined,
                evidence=f"artist {score.artist:.2f}, title {score.title:.2f}",
            )
        )
    return sorted(pairs, key=lambda pair: -pair.score)


def find_by_audio(
    entries: Sequence[LibraryEntry],
    *,
    threshold: float = DEFAULT_AUDIO_THRESHOLD,
    duration_tolerance_seconds: float = DEFAULT_DURATION_TOLERANCE_SECONDS,
    index: LibraryIndex | None = None,
    on_fingerprint: Callable[[LibraryEntry], None] | None = None,
) -> list[DuplicatePair]:
    """Pairs whose audio matches, whatever they are named or tagged.

    Duration is only a prefilter here. Treating a duration and size match as
    proof, which the original script did, produces coincidental hits between
    unrelated tracks of similar length; the fingerprint is what decides.
    """
    comparable = [entry for entry in entries if entry.duration_seconds]
    candidates = _pairs_within_duration(comparable, duration_tolerance_seconds)
    if not candidates:
        return []

    needed = {position for pair in candidates for position in pair}
    computed: dict[int, fingerprints.Fingerprint] = {}
    for position in sorted(needed):
        entry = comparable[position]
        if on_fingerprint is not None:
            on_fingerprint(entry)
        value = _fingerprint_of(entry, index)
        if value is not None and len(value):
            computed[position] = value

    pairs = []
    for left_index, right_index in candidates:
        left_print = computed.get(left_index)
        right_print = computed.get(right_index)
        if left_print is None or right_print is None:
            continue
        score = fingerprints.similarity(left_print, right_print)
        if score < threshold:
            continue
        pairs.append(
            DuplicatePair(
                left=comparable[left_index],
                right=comparable[right_index],
                score=score,
                evidence=f"{left_print.kind} similarity {score:.2f}",
            )
        )
    return sorted(pairs, key=lambda pair: -pair.score)


def _pairs_within_duration(
    entries: Sequence[LibraryEntry], tolerance_seconds: float
) -> list[tuple[int, int]]:
    ordered = sorted(range(len(entries)), key=lambda i: entries[i].duration_seconds or 0.0)

    pairs = []
    for offset, left in enumerate(ordered):
        left_duration = entries[left].duration_seconds or 0.0
        for right in ordered[offset + 1 :]:
            right_duration = entries[right].duration_seconds or 0.0
            if right_duration - left_duration > tolerance_seconds:
                break
            pairs.append((left, right) if left < right else (right, left))
    return pairs


def _fingerprint_of(
    entry: LibraryEntry, index: LibraryIndex | None
) -> fingerprints.Fingerprint | None:
    if index is not None:
        cached = index.fingerprint(entry.path, entry.mtime)
        if cached is not None:
            return cached
    try:
        value = fingerprints.fingerprint(entry.path)
    except ExternalToolError:
        return None
    if index is not None:
        index.store_fingerprint(entry.path, entry.mtime, value)
    return value
