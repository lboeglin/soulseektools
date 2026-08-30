"""``stk genre`` -- apply a consistent genre taxonomy across the library."""

from __future__ import annotations

import json
from typing import Annotated

import typer
from rich.table import Table

from soulseektools import genres as genre_module
from soulseektools.cli.context import console, context, fail
from soulseektools.errors import ConfigError, TagError

app = typer.Typer(help="Audit and apply genre tags.", no_args_is_help=True)


@app.command("check")
def check(ctx: typer.Context) -> None:
    """Show the genres currently in use and which artists the map does not cover."""
    app_context = context(ctx)
    with app_context.open_index() as index:
        entries = list(index.entries())

    counts: dict[str, int] = {}
    untagged = 0
    for entry in entries:
        if not entry.tags.genres:
            untagged += 1
        for genre in entry.tags.genres:
            counts[genre] = counts.get(genre, 0) + 1

    unmapped: dict[str, int] = {}
    map_path = app_context.settings.resolved_genre_map_path()
    if map_path.is_file():
        unmapped = genre_module.unmapped_artists(entries, genre_module.load_genre_map(map_path))

    if app_context.as_json:
        console.print_json(
            json.dumps({"genres": counts, "untagged": untagged, "unmapped_artists": unmapped})
        )
        return

    if counts:
        table = Table("Files", "Genre", box=None, pad_edge=False)
        for genre, count in sorted(counts.items(), key=lambda item: (-item[1], item[0])):
            table.add_row(str(count), genre)
        console.print(table)
    console.print(f"{untagged} of {len(entries)} file(s) have no genre tag")

    if not map_path.is_file():
        console.print(f"[yellow]No genre map at {map_path}[/] -- see data/genres.example.toml")
        return
    if unmapped:
        console.print(f"\n{len(unmapped)} artist folder(s) missing from {map_path.name}:")
        for artist, count in unmapped.items():
            console.print(f"  {count:4d}  {artist}")


@app.command("apply")
def apply_genres(
    ctx: typer.Context,
    apply: Annotated[
        bool,
        typer.Option("--apply", help="Write the tags. Without this, nothing is modified."),
    ] = False,
) -> None:
    """Set each mapped artist's genre tag across their files."""
    app_context = context(ctx)
    map_path = app_context.settings.resolved_genre_map_path()

    try:
        genre_map = genre_module.load_genre_map(map_path)
    except ConfigError as error:
        fail(str(error), hint="copy data/genres.example.toml and edit it for your library")
        return

    with app_context.open_index() as index:
        entries = list(index.entries())

    changes = [
        change for change in genre_module.plan_changes(entries, genre_map) if change.is_change
    ]

    failures: list[tuple[str, str]] = []
    if apply:
        for change in changes:
            try:
                genre_module.apply_change(change)
            except TagError as error:
                failures.append((str(change.entry.path), str(error)))

    if app_context.as_json:
        console.print_json(
            json.dumps(
                {
                    "planned": [
                        {
                            "path": str(change.entry.path),
                            "from": list(change.current),
                            "to": list(change.proposed),
                        }
                        for change in changes
                    ],
                    "applied": apply,
                    "failed": [{"path": path, "error": error} for path, error in failures],
                }
            )
        )
        return

    if not changes:
        console.print(f"Every mapped file already carries its genre ({len(genre_map)} artists).")
        return

    table = Table(box=None, pad_edge=False)
    table.add_column("File", overflow="fold")
    table.add_column("From")
    table.add_column("To")
    for change in changes[:50]:
        table.add_row(
            str(change.entry.path),
            ", ".join(change.current) or "[dim]none[/]",
            ", ".join(change.proposed),
        )
    console.print(table)

    if apply:
        console.print(f"Tagged {len(changes) - len(failures)} file(s)")
        for path, reason in failures:
            console.print(f"[yellow]failed[/] {path}: {reason}")
    else:
        console.print(
            f"{len(changes)} file(s) would change. "
            "Nothing was written -- pass [bold]--apply[/] to tag them."
        )
