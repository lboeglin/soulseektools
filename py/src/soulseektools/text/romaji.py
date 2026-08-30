"""Romaji transliteration, used to compare Japanese titles with romanised filenames.

A streaming export lists ``千本桜`` while the file on disk is ``Senbonzakura.flac``;
no amount of string similarity relates those two without transliterating one of
them first. Artist readings are less trustworthy than title readings -- aliases
frequently use non-standard readings -- which is why callers weight an artist
match lower than a title match.
"""

from __future__ import annotations

import re
from functools import lru_cache
from typing import TYPE_CHECKING, Any

from soulseektools.text.normalize import normalize

if TYPE_CHECKING:
    from collections.abc import Iterable

_JAPANESE_CHARACTER = re.compile(
    r"[\u3040-\u309f\u30a0-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\uff66-\uff9f]"
)

_converter: Any | None = None


def contains_japanese(text: str) -> bool:
    """Report whether transliteration could change anything."""
    return _JAPANESE_CHARACTER.search(text) is not None


def _kakasi() -> Any:
    # pykakasi builds a sizeable dictionary on construction, so it is created on
    # first use rather than at import: most commands never touch a Japanese name.
    global _converter
    if _converter is None:
        import pykakasi

        _converter = pykakasi.kakasi()
    return _converter


@lru_cache(maxsize=16384)
def to_romaji(text: str) -> str:
    """Return the normalised Hepburn romanisation of ``text``.

    Text without Japanese characters is returned normalised and otherwise
    untouched, so this is safe to call on every name unconditionally.
    """
    if not contains_japanese(text):
        return normalize(text)
    parts: Iterable[dict[str, str]] = _kakasi().convert(text)
    return normalize("".join(part["hepburn"] for part in parts))
