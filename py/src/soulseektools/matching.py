"""Fuzzy matching between wantlist tracks, library files and search results.

Every comparison in the project runs through :func:`similarity`, which scores a
name against another in both its normalised and its romanised form. The
asymmetric thresholds in :class:`MatchThresholds` encode a deliberate bias: a
very strong title match is allowed to carry a weak artist match, because artist
readings are the unreliable half (see :mod:`soulseektools.text.romaji`), but the
reverse is never accepted.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from rapidfuzz import fuzz

from soulseektools.text.normalize import normalize
from soulseektools.text.romaji import to_romaji

if TYPE_CHECKING:
    from collections.abc import Iterable, Iterator

CONTAINMENT_SCORE = 0.85
"""Score awarded when one name is contained in the other.

Truncated and extended filenames ("Senbonzakura" vs "Senbonzakura -Long Ver-")
are the same track far more often than their raw character ratio suggests.
"""

_BLOCKING_GRAM_SIZE = 4


@dataclass(frozen=True, slots=True)
class NameForms:
    """A name in each of the forms the matcher compares."""

    raw: str
    normalized: str
    romaji: str

    @classmethod
    def of(cls, text: str | None) -> NameForms:
        normalized = normalize(text)
        return cls(raw=text or "", normalized=normalized, romaji=to_romaji(normalized))

    def __bool__(self) -> bool:
        return bool(self.normalized)


@dataclass(frozen=True, slots=True)
class MatchThresholds:
    """Cut-offs deciding whether two names describe the same track."""

    artist: float = 0.6
    artist_weak: float = 0.35
    title: float = 0.62
    title_strong: float = 0.85
    artist_weight: float = 0.4

    @property
    def title_weight(self) -> float:
        return 1.0 - self.artist_weight


@dataclass(frozen=True, slots=True)
class MatchScore:
    """The outcome of comparing an (artist, title) pair against another."""

    artist: float
    title: float
    combined: float


def _pair_ratio(left: str, right: str) -> float:
    if not left or not right:
        return 0.0
    best = max(fuzz.ratio(left, right), fuzz.token_sort_ratio(left, right)) / 100.0
    if left in right or right in left:
        best = max(best, CONTAINMENT_SCORE)
    return best


def similarity(left: NameForms, right: NameForms) -> float:
    """Best similarity in ``[0, 1]`` across the normalised and romanised forms.

    All four combinations are compared, including romaji against romaji: two
    different kanji spellings that share a reading (``稲葉曇`` / ``いなば くもり``)
    only line up once both sides are transliterated.
    """
    if not left or not right:
        return 0.0
    return max(
        _pair_ratio(left.normalized, right.normalized),
        _pair_ratio(left.romaji, right.normalized),
        _pair_ratio(left.normalized, right.romaji),
        _pair_ratio(left.romaji, right.romaji),
    )


def score_track(
    left_artist: NameForms,
    left_title: NameForms,
    right_artist: NameForms,
    right_title: NameForms,
    thresholds: MatchThresholds,
) -> MatchScore | None:
    """Score two tracks against each other, or return ``None`` if they differ.

    Returning ``None`` rather than a low score keeps callers conservative by
    construction: an uncertain pair is never silently treated as a match.
    """
    artist = similarity(left_artist, right_artist)
    title = similarity(left_title, right_title)

    strong_title_match = title >= thresholds.title_strong and artist >= thresholds.artist_weak
    ordinary_match = artist >= thresholds.artist and title >= thresholds.title
    if not (strong_title_match or ordinary_match):
        return None

    combined = artist * thresholds.artist_weight + title * thresholds.title_weight
    return MatchScore(artist=artist, title=title, combined=combined)


def blocking_keys(*forms: NameForms) -> frozenset[str]:
    """Character n-grams used to avoid comparing every name against every other.

    Two names similar enough to pass :class:`MatchThresholds` overlap by well
    over ``_BLOCKING_GRAM_SIZE`` characters, so restricting comparisons to names
    that share at least one gram turns a quadratic scan into a near-linear one
    without losing matches.
    """
    grams: set[str] = set()
    for form in forms:
        for variant in (form.normalized, form.romaji):
            compact = variant.replace(" ", "")
            if len(compact) < _BLOCKING_GRAM_SIZE:
                if compact:
                    grams.add(compact)
                continue
            grams.update(
                compact[i : i + _BLOCKING_GRAM_SIZE]
                for i in range(len(compact) - _BLOCKING_GRAM_SIZE + 1)
            )
    return frozenset(grams)


def candidate_pairs(keyed: Iterable[tuple[int, frozenset[str]]]) -> Iterator[tuple[int, int]]:
    """Yield index pairs sharing at least one blocking key, each pair once."""
    by_gram: dict[str, list[int]] = {}
    for index, grams in keyed:
        for gram in grams:
            by_gram.setdefault(gram, []).append(index)

    seen: set[tuple[int, int]] = set()
    for indices in by_gram.values():
        for position, left in enumerate(indices):
            for right in indices[position + 1 :]:
                pair = (left, right) if left < right else (right, left)
                if pair[0] != pair[1] and pair not in seen:
                    seen.add(pair)
                    yield pair
