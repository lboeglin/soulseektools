"""``stk review`` -- inspect and decide on the candidates found for each track."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from soulseektools.cli.context import console, context, fail
from soulseektools.models import TrackStatus
from soulseektools.review.legacy_csv import import_csv

app = typer.Typer(help="Review and approve download candidates.", no_args_is_help=True)

_STATUS_STYLE = {
    TrackStatus.PENDING: "dim",
    TrackStatus.NO_CANDIDATES: "yellow",
    TrackStatus.AWAITING_REVIEW: "cyan",
    TrackStatus.APPROVED: "green",
    TrackStatus.REJECTED: "red",
    TrackStatus.QUEUED: "blue",
    TrackStatus.DOWNLOADED: "green",
    TrackStatus.FAILED: "red",
}


@app.command("list")
def list_tracks(
    ctx: typer.Context,
    status: Annotated[
        TrackStatus | None,
        typer.Option("--status", help="Show only tracks in this state."),
    ] = None,
    limit: Annotated[int, typer.Option("--limit", help="Show at most this many.")] = 50,
    candidates: Annotated[
        bool,
        typer.Option("--candidates", help="Show each track's candidates too."),
    ] = False,
) -> None:
    """Show what is in the review queue."""
    app_context = context(ctx)
    with app_context.open_review() as store:
        tracks = store.tracks(*([status] if status else []), limit=limit)
        counts = store.counts()

    if app_context.as_json:
        console.print_json(
            json.dumps(
                {
                    "counts": {str(key): value for key, value in counts.items()},
                    "tracks": [
                        {
                            "id": entry.id,
                            "artist": entry.track.artist,
                            "title": entry.track.title,
                            "status": str(entry.status),
                            "detail": entry.detail,
                            "approved_rank": entry.approved_rank,
                            "candidates": [
                                {
                                    "rank": rank,
                                    "score": scored.score,
                                    "flags": list(scored.flags),
                                    "username": scored.candidate.username,
                                    "remote_path": scored.candidate.remote_path,
                                }
                                for rank, scored in enumerate(entry.candidates, start=1)
                            ],
                        }
                        for entry in tracks
                    ],
                }
            )
        )
        return

    if not tracks:
        console.print("Nothing in the review queue.")
        return

    table = Table("ID", "Status", "Track", "Detail", box=None, pad_edge=False)
    for entry in tracks:
        table.add_row(
            str(entry.id),
            f"[{_STATUS_STYLE[entry.status]}]{entry.status}[/]",
            f"{entry.track.artist} - {entry.track.title}",
            entry.detail,
        )
    console.print(table)

    if candidates:
        for entry in tracks:
            if not entry.candidates:
                continue
            console.print(f"\n[bold]{entry.id}[/] {entry.track.artist} - {entry.track.title}")
            for rank, scored in enumerate(entry.candidates, start=1):
                marker = "*" if rank == entry.approved_rank else " "
                console.print(f" {marker}{rank}. {scored.describe()}")
                console.print(f"    [dim]{scored.candidate.remote_path}[/]")

    console.print("\n" + ", ".join(f"{value} {key}" for key, value in sorted(counts.items())))


@app.command("approve")
def approve(
    ctx: typer.Context,
    track_id: Annotated[int, typer.Argument(help="Track to approve.")],
    rank: Annotated[int, typer.Argument(help="Which candidate to take.")] = 1,
) -> None:
    """Choose one candidate for a track, so the downloader can queue it."""
    app_context = context(ctx)
    with app_context.open_review() as store:
        try:
            store.approve(track_id, rank)
        except KeyError as error:
            fail(str(error).strip("'"))
            return
        entry = store.track(track_id)

    if entry is not None and entry.approved is not None:
        console.print(f"Approved candidate {rank}: {entry.approved.describe()}")


@app.command("reject")
def reject(
    ctx: typer.Context,
    track_id: Annotated[int, typer.Argument(help="Track to reject.")],
    reason: Annotated[str, typer.Option("--reason", help="Why, for the record.")] = "",
) -> None:
    """Mark a track as not worth downloading from any candidate found."""
    with context(ctx).open_review() as store:
        if store.track(track_id) is None:
            fail(f"no track {track_id} in the review queue")
            return
        store.reject(track_id, reason)
    console.print(f"Rejected track {track_id}")


@app.command("reset")
def reset(
    ctx: typer.Context,
    track_id: Annotated[int, typer.Argument(help="Track to search again.")],
) -> None:
    """Discard a track's candidates and return it to the search queue."""
    with context(ctx).open_review() as store:
        if store.track(track_id) is None:
            fail(f"no track {track_id} in the review queue")
            return
        store.reset(track_id)
    console.print(f"Track {track_id} will be searched again")


@app.command("import")
def import_legacy(
    ctx: typer.Context,
    path: Annotated[Path, typer.Argument(help="A soulseek_review.csv from the old plugin.")],
) -> None:
    """Load a review CSV written by the Nicotine+ plugin, approvals included."""
    if not path.is_file():
        fail(f"no such file: {path}")
        return

    with context(ctx).open_review() as store:
        added, approvals = import_csv(path, store)

    console.print(f"Imported {added} new track(s), carrying over {approvals} approval(s)")
