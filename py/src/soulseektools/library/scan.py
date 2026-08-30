"""Walking a music library on disk.

The scripts this replaces used a two-level ``listdir``, which silently ignored
anything filed under ``Artist/Album/track.flac``. The walk here is recursive and
the extension set is configurable, so what counts as "in the library" is a
setting rather than a literal repeated in a dozen places.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from soulseektools.library.tags import TAGGABLE_EXTENSIONS

if TYPE_CHECKING:
    from collections.abc import Collection, Iterator
    from pathlib import Path

AUDIO_EXTENSIONS = frozenset(
    TAGGABLE_EXTENSIONS | {".wav", ".aiff", ".aif", ".wv", ".ape", ".alac"}
)
"""Every container recognised as part of a library, taggable or not."""

LOSSLESS_EXTENSIONS = frozenset({".flac", ".wav", ".aiff", ".aif", ".ape", ".wv", ".alac", ".m4a"})
"""Containers that can hold lossless audio, and so are worth verifying."""


def iter_audio_files(root: Path, extensions: Collection[str] = AUDIO_EXTENSIONS) -> Iterator[Path]:
    """Yield every audio file under ``root``, sorted for reproducible output."""
    wanted = {extension.lower() for extension in extensions}
    for path in sorted(root.rglob("*")):
        if path.is_file() and path.suffix.lower() in wanted:
            yield path


def folder_artist(path: Path, root: Path) -> str:
    """The artist implied by a file's position in the library.

    Libraries are laid out as ``<root>/<artist>/...``, so the first component
    below the root names the artist. A file sitting directly in the root has no
    implied artist.
    """
    try:
        relative = path.relative_to(root)
    except ValueError:
        return ""
    return relative.parts[0] if len(relative.parts) > 1 else ""
