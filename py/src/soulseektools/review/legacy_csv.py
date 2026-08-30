"""Importing the review CSV the Nicotine+ plugin used.

A migration path, so a session interrupted mid-batch under the old plugin can be
picked up here rather than re-searched from scratch. The CSV held three
candidates per row in columns named ``cand1_user``, ``cand2_path`` and so on,
plus an ``approve`` column holding ``1``, ``2`` or ``3``.
"""

from __future__ import annotations

import csv
from typing import TYPE_CHECKING

from soulseektools.models import Candidate, ScoredCandidate, Track, TrackStatus

if TYPE_CHECKING:
    from pathlib import Path

    from soulseektools.review.store import ReviewStore

CANDIDATES_PER_ROW = 3


def import_csv(path: Path, store: ReviewStore) -> tuple[int, int]:
    """Load a legacy review CSV into the store.

    Returns the number of tracks added and the number whose approval carried
    over. Tracks already in the store are left untouched.
    """
    with path.open(encoding="utf-8-sig", newline="") as handle:
        rows = list(csv.DictReader(handle))

    added = store.add(
        Track(artist=(row.get("artist") or "").strip(), title=(row.get("title") or "").strip())
        for row in rows
        if (row.get("artist") or row.get("title"))
    )

    by_query = {entry.track.query: entry.id for entry in store.tracks()}
    approvals = 0

    for row in rows:
        query = f"{(row.get('artist') or '').strip()} - {(row.get('title') or '').strip()}"
        track_id = by_query.get(query)
        if track_id is None:
            continue

        candidates = _candidates(row)
        if not candidates:
            store.mark(track_id, TrackStatus.NO_CANDIDATES, "no candidates in imported CSV")
            continue

        store.set_candidates(track_id, candidates)
        approve = (row.get("approve") or "").strip()
        if approve.isdigit() and 1 <= int(approve) <= len(candidates):
            store.approve(track_id, int(approve))
            approvals += 1

    return added, approvals


def _candidates(row: dict[str, str]) -> list[ScoredCandidate]:
    candidates = []
    for position in range(1, CANDIDATES_PER_ROW + 1):
        prefix = f"cand{position}_"
        username = (row.get(f"{prefix}user") or "").strip()
        remote_path = (row.get(f"{prefix}path") or "").strip()
        if not username or not remote_path:
            continue
        candidates.append(
            ScoredCandidate(
                candidate=Candidate(
                    username=username,
                    remote_path=remote_path,
                    size=_int(row.get(f"{prefix}size")) or 0,
                    bitrate=_int(row.get(f"{prefix}bitrate")),
                    sample_rate=_int(row.get(f"{prefix}samplerate")),
                    bit_depth=_int(row.get(f"{prefix}bitdepth")),
                    is_vbr=bool(_int(row.get(f"{prefix}vbr"))),
                ),
                score=_int(row.get(f"{prefix}score")) or 0,
                title_ratio=0.0,
                flags=("imported",),
            )
        )
    return candidates


def _int(raw: str | None) -> int | None:
    text = (raw or "").strip()
    try:
        return int(float(text)) if text else None
    except ValueError:
        return None
