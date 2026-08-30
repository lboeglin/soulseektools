"""``stk library`` -- build and inspect the cached view of the library."""

from __future__ import annotations

import json
from typing import Annotated

import typer
from rich.table import Table

from soulseektools.cli.context import console, context, progress_bar

app = typer.Typer(help="Scan and inspect the library index.", no_args_is_help=True)


@app.command("scan")
def scan(
    ctx: typer.Context,
    full: Annotated[
        bool,
        typer.Option("--full", help="Re-read every file, ignoring the cached size and mtime."),
    ] = False,
) -> None:
    """Bring the index in line with what is on disk."""
    app_context = context(ctx)
    root = app_context.require_library_root()

    with (
        app_context.open_index() as index,
        progress_bar(f"Scanning {root}", enabled=not app_context.as_json) as report,
    ):
        result = index.refresh(
            root,
            app_context.settings.library.extensions,
            full=full,
            on_file=report,
        )
        total = index.count()

    if app_context.as_json:
        console.print_json(
            json.dumps(
                {
                    "root": str(root),
                    "added": result.added,
                    "updated": result.updated,
                    "unchanged": result.unchanged,
                    "removed": result.removed,
                    "failed": [[str(path), reason] for path, reason in result.failed],
                    "total": total,
                }
            )
        )
        return

    console.print(
        f"Indexed {total} file(s) under {root}: "
        f"[green]{result.added} added[/], {result.updated} updated, "
        f"{result.unchanged} unchanged, {result.removed} removed"
    )
    for path, reason in result.failed:
        console.print(f"[yellow]skipped[/] {path}: {reason}")


@app.command("stats")
def stats(
    ctx: typer.Context,
    top: Annotated[int, typer.Option("--top", help="How many artists to list.")] = 20,
) -> None:
    """Summarise what the index currently holds."""
    app_context = context(ctx)
    with app_context.open_index() as index:
        total = index.count()
        by_artist = index.artist_counts()
        entries = list(index.entries())

    tagged = sum(1 for entry in entries if entry.tags.title)
    with_art = sum(1 for entry in entries if entry.has_artwork)
    genres = sum(1 for entry in entries if entry.tags.genres)

    if app_context.as_json:
        console.print_json(
            json.dumps(
                {
                    "total": total,
                    "tagged": tagged,
                    "with_artwork": with_art,
                    "with_genre": genres,
                    "artists": by_artist,
                }
            )
        )
        return

    if not total:
        console.print("Index is empty. Run [bold]stk library scan[/] first.")
        return

    console.print(
        f"{total} file(s) across {len(by_artist)} artist folder(s): "
        f"{tagged} titled, {with_art} with artwork, {genres} with a genre"
    )

    table = Table("Artist", "Files", title=f"Largest {min(top, len(by_artist))} artist folders")
    for artist, count in list(by_artist.items())[:top]:
        table.add_row(artist or "[dim]<library root>[/]", str(count))
    console.print(table)
