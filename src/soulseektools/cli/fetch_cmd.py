"""``stk fetch`` -- search Soulseek, queue approved picks, and check what arrives."""

from __future__ import annotations

import json
from typing import Annotated

import typer
from rich.table import Table

from soulseektools import fetch
from soulseektools.cli.context import AppContext, console, context, fail, progress_bar
from soulseektools.models import TrackStatus, Verdict
from soulseektools.soulseek.slskd import SlskdClient

app = typer.Typer(help="Search Soulseek and download approved candidates.", no_args_is_help=True)


@app.command("search")
def search(
    ctx: typer.Context,
    limit: Annotated[
        int | None,
        typer.Option("--limit", help="Search at most this many tracks."),
    ] = None,
) -> None:
    """Search for every track waiting in the review queue and rank what comes back."""
    app_context = context(ctx)

    with _client(app_context) as client, app_context.open_review() as store:
        pending = store.tracks(TrackStatus.PENDING, limit=limit)
        if not pending:
            console.print("Nothing waiting to be searched.")
            return

        with progress_bar("Searching", len(pending), enabled=not app_context.as_json) as advance:
            outcomes = fetch.search_pending(
                store, client, app_context.settings, limit=limit, on_track=advance
            )

    if app_context.as_json:
        console.print_json(
            json.dumps(
                [
                    {
                        "track_id": outcome.track_id,
                        "query": outcome.query,
                        "found": outcome.found,
                        "error": outcome.error,
                    }
                    for outcome in outcomes
                ]
            )
        )
        return

    table = Table("Found", "Track", "Note", box=None, pad_edge=False)
    for outcome in outcomes:
        table.add_row(
            str(outcome.found) if not outcome.error else "[red]--[/]",
            outcome.query,
            outcome.error,
        )
    console.print(table)
    with_candidates = sum(1 for outcome in outcomes if outcome.found)
    console.print(
        f"Searched {len(outcomes)} track(s); {with_candidates} have candidates to review. "
        "Run [bold]stk review[/] to choose."
    )


@app.command("download")
def download(
    ctx: typer.Context,
    limit: Annotated[
        int | None,
        typer.Option("--limit", help="Queue at most this many tracks."),
    ] = None,
    collect_only: Annotated[
        bool,
        typer.Option("--collect", help="Only check finished transfers; queue nothing new."),
    ] = False,
) -> None:
    """Queue approved picks, then record and verify whatever has finished."""
    app_context = context(ctx)

    with _client(app_context) as client, app_context.open_review() as store:
        queued = [] if collect_only else fetch.enqueue_approved(store, client, limit=limit)
        collected = fetch.collect(store, client, app_context.settings)

    if app_context.as_json:
        console.print_json(
            json.dumps(
                {
                    "queued": [
                        {
                            "track_id": outcome.track_id,
                            "query": outcome.query,
                            "destination": outcome.destination,
                            "ok": outcome.queued,
                            "error": outcome.error,
                        }
                        for outcome in queued
                    ],
                    "collected": [
                        {
                            "track_id": outcome.track_id,
                            "query": outcome.query,
                            "state": str(outcome.state),
                            "path": str(outcome.local_path) if outcome.local_path else "",
                            "verdict": str(outcome.verdict.verdict) if outcome.verdict else "",
                            "detail": outcome.verdict.detail if outcome.verdict else "",
                        }
                        for outcome in collected
                    ],
                }
            )
        )
        return

    if queued:
        console.print(f"Queued {sum(1 for enqueued in queued if enqueued.queued)} download(s)")
        for enqueued in queued:
            if enqueued.error:
                console.print(f"[yellow]failed[/] {enqueued.query}: {enqueued.error}")

    if collected:
        table = Table("Outcome", "Track", "Detail", box=None, pad_edge=False)
        for finished in collected:
            table.add_row(_outcome_label(finished), finished.query, _outcome_detail(finished))
        console.print(table)
    elif not queued:
        console.print("Nothing approved to queue, and no finished transfers to record.")


def _outcome_label(outcome: fetch.CollectOutcome) -> str:
    if outcome.error:
        return "[red]failed[/]"
    if outcome.verdict is None:
        return "[green]done[/]"
    style = "green" if outcome.verdict.verdict is Verdict.OK else "red"
    return f"[{style}]{outcome.verdict.verdict}[/]"


def _outcome_detail(outcome: fetch.CollectOutcome) -> str:
    if outcome.error:
        return outcome.error
    if outcome.verdict is not None:
        return outcome.verdict.detail
    return str(outcome.local_path or "")


def _client(app_context: AppContext) -> SlskdClient:
    settings = app_context.settings.slskd
    client = SlskdClient(settings)
    if not client.is_connected():
        client.close()
        fail(
            f"slskd at {settings.url} is not connected to the Soulseek server",
            hint="check the daemon is running, SLSKD_API_KEY is set, and its credentials are valid",
        )
    return client
