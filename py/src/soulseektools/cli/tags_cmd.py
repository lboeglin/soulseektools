"""``stk tags`` -- dump the library's metadata for eyeballing or grepping."""

from __future__ import annotations

import csv
import json
import sys
from pathlib import Path
from typing import Annotated

import typer

from soulseektools.cli.context import console, context

app = typer.Typer(help="Inspect tags across the library.", no_args_is_help=True)

DUMP_FIELDS = ("path", "folder_artist", "artist", "album", "title", "genres", "duration_seconds")


@app.command("dump")
def dump(
    ctx: typer.Context,
    output: Annotated[
        Path | None,
        typer.Option("--output", "-o", help="Write CSV here instead of to stdout."),
    ] = None,
) -> None:
    """Print artist, album, title and genre for every indexed file.

    Useful for spotting a whole class of mismatch at once -- grepping the output
    for "Symphony" or "Live" surfaces concert recordings filed as studio tracks.
    """
    app_context = context(ctx)
    with app_context.open_index() as index:
        rows = [
            {
                "path": str(entry.path),
                "folder_artist": entry.folder_artist,
                "artist": entry.tags.artist,
                "album": entry.tags.album,
                "title": entry.tags.title,
                "genres": "; ".join(entry.tags.genres),
                "duration_seconds": (
                    f"{entry.duration_seconds:.1f}" if entry.duration_seconds else ""
                ),
            }
            for entry in index.entries()
        ]

    if app_context.as_json:
        console.print_json(json.dumps(rows))
        return

    if output is not None:
        output.parent.mkdir(parents=True, exist_ok=True)
        with output.open("w", encoding="utf-8-sig", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=DUMP_FIELDS)
            writer.writeheader()
            writer.writerows(rows)
        console.print(f"Wrote {len(rows)} row(s) to {output}")
        return

    writer = csv.DictWriter(sys.stdout, fieldnames=DUMP_FIELDS, lineterminator="\n")
    writer.writeheader()
    writer.writerows(rows)
