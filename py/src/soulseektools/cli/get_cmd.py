"""``stk get`` -- fetch one track by name, without a CSV in sight.

The batch flow exists for building a library from an export. This is the other
half: you want one song, you know what it is called, and setting up a wantlist to
get it would be absurd. Same search, same ranking, same verification -- just
asked for directly and answered in one command.
"""

from __future__ import annotations

import json
from typing import Annotated

import typer
from rich.table import Table

from soulseektools import fetch
from soulseektools.cli.client import open_client
from soulseektools.cli.context import console, context, fail
from soulseektools.models import ScoredCandidate, Track
from soulseektools.soulseek import ranking

# The en and em dashes are deliberate: a track name pasted out of a browser
# or a music player usually carries one rather than a plain hyphen.
SEPARATORS = (" - ", " – ", " — ")  # noqa: RUF001


def split_query(query: str) -> tuple[str, str]:
    """Split ``artist - title`` into its parts, tolerating either being absent.

    A query with no separator is treated as a title: searching for a title alone
    works, searching for a bare artist does not.
    """
    for separator in SEPARATORS:
        if separator in query:
            artist, _, title = query.partition(separator)
            return artist.strip(), title.strip()
    return "", query.strip()


def get(
    ctx: typer.Context,
    query: Annotated[
        str,
        typer.Argument(help='What to find: "Artist - Title", or just a title.'),
    ],
    artist: Annotated[
        str | None,
        typer.Option("--artist", "-a", help="The artist, if not given in the query."),
    ] = None,
    lossless: Annotated[
        bool,
        typer.Option("--lossless", help="Accept only lossless results."),
    ] = False,
    min_bitrate: Annotated[
        int | None,
        typer.Option("--min-bitrate", help="Reject lossy results below this bitrate."),
    ] = None,
    yes: Annotated[
        bool,
        typer.Option("--yes", "-y", help="Take the best candidate without asking."),
    ] = False,
    queue_only: Annotated[
        bool,
        typer.Option("--queue-only", help="Choose a candidate but do not download it yet."),
    ] = False,
    limit: Annotated[
        int,
        typer.Option("--limit", "-n", help="How many candidates to offer."),
    ] = 5,
) -> None:
    """Search for one track, show what turned up, and download the one you pick."""
    app_context = context(ctx)
    parsed_artist, title = split_query(query)
    track = Track(artist=artist or parsed_artist, title=title)
    if not track.title:
        fail("nothing to search for", hint='try: stk get "DECO*27 - Roki"')
        return

    options = app_context.settings.ranking.model_copy(
        update={
            "require_lossless": lossless or app_context.settings.ranking.require_lossless,
            "min_bitrate": min_bitrate or app_context.settings.ranking.min_bitrate,
        }
    )

    with open_client(app_context) as client:
        if not app_context.as_json:
            console.print(f"Searching for [bold]{track.query}[/] ...")
        candidates = client.search(
            f"{track.artist} {track.title}".strip(),
            timeout_seconds=app_context.settings.slskd.search_timeout_seconds,
            response_limit=app_context.settings.slskd.search_response_limit,
        )
        scored = ranking.rank(track, candidates, options, limit=limit)

        if app_context.as_json:
            console.print_json(json.dumps(_as_payload(track, scored)))
            return

        if not scored:
            console.print(
                f"Nothing usable found for [bold]{track.query}[/]"
                + (" with those quality limits" if lossless or min_bitrate else "")
            )
            raise typer.Exit(1)

        _print_candidates(scored)
        choice = 1 if yes else _ask(len(scored))
        if choice is None:
            console.print("Nothing downloaded.")
            return

        with app_context.open_review() as store:
            store.add([track])
            entry = next(item for item in store.tracks() if item.track.query == track.query)
            store.set_candidates(entry.id, scored)
            store.approve(entry.id, choice)

            if queue_only:
                console.print(f"Approved candidate {choice}. Run [bold]stk fetch download[/].")
                return

            outcomes = fetch.enqueue_approved(store, client, limit=None)

    failures = [outcome for outcome in outcomes if not outcome.queued]
    if failures:
        fail(failures[0].error or "could not queue the download")
        return
    console.print(
        f"Queued candidate {choice}. "
        "Run [bold]stk fetch download --collect[/] to record it once it finishes."
    )


def _print_candidates(scored: list[ScoredCandidate]) -> None:
    table = Table(box=None, pad_edge=False)
    table.add_column("#")
    table.add_column("Detail", overflow="fold")
    for rank, candidate in enumerate(scored, start=1):
        table.add_row(str(rank), candidate.describe())
        table.add_row("", f"[dim]{candidate.candidate.remote_path}[/]")
        if candidate.flags:
            table.add_row("", f"[yellow]warnings: {', '.join(candidate.flags)}[/]")
        table.add_section()
    console.print(table)


def _ask(count: int) -> int | None:
    """Ask which candidate to take, or none."""
    answer = (
        typer.prompt(f"Take which? [1-{count}, or n for none]", default="1", show_default=True)
        .strip()
        .lower()
    )
    if answer in {"n", "no", "none", "q"}:
        return None
    if answer.isdigit() and 1 <= int(answer) <= count:
        return int(answer)
    console.print("[yellow]Not one of the offered candidates.[/]")
    return None


def _as_payload(track: Track, scored: list[ScoredCandidate]) -> dict[str, object]:
    return {
        "query": track.query,
        "candidates": [
            {
                "rank": rank,
                "score": candidate.score,
                "username": candidate.candidate.username,
                "remote_path": candidate.candidate.remote_path,
                "size": candidate.candidate.size,
                "flags": list(candidate.flags),
            }
            for rank, candidate in enumerate(scored, start=1)
        ],
    }
