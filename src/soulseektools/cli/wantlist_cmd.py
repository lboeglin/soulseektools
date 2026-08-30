"""``stk wantlist`` -- work out what a streaming export contains that the library does not."""

from __future__ import annotations

import csv
import json
from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from soulseektools import wantlist
from soulseektools.cli.context import console, context, fail
from soulseektools.errors import SoulseekToolsError

app = typer.Typer(help="Compare a streaming export against the library.", no_args_is_help=True)


@app.command("diff")
def diff_command(
    ctx: typer.Context,
    export: Annotated[Path, typer.Argument(help="CSV exported from a streaming service.")],
    queue: Annotated[
        bool,
        typer.Option("--queue/--no-queue", help="Add the missing tracks to the review queue."),
    ] = True,
    missing_csv: Annotated[
        Path | None,
        typer.Option("--missing", help="Also write the missing tracks here."),
    ] = None,
    owned_csv: Annotated[
        Path | None,
        typer.Option("--owned", help="Write the matched tracks and their scores here."),
    ] = None,
) -> None:
    """List the tracks in ``export`` that the library does not already hold.

    An uncertain match is reported as missing rather than owned: a duplicate
    download costs a few megabytes, a track dropped by mistake is simply gone.
    Pass ``--owned`` to review what was matched and how confidently.
    """
    app_context = context(ctx)

    try:
        tracks = wantlist.read_export(export)
    except SoulseekToolsError as error:
        fail(str(error))
        return

    with app_context.open_index() as index:
        entries = list(index.entries())

    result = wantlist.diff(tracks, entries, app_context.settings.matching.thresholds())

    queued = 0
    if queue and result.missing:
        with app_context.open_review() as store:
            queued = store.add(result.missing)

    if missing_csv is not None:
        _write_missing(missing_csv, result)
    if owned_csv is not None:
        _write_owned(owned_csv, result)

    if app_context.as_json:
        console.print_json(
            json.dumps(
                {
                    "total": result.total,
                    "missing": [
                        {"artist": track.artist, "title": track.title} for track in result.missing
                    ],
                    "owned": [
                        {
                            "artist": owned.track.artist,
                            "title": owned.track.title,
                            "path": str(owned.entry.path),
                            "score": round(owned.score, 3),
                        }
                        for owned in result.owned
                    ],
                    "queued": queued,
                }
            )
        )
        return

    if not entries:
        console.print("[yellow]The library index is empty[/] -- run 'stk library scan' first.")

    if result.owned:
        table = Table("Score", "Wanted", "Matched", box=None, pad_edge=False)
        for owned in sorted(result.owned, key=lambda item: item.score)[:20]:
            table.add_row(
                f"{owned.score:.2f}",
                f"{owned.track.artist} - {owned.track.title}",
                str(owned.entry.path),
            )
        console.print(table)
        console.print(
            "[dim]Least confident matches shown first -- check these before trusting them.[/]\n"
        )

    console.print(
        f"{len(result.missing)} of {result.total} track(s) missing, {len(result.owned)} owned"
    )
    if queued:
        console.print(f"Queued {queued} new track(s) for searching")


def _write_missing(destination: Path, result: wantlist.WantlistDiff) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = list(result.missing[0].source_row) if result.missing else ["artist", "title"]
    with destination.open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for track in result.missing:
            writer.writerow(track.source_row or {"artist": track.artist, "title": track.title})


def _write_owned(destination: Path, result: wantlist.WantlistDiff) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=["artist", "title", "score", "matched_path"])
        writer.writeheader()
        for owned in sorted(result.owned, key=lambda item: item.score):
            writer.writerow(
                {
                    "artist": owned.track.artist,
                    "title": owned.track.title,
                    "score": round(owned.score, 3),
                    "matched_path": str(owned.entry.path),
                }
            )
