"""``stk dupes`` -- find the same track filed twice."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Annotated

import typer
from rich.table import Table

from soulseektools import duplicates
from soulseektools.audio.fingerprint import available_kind
from soulseektools.cli.context import AppContext, console, context

if TYPE_CHECKING:
    from soulseektools.duplicates import DuplicatePair
    from soulseektools.models import LibraryEntry

app = typer.Typer(help="Find duplicate tracks by title or by audio.", no_args_is_help=True)


@app.command("titles")
def titles(
    ctx: typer.Context,
    limit: Annotated[int, typer.Option("--limit", help="Show at most this many pairs.")] = 50,
) -> None:
    """Match on artist and title, across the whole library.

    Catches the same song under two spellings of an artist, which grouping by
    folder cannot see.
    """
    app_context = context(ctx)
    with app_context.open_index() as index:
        entries = list(index.entries())

    if not _require_index(entries):
        return

    pairs = duplicates.find_by_title(entries, app_context.settings.matching.thresholds())
    _report(app_context, pairs, limit, scanned=len(entries))


@app.command("content")
def content(
    ctx: typer.Context,
    threshold: Annotated[
        float,
        typer.Option("--threshold", min=0.0, max=1.0, help="Minimum audio similarity."),
    ] = duplicates.DEFAULT_AUDIO_THRESHOLD,
    tolerance: Annotated[
        float,
        typer.Option("--tolerance", help="Duration difference, in seconds, still worth checking."),
    ] = duplicates.DEFAULT_DURATION_TOLERANCE_SECONDS,
    limit: Annotated[int, typer.Option("--limit", help="Show at most this many pairs.")] = 50,
) -> None:
    """Match on the audio itself, whatever the files are named or tagged."""
    app_context = context(ctx)
    with app_context.open_index() as index:
        entries = list(index.entries())
        if not _require_index(entries):
            return

        if not app_context.as_json:
            console.print(f"Fingerprinting with the [bold]{available_kind()}[/] backend")
        pairs = duplicates.find_by_audio(
            entries,
            threshold=threshold,
            duration_tolerance_seconds=tolerance,
            index=index,
        )

    _report(app_context, pairs, limit, scanned=len(entries))


def _require_index(entries: list[LibraryEntry]) -> bool:
    if entries:
        return True
    console.print("Index is empty. Run [bold]stk library scan[/] first.")
    return False


def _report(app_context: AppContext, pairs: list[DuplicatePair], limit: int, scanned: int) -> None:
    if app_context.as_json:
        console.print_json(
            json.dumps(
                [
                    {
                        "score": round(pair.score, 3),
                        "evidence": pair.evidence,
                        "keep": str(pair.larger.path),
                        "other": str(pair.smaller.path),
                        "keep_bytes": pair.larger.size,
                        "other_bytes": pair.smaller.size,
                    }
                    for pair in pairs
                ]
            )
        )
        return

    if not pairs:
        console.print(f"No duplicates found across {scanned} file(s).")
        return

    table = Table("Score", "Larger copy", "Other copy", "Evidence", box=None, pad_edge=False)
    for pair in pairs[:limit]:
        table.add_row(
            f"{pair.score:.2f}",
            f"{_megabytes(pair.larger.size)}  {pair.larger.path}",
            f"{_megabytes(pair.smaller.size)}  {pair.smaller.path}",
            pair.evidence,
        )
    console.print(table)

    shown = min(limit, len(pairs))
    suffix = f" (showing {shown})" if shown < len(pairs) else ""
    console.print(f"{len(pairs)} likely duplicate pair(s) across {scanned} file(s){suffix}")
    console.print("[dim]Nothing was deleted. Verify each pair before removing anything.[/]")


def _megabytes(size: int) -> str:
    return f"{size / 1_000_000:6.1f} MB"
