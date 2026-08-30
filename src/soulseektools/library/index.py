"""SQLite cache of the library, so commands stop rescanning everything.

Reading tags, artwork size and duration for a few thousand files takes long
enough that the original scripts each paid it again from scratch. Rows here are
keyed by path and invalidated by ``(size, mtime)``, so a rescan only touches
files that actually changed, and the expensive derived values -- normalised and
romanised name forms -- are stored rather than recomputed.
"""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Self

from soulseektools.audio.fingerprint import Fingerprint, FingerprintKind
from soulseektools.errors import TagError, UnsupportedFormatError
from soulseektools.library import tags as tag_facade
from soulseektools.library.scan import AUDIO_EXTENSIONS, folder_artist, iter_audio_files
from soulseektools.matching import NameForms
from soulseektools.models import LibraryEntry, TrackTags
from soulseektools.text.normalize import strip_artist_prefix

if TYPE_CHECKING:
    from collections.abc import Callable, Collection, Iterator
    from types import TracebackType

SCHEMA_VERSION = 2

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    path               TEXT PRIMARY KEY,
    size               INTEGER NOT NULL,
    mtime              REAL    NOT NULL,
    duration_seconds   REAL,
    artwork_bytes      INTEGER NOT NULL DEFAULT 0,
    folder_artist      TEXT    NOT NULL DEFAULT '',
    tag_artist         TEXT    NOT NULL DEFAULT '',
    tag_album_artist   TEXT    NOT NULL DEFAULT '',
    tag_album          TEXT    NOT NULL DEFAULT '',
    tag_title          TEXT    NOT NULL DEFAULT '',
    tag_genres         TEXT    NOT NULL DEFAULT '[]',
    tag_track_number   INTEGER,
    artist_raw         TEXT    NOT NULL DEFAULT '',
    artist_normalized  TEXT    NOT NULL DEFAULT '',
    artist_romaji      TEXT    NOT NULL DEFAULT '',
    title_raw          TEXT    NOT NULL DEFAULT '',
    title_normalized   TEXT    NOT NULL DEFAULT '',
    title_romaji       TEXT    NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS files_artist_normalized ON files (artist_normalized);
CREATE INDEX IF NOT EXISTS files_title_normalized  ON files (title_normalized);

CREATE TABLE IF NOT EXISTS fingerprints (
    path   TEXT PRIMARY KEY,
    mtime  REAL NOT NULL,
    kind   TEXT NOT NULL,
    values_json TEXT NOT NULL,
    duration_seconds REAL
);
"""

_COLUMNS = (
    "path",
    "size",
    "mtime",
    "duration_seconds",
    "artwork_bytes",
    "folder_artist",
    "tag_artist",
    "tag_album_artist",
    "tag_album",
    "tag_title",
    "tag_genres",
    "tag_track_number",
    "artist_raw",
    "artist_normalized",
    "artist_romaji",
    "title_raw",
    "title_normalized",
    "title_romaji",
)
_SELECT_COLUMNS = ", ".join(_COLUMNS)
_INSERT = (
    f"INSERT OR REPLACE INTO files ({_SELECT_COLUMNS}) VALUES ({', '.join(['?'] * len(_COLUMNS))})"
)


@dataclass(frozen=True, slots=True)
class RefreshResult:
    """What a refresh changed, for reporting and for asserting incrementality."""

    added: int = 0
    updated: int = 0
    unchanged: int = 0
    removed: int = 0
    failed: tuple[tuple[Path, str], ...] = ()

    @property
    def scanned(self) -> int:
        return self.added + self.updated + self.unchanged


def describe(path: Path, root: Path) -> LibraryEntry:
    """Read everything the index stores about one file.

    Tags win over the directory layout, which wins over the filename: a large
    part of any Soulseek-sourced library arrives with no tags at all, and the
    folder it landed in is then the only artist information there is.
    """
    stat = path.stat()
    artist_folder = folder_artist(path, root)

    try:
        tags = tag_facade.read_tags(path)
    except (TagError, UnsupportedFormatError):
        tags = TrackTags()
    try:
        duration = tag_facade.read_duration(path)
    except (TagError, UnsupportedFormatError):
        duration = None

    artist = tags.artist or tags.album_artist or artist_folder
    title = tags.title or strip_artist_prefix(path.stem, artist_folder or artist)

    return LibraryEntry(
        path=path,
        size=stat.st_size,
        mtime=stat.st_mtime,
        folder_artist=artist_folder,
        tags=tags,
        artist=NameForms.of(artist),
        title=NameForms.of(title),
        duration_seconds=duration,
        artwork_bytes=tag_facade.artwork_size(path),
    )


class LibraryIndex:
    """Read/write access to the cached view of a library."""

    def __init__(self, database: Path | str) -> None:
        self.database = Path(database)
        if self.database.parent != Path():
            self.database.parent.mkdir(parents=True, exist_ok=True)
        self._connection = sqlite3.connect(self.database)
        self._connection.row_factory = sqlite3.Row
        self._connection.executescript(_SCHEMA)
        self._connection.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)",
            (str(SCHEMA_VERSION),),
        )
        self._connection.commit()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    def close(self) -> None:
        self._connection.close()

    def refresh(
        self,
        root: Path,
        extensions: Collection[str] = AUDIO_EXTENSIONS,
        *,
        full: bool = False,
        on_file: Callable[[Path], None] | None = None,
    ) -> RefreshResult:
        """Bring the index in line with the files under ``root``.

        Only files whose size or modification time changed are re-read, unless
        ``full`` forces every file to be reprocessed.
        """
        known = {
            row["path"]: (row["size"], row["mtime"])
            for row in self._connection.execute("SELECT path, size, mtime FROM files")
        }

        added = updated = unchanged = 0
        failed: list[tuple[Path, str]] = []
        seen: set[str] = set()

        for path in iter_audio_files(root, extensions):
            key = str(path)
            seen.add(key)
            if on_file is not None:
                on_file(path)

            stat = path.stat()
            cached = known.get(key)
            if not full and cached == (stat.st_size, stat.st_mtime):
                unchanged += 1
                continue

            try:
                entry = describe(path, root)
            except OSError as error:
                failed.append((path, str(error)))
                continue

            self._upsert(entry)
            if cached is None:
                added += 1
            else:
                updated += 1

        stale = set(known) - seen
        if stale:
            self._connection.executemany(
                "DELETE FROM files WHERE path = ?", ((path,) for path in stale)
            )
        self._connection.commit()

        return RefreshResult(
            added=added,
            updated=updated,
            unchanged=unchanged,
            removed=len(stale),
            failed=tuple(failed),
        )

    def entries(self) -> Iterator[LibraryEntry]:
        """Every indexed file, ordered by path."""
        query = f"SELECT {_SELECT_COLUMNS} FROM files ORDER BY path"
        with closing(self._connection.execute(query)) as cursor:
            for row in cursor:
                yield _row_to_entry(row)

    def entry(self, path: Path | str) -> LibraryEntry | None:
        query = f"SELECT {_SELECT_COLUMNS} FROM files WHERE path = ?"
        row = self._connection.execute(query, (str(path),)).fetchone()
        return _row_to_entry(row) if row else None

    def count(self) -> int:
        return int(self._connection.execute("SELECT COUNT(*) FROM files").fetchone()[0])

    def artist_counts(self) -> dict[str, int]:
        """Number of indexed files per artist folder, largest first."""
        rows = self._connection.execute(
            "SELECT folder_artist, COUNT(*) AS total FROM files "
            "GROUP BY folder_artist ORDER BY total DESC, folder_artist"
        )
        return {row["folder_artist"]: row["total"] for row in rows}

    def fingerprint(self, path: Path, mtime: float) -> Fingerprint | None:
        """A cached fingerprint, or ``None`` if absent or computed before an edit."""
        row = self._connection.execute(
            "SELECT kind, values_json, duration_seconds FROM fingerprints "
            "WHERE path = ? AND mtime = ?",
            (str(path), mtime),
        ).fetchone()
        if row is None:
            return None
        kind = FingerprintKind(row["kind"])
        raw = json.loads(row["values_json"])
        values = (
            tuple(int(v) for v in raw)
            if kind is FingerprintKind.CHROMAPRINT
            else tuple(float(v) for v in raw)
        )
        return Fingerprint(kind=kind, values=values, duration_seconds=row["duration_seconds"])

    def store_fingerprint(self, path: Path, mtime: float, value: Fingerprint) -> None:
        self._connection.execute(
            "INSERT OR REPLACE INTO fingerprints "
            "(path, mtime, kind, values_json, duration_seconds) VALUES (?, ?, ?, ?, ?)",
            (
                str(path),
                mtime,
                str(value.kind),
                json.dumps(list(value.values)),
                value.duration_seconds,
            ),
        )
        self._connection.commit()

    def _upsert(self, entry: LibraryEntry) -> None:
        self._connection.execute(
            _INSERT,
            (
                str(entry.path),
                entry.size,
                entry.mtime,
                entry.duration_seconds,
                entry.artwork_bytes,
                entry.folder_artist,
                entry.tags.artist,
                entry.tags.album_artist,
                entry.tags.album,
                entry.tags.title,
                json.dumps(list(entry.tags.genres)),
                entry.tags.track_number,
                entry.artist.raw,
                entry.artist.normalized,
                entry.artist.romaji,
                entry.title.raw,
                entry.title.normalized,
                entry.title.romaji,
            ),
        )


def _row_to_entry(row: sqlite3.Row) -> LibraryEntry:
    return LibraryEntry(
        path=Path(row["path"]),
        size=row["size"],
        mtime=row["mtime"],
        folder_artist=row["folder_artist"],
        tags=TrackTags(
            artist=row["tag_artist"],
            album_artist=row["tag_album_artist"],
            album=row["tag_album"],
            title=row["tag_title"],
            genres=tuple(json.loads(row["tag_genres"])),
            track_number=row["tag_track_number"],
        ),
        artist=NameForms(
            raw=row["artist_raw"],
            normalized=row["artist_normalized"],
            romaji=row["artist_romaji"],
        ),
        title=NameForms(
            raw=row["title_raw"],
            normalized=row["title_normalized"],
            romaji=row["title_romaji"],
        ),
        duration_seconds=row["duration_seconds"],
        artwork_bytes=row["artwork_bytes"],
    )
