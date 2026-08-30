"""The review queue: what is wanted, what was found, and what was approved.

This replaces the review CSV the Nicotine+ plugin polled every thirty seconds.
A CSV made the approval step a hand-edit and gave no way to record a track's
progress beyond a free-text column; a table makes each track's state explicit
and lets the TUI, the CLI and the downloader all read the same thing.
"""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING, Self

from soulseektools.models import Candidate, ScoredCandidate, Track, TrackStatus

if TYPE_CHECKING:
    from collections.abc import Iterable, Sequence
    from types import TracebackType

SCHEMA_VERSION = 1

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tracks (
    id               INTEGER PRIMARY KEY,
    query            TEXT NOT NULL UNIQUE,
    artist           TEXT NOT NULL,
    title            TEXT NOT NULL,
    album            TEXT NOT NULL DEFAULT '',
    duration_seconds REAL,
    status           TEXT NOT NULL DEFAULT 'pending',
    detail           TEXT NOT NULL DEFAULT '',
    source_row       TEXT NOT NULL DEFAULT '{}',
    added_at         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS candidates (
    id               INTEGER PRIMARY KEY,
    track_id         INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    rank             INTEGER NOT NULL,
    score            INTEGER NOT NULL,
    title_ratio      REAL NOT NULL DEFAULT 0,
    flags            TEXT NOT NULL DEFAULT '[]',
    username         TEXT NOT NULL,
    remote_path      TEXT NOT NULL,
    size             INTEGER NOT NULL DEFAULT 0,
    bitrate          INTEGER,
    sample_rate      INTEGER,
    bit_depth        INTEGER,
    is_vbr           INTEGER NOT NULL DEFAULT 0,
    free_slots       INTEGER NOT NULL DEFAULT 0,
    queue_length     INTEGER NOT NULL DEFAULT 0,
    duration_seconds REAL,
    approved         INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS candidates_track ON candidates (track_id, rank);
CREATE INDEX IF NOT EXISTS tracks_status ON tracks (status);
"""


@dataclass(frozen=True, slots=True)
class ReviewTrack:
    """A wanted track and everything found for it."""

    id: int
    track: Track
    status: TrackStatus
    detail: str
    candidates: tuple[ScoredCandidate, ...] = ()
    approved_rank: int | None = None

    @property
    def approved(self) -> ScoredCandidate | None:
        if self.approved_rank is None:
            return None
        position = self.approved_rank - 1
        return self.candidates[position] if 0 <= position < len(self.candidates) else None


class ReviewStore:
    """Read/write access to the review queue."""

    def __init__(self, database: Path | str) -> None:
        self.database = Path(database)
        if self.database.parent != Path():
            self.database.parent.mkdir(parents=True, exist_ok=True)
        self._connection = sqlite3.connect(self.database)
        self._connection.row_factory = sqlite3.Row
        self._connection.execute("PRAGMA foreign_keys = ON")
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

    def add(self, tracks: Iterable[Track]) -> int:
        """Queue tracks for searching, skipping any already present."""
        added = 0
        now = datetime.now(UTC).isoformat()
        for track in tracks:
            cursor = self._connection.execute(
                "INSERT OR IGNORE INTO tracks "
                "(query, artist, title, album, duration_seconds, status, source_row, added_at) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    track.query,
                    track.artist,
                    track.title,
                    track.album,
                    track.duration_seconds,
                    str(TrackStatus.PENDING),
                    json.dumps(track.source_row),
                    now,
                ),
            )
            added += cursor.rowcount
        self._connection.commit()
        return added

    def tracks(self, *statuses: TrackStatus, limit: int | None = None) -> list[ReviewTrack]:
        """Tracks in the given states, oldest first; all of them when none is given."""
        query = "SELECT * FROM tracks"
        parameters: list[object] = []
        if statuses:
            placeholders = ", ".join("?" * len(statuses))
            query += f" WHERE status IN ({placeholders})"
            parameters.extend(str(status) for status in statuses)
        query += " ORDER BY id"
        if limit is not None:
            query += " LIMIT ?"
            parameters.append(limit)

        with closing(self._connection.execute(query, parameters)) as cursor:
            rows = cursor.fetchall()
        return [self._to_review_track(row) for row in rows]

    def track(self, track_id: int) -> ReviewTrack | None:
        row = self._connection.execute("SELECT * FROM tracks WHERE id = ?", (track_id,)).fetchone()
        return self._to_review_track(row) if row else None

    def counts(self) -> dict[TrackStatus, int]:
        rows = self._connection.execute(
            "SELECT status, COUNT(*) AS total FROM tracks GROUP BY status"
        )
        return {TrackStatus(row["status"]): row["total"] for row in rows}

    def set_candidates(self, track_id: int, candidates: Sequence[ScoredCandidate]) -> None:
        """Record what a search found, replacing anything found before."""
        self._connection.execute("DELETE FROM candidates WHERE track_id = ?", (track_id,))
        for rank, scored in enumerate(candidates, start=1):
            self._connection.execute(
                "INSERT INTO candidates (track_id, rank, score, title_ratio, flags, username, "
                "remote_path, size, bitrate, sample_rate, bit_depth, is_vbr, free_slots, "
                "queue_length, duration_seconds) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    track_id,
                    rank,
                    scored.score,
                    scored.title_ratio,
                    json.dumps(list(scored.flags)),
                    scored.candidate.username,
                    scored.candidate.remote_path,
                    scored.candidate.size,
                    scored.candidate.bitrate,
                    scored.candidate.sample_rate,
                    scored.candidate.bit_depth,
                    int(scored.candidate.is_vbr),
                    int(scored.candidate.free_upload_slots),
                    scored.candidate.queue_length,
                    scored.candidate.duration_seconds,
                ),
            )
        status = TrackStatus.AWAITING_REVIEW if candidates else TrackStatus.NO_CANDIDATES
        self._set_status(track_id, status, "" if candidates else "no usable search results")
        self._connection.commit()

    def approve(self, track_id: int, rank: int) -> None:
        """Choose one candidate, clearing any previous choice."""
        self._connection.execute(
            "UPDATE candidates SET approved = 0 WHERE track_id = ?", (track_id,)
        )
        updated = self._connection.execute(
            "UPDATE candidates SET approved = 1 WHERE track_id = ? AND rank = ?",
            (track_id, rank),
        ).rowcount
        if not updated:
            self._connection.rollback()
            raise KeyError(f"track {track_id} has no candidate {rank}")
        self._set_status(track_id, TrackStatus.APPROVED, f"candidate {rank}")
        self._connection.commit()

    def reject(self, track_id: int, reason: str = "") -> None:
        self._connection.execute(
            "UPDATE candidates SET approved = 0 WHERE track_id = ?", (track_id,)
        )
        self._set_status(track_id, TrackStatus.REJECTED, reason)
        self._connection.commit()

    def mark(self, track_id: int, status: TrackStatus, detail: str = "") -> None:
        self._set_status(track_id, status, detail)
        self._connection.commit()

    def reset(self, track_id: int) -> None:
        """Return a track to the queue so it is searched again."""
        self._connection.execute("DELETE FROM candidates WHERE track_id = ?", (track_id,))
        self._set_status(track_id, TrackStatus.PENDING, "")
        self._connection.commit()

    def _set_status(self, track_id: int, status: TrackStatus, detail: str) -> None:
        self._connection.execute(
            "UPDATE tracks SET status = ?, detail = ? WHERE id = ?",
            (str(status), detail, track_id),
        )

    def _to_review_track(self, row: sqlite3.Row) -> ReviewTrack:
        candidate_rows = self._connection.execute(
            "SELECT * FROM candidates WHERE track_id = ? ORDER BY rank", (row["id"],)
        ).fetchall()
        approved = next((c["rank"] for c in candidate_rows if c["approved"]), None)

        return ReviewTrack(
            id=row["id"],
            track=Track(
                artist=row["artist"],
                title=row["title"],
                album=row["album"],
                duration_seconds=row["duration_seconds"],
                source_row=json.loads(row["source_row"]),
            ),
            status=TrackStatus(row["status"]),
            detail=row["detail"],
            candidates=tuple(_to_scored(candidate) for candidate in candidate_rows),
            approved_rank=approved,
        )


def _to_scored(row: sqlite3.Row) -> ScoredCandidate:
    return ScoredCandidate(
        candidate=Candidate(
            username=row["username"],
            remote_path=row["remote_path"],
            size=row["size"],
            bitrate=row["bitrate"],
            sample_rate=row["sample_rate"],
            bit_depth=row["bit_depth"],
            is_vbr=bool(row["is_vbr"]),
            free_upload_slots=bool(row["free_slots"]),
            queue_length=row["queue_length"],
            duration_seconds=row["duration_seconds"],
        ),
        score=row["score"],
        title_ratio=row["title_ratio"],
        flags=tuple(json.loads(row["flags"])),
    )
