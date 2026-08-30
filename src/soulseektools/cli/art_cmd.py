"""``stk art`` -- audit and repair embedded cover art."""

from __future__ import annotations

import json
from dataclasses import replace
from typing import TYPE_CHECKING, Annotated

import typer
from rich.table import Table

from soulseektools.artwork import service
from soulseektools.cli.context import AppContext, console, context, fail
from soulseektools.errors import ConfigError, TagError

if TYPE_CHECKING:
    from soulseektools.artwork.service import ArtworkResult

app = typer.Typer(help="Audit and repair embedded cover art.", no_args_is_help=True)


@app.command("check")
def check(ctx: typer.Context) -> None:
    """List files with no cover, or with one too small to be a real image."""
    app_context = context(ctx)
    with app_context.open_index() as index:
        entries = list(index.entries())

    if not entries:
        console.print("Index is empty. Run [bold]stk library scan[/] first.")
        return

    statuses = service.audit(entries, app_context.settings.artwork)
    missing = [status for status in statuses if status.needs_artwork]

    if app_context.as_json:
        console.print_json(
            json.dumps(
                [{"path": str(status.entry.path), "reason": status.reason} for status in missing]
            )
        )
        return

    if missing:
        table = Table(box=None, pad_edge=False)
        table.add_column("File", overflow="fold")
        table.add_column("Problem")
        for status in missing:
            table.add_row(str(status.entry.path), status.reason)
        console.print(table)
    console.print(f"{len(missing)} of {len(entries)} file(s) need artwork")


@app.command("fetch")
def fetch(
    ctx: typer.Context,
    apply: Annotated[
        bool,
        typer.Option("--apply", help="Write the artwork. Without this, nothing is modified."),
    ] = False,
    limit: Annotated[
        int | None,
        typer.Option("--limit", help="Look up at most this many files."),
    ] = None,
    allow_unverified: Annotated[
        bool,
        typer.Option(
            "--allow-unverified",
            help="Accept a result even when its artist does not match. Not recommended.",
        ),
    ] = False,
) -> None:
    """Look up artwork for files that need it, and embed the verified matches.

    A result whose artist does not match the file's is rejected rather than
    embedded: the wrong cover is worse than none, and search match quality
    outside the mainstream is poor.
    """
    app_context = context(ctx)
    settings = app_context.settings.artwork
    if allow_unverified:
        settings = settings.model_copy(update={"require_artist_match": False})

    try:
        providers = service.build_providers(settings)
    except ConfigError as error:
        fail(str(error))
        return

    with app_context.open_index() as index:
        entries = list(index.entries())

    needing = [status.entry for status in service.audit(entries, settings) if status.needs_artwork]
    if limit is not None:
        needing = needing[:limit]
    if not needing:
        console.print("Every indexed file already has usable artwork.")
        return

    results = []
    with service.new_client(settings) as client:
        for entry in needing:
            result = service.find_artwork(entry, providers, settings, client)
            if result.ok and apply:
                try:
                    service.embed(result)
                except TagError as error:
                    result = replace(result, error=str(error))
            results.append(result)

    _report(app_context, results, applied=apply)


def _report(app_context: AppContext, results: list[ArtworkResult], *, applied: bool) -> None:
    if app_context.as_json:
        console.print_json(
            json.dumps(
                [
                    {
                        "path": str(result.entry.path),
                        "ok": result.ok,
                        "reason": result.error or result.selection.reason,
                        "url": result.selection.candidate.url if result.selection.candidate else "",
                        "bytes": len(result.artwork) if result.artwork else 0,
                        "applied": applied and result.ok,
                    }
                    for result in results
                ]
            )
        )
        return

    table = Table("", "File", "Outcome", box=None, pad_edge=False)
    for result in results:
        marker = "[green]OK[/]" if result.ok else "[yellow]--[/]"
        table.add_row(marker, result.entry.path.name, result.error or result.selection.reason)
    console.print(table)

    found = sum(1 for result in results if result.ok)
    if applied:
        console.print(f"Embedded artwork in {found} of {len(results)} file(s)")
    else:
        console.print(
            f"Found verified artwork for {found} of {len(results)} file(s). "
            "Nothing was written -- pass [bold]--apply[/] to embed it."
        )
