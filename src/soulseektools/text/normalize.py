"""Text normalisation shared by every matcher in the project.

The same track arrives spelled several different ways: a streaming export gives
``DECO*27`` and ``弱虫モンブラン (sasakure.UK Remix)``, the ripped file on disk is
``(06) [DECO-27] 弱虫モンブラン.flac``, and a Soulseek peer offers
``06. DECO_27 - Yowamushi Montblanc.flac``. Normalising away featured-artist
credits, bracketed suffixes, punctuation and case is what makes those three
comparable at all.
"""

from __future__ import annotations

import re
import unicodedata

_FEATURED_CREDIT = re.compile(r"\(feat\.[^)]*\)|feat\.[^,(\[]*", re.IGNORECASE)
_BRACKETED_SUFFIX = re.compile(r"[\(\[][^\)\]]*[\)\]]")
# Underscore is a word character to ``re`` but stands in for a stripped symbol in
# filenames, so "DECO_27" has to fold to the same form as "DECO*27".
_PUNCTUATION = re.compile(r"[^\w\s]|_", re.UNICODE)
_WHITESPACE = re.compile(r"\s+")
_WORD = re.compile(r"\w+", re.UNICODE)
_LEADING_TRACK_NUMBER = re.compile(
    r"^\s*[\(\[]?\d{1,3}(?:[.\-]\d{1,3})?[\)\]]?\s*[-.\u2013\u2014_]?\s*"
)
_SEPARATOR = r"\s*[-\u2013\u2014_:.|]*\s*"
_INVALID_FILENAME_CHARS = re.compile(r'[<>:"/\|?*\x00-\x1f]')


def normalize(text: str | None) -> str:
    """Reduce a name to the lowercase, punctuation-free form used for comparison."""
    folded = unicodedata.normalize("NFKC", text or "")
    folded = _FEATURED_CREDIT.sub(" ", folded)
    folded = _BRACKETED_SUFFIX.sub(" ", folded)
    folded = _PUNCTUATION.sub(" ", folded)
    return _WHITESPACE.sub(" ", folded).strip().lower()


def strip_track_number(name: str) -> str:
    """Drop a leading track number such as ``01 -``, ``(06)`` or ``15.``."""
    stripped = _LEADING_TRACK_NUMBER.sub("", name, count=1)
    return stripped.strip() or name.strip()


def strip_artist_prefix(stem: str, artist: str) -> str:
    """Remove a leading artist name from a filename stem, if present.

    Matching on the artist's tokens rather than on the literal folder name lets a
    ``DECO_27`` folder strip the prefix from ``DECO*27 - Roki``: the two spellings
    differ only in punctuation, and a literal-length slice would cut the title in
    the wrong place whenever the two spellings differ in length.
    """
    name = strip_track_number(unicodedata.normalize("NFKC", stem))
    tokens = _WORD.findall(unicodedata.normalize("NFKC", artist).replace("_", " ").lower())

    # Shorter token runs are tried after the full name so that a "ryo (supercell)"
    # folder strips both "ryo (supercell) - Melt" and "ryo - Melt".
    for count in range(len(tokens), 0, -1):
        pattern = r"[\W_]*".join(re.escape(token) for token in tokens[:count])
        # A trailing bracket belongs to the artist ("ryo (supercell)"), not the title.
        match = re.match(rf"{_SEPARATOR}{pattern}[\)\]\}}]*(?!\w){_SEPARATOR}", name, re.IGNORECASE)
        if match is not None and 0 < match.end() < len(name):
            return name[match.end() :].strip()
    return name


def sanitize_folder_name(name: str, fallback: str = "Unknown Artist") -> str:
    """Make a name safe to use as a directory component on any supported platform."""
    cleaned = _INVALID_FILENAME_CHARS.sub("_", name).strip().rstrip(".")
    return cleaned or fallback
