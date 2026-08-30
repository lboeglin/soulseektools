"""One place where container formats are handled.

The scripts this replaces each grew their own mutagen dispatch, and the copies
drifted: some handled Ogg and some did not, some treated a missing ID3 header as
an error and some as an empty tag. Everything that touches metadata now goes
through this module, so a format is supported everywhere or nowhere.

Reading uses mutagen's "easy" key mapping, which presents ``artist``/``album``/
``title``/``genre`` identically across ID3, Vorbis comments and MP4 atoms.
Artwork has no easy mapping and is handled per container below.
"""

from __future__ import annotations

import base64
from typing import TYPE_CHECKING, Any

import mutagen
from mutagen.flac import FLAC, Picture
from mutagen.id3 import APIC, ID3
from mutagen.mp4 import MP4, MP4Cover
from mutagen.oggopus import OggOpus
from mutagen.oggvorbis import OggVorbis

from soulseektools.errors import TagError, UnsupportedFormatError
from soulseektools.models import Artwork, TrackTags

if TYPE_CHECKING:
    from collections.abc import Iterable, Sequence
    from pathlib import Path

TAGGABLE_EXTENSIONS = frozenset({".flac", ".mp3", ".m4a", ".mp4", ".ogg", ".opus"})
"""Containers whose tags this module can read and write."""

_ARTWORK_MIME_BY_FORMAT = {MP4Cover.FORMAT_JPEG: "image/jpeg", MP4Cover.FORMAT_PNG: "image/png"}
_MP4_COVER_FORMATS = {"image/png": MP4Cover.FORMAT_PNG, "image/jpeg": MP4Cover.FORMAT_JPEG}
_FRONT_COVER = 3


def _first(values: Sequence[str] | None) -> str:
    return str(values[0]) if values else ""


def _open(path: Path, *, easy: bool = False) -> Any:
    try:
        audio = mutagen.File(path, easy=easy)
    except Exception as error:
        raise TagError(f"{path}: {error}") from error
    if audio is None:
        raise UnsupportedFormatError(f"{path}: not a recognised audio file")
    return audio


def read_tags(path: Path) -> TrackTags:
    """Read the metadata fields this project cares about."""
    audio = _open(path, easy=True)
    tags = audio.tags or {}
    raw_track = _first(tags.get("tracknumber"))
    leading_digits = raw_track.split("/")[0].strip()
    return TrackTags(
        artist=_first(tags.get("artist")),
        album_artist=_first(tags.get("albumartist")),
        album=_first(tags.get("album")),
        title=_first(tags.get("title")),
        genres=tuple(str(value) for value in tags.get("genre", ()) if str(value)),
        track_number=int(leading_digits) if leading_digits.isdigit() else None,
    )


def read_duration(path: Path) -> float | None:
    """Return the track length in seconds, straight from the container header.

    The header already carries this, so nothing here needs to spawn ffprobe once
    per file the way the original duplicate scan did.
    """
    audio = _open(path)
    length = getattr(audio.info, "length", None)
    return float(length) if length else None


def write_genres(path: Path, genres: Iterable[str]) -> None:
    """Replace the genre tag, creating a tag block if the file has none."""
    values = [genre for genre in genres if genre]
    audio = _open(path, easy=True)
    if audio.tags is None:
        audio.add_tags()
    if values:
        audio.tags["genre"] = values
    else:
        audio.tags.pop("genre", None)
    audio.save()


def read_artwork(path: Path) -> Artwork | None:
    """Return the embedded front cover, or ``None`` when the file has none."""
    suffix = path.suffix.lower()
    if suffix == ".flac":
        pictures = FLAC(path).pictures
        front = next((p for p in pictures if p.type == _FRONT_COVER), None) or next(
            iter(pictures), None
        )
        return Artwork(bytes(front.data), front.mime or "image/jpeg") if front else None

    if suffix == ".mp3":
        frames = _id3(path)
        if frames is None:
            return None
        covers = frames.getall("APIC")
        front = next((c for c in covers if c.type == _FRONT_COVER), None) or next(
            iter(covers), None
        )
        return Artwork(bytes(front.data), front.mime or "image/jpeg") if front else None

    if suffix in {".m4a", ".mp4"}:
        covers = MP4(path).get("covr") or []
        if not covers:
            return None
        cover = covers[0]
        image_format = getattr(cover, "imageformat", MP4Cover.FORMAT_JPEG)
        return Artwork(bytes(cover), _ARTWORK_MIME_BY_FORMAT.get(image_format, "image/jpeg"))

    if suffix in {".ogg", ".opus"}:
        return _read_ogg_artwork(path)

    raise UnsupportedFormatError(f"{path}: artwork is not supported for '{suffix}'")


def artwork_size(path: Path) -> int:
    """Size in bytes of the embedded cover, or ``0`` when there is none."""
    try:
        artwork = read_artwork(path)
    except (TagError, UnsupportedFormatError):
        return 0
    return len(artwork) if artwork else 0


def write_artwork(path: Path, artwork: Artwork) -> None:
    """Replace the embedded cover, removing any images already present."""
    suffix = path.suffix.lower()
    if suffix == ".flac":
        flac = FLAC(path)
        flac.clear_pictures()
        flac.add_picture(_flac_picture(artwork))
        flac.save()
        return

    if suffix == ".mp3":
        frames = _id3(path) or ID3()
        frames.delall("APIC")
        frames.add(
            APIC(encoding=3, mime=artwork.mime, type=_FRONT_COVER, desc="Cover", data=artwork.data)
        )
        frames.save(path)
        return

    if suffix in {".m4a", ".mp4"}:
        mp4 = MP4(path)
        image_format = _MP4_COVER_FORMATS.get(artwork.mime, MP4Cover.FORMAT_JPEG)
        mp4["covr"] = [MP4Cover(artwork.data, imageformat=image_format)]
        mp4.save()
        return

    if suffix in {".ogg", ".opus"}:
        _write_ogg_artwork(path, artwork)
        return

    raise UnsupportedFormatError(f"{path}: artwork is not supported for '{suffix}'")


def _flac_picture(artwork: Artwork) -> Picture:
    picture = Picture()
    picture.type = _FRONT_COVER
    picture.mime = artwork.mime
    picture.data = artwork.data
    return picture


def _id3(path: Path) -> ID3 | None:
    try:
        return ID3(path)
    except Exception:
        # A file with no ID3 header at all is untagged, not broken.
        return None


def _ogg(path: Path) -> OggVorbis | OggOpus:
    return OggOpus(path) if path.suffix.lower() == ".opus" else OggVorbis(path)


def _read_ogg_artwork(path: Path) -> Artwork | None:
    audio = _ogg(path)
    encoded = audio.get("metadata_block_picture")
    if not encoded:
        return None
    picture = Picture(base64.b64decode(encoded[0]))
    return Artwork(bytes(picture.data), picture.mime or "image/jpeg")


def _write_ogg_artwork(path: Path, artwork: Artwork) -> None:
    audio = _ogg(path)
    encoded = base64.b64encode(_flac_picture(artwork).write()).decode("ascii")
    audio["metadata_block_picture"] = [encoded]
    audio.save()
