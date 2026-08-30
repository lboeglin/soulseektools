"""``stk doctor`` -- check that everything the CLI depends on is actually present."""

from __future__ import annotations

import json
import shutil
from dataclasses import dataclass

import typer
from rich.table import Table

from soulseektools.cli.context import console, context

app = typer.Typer(help="Check external tools, paths and configuration.")


@dataclass(frozen=True, slots=True)
class Check:
    """One diagnostic, and what to do when it fails."""

    name: str
    ok: bool
    detail: str
    required: bool = True

    @property
    def status(self) -> str:
        if self.ok:
            return "ok"
        return "missing" if self.required else "optional"


def run_checks(ctx: typer.Context) -> list[Check]:
    """Every diagnostic, in the order they are reported."""
    app_context = context(ctx)
    settings = app_context.settings
    root = app_context.library_root

    ffmpeg = shutil.which("ffmpeg")
    ffprobe = shutil.which("ffprobe")
    fpcalc = shutil.which("fpcalc")

    return [
        Check(
            "config",
            settings.source_path is not None,
            str(settings.source_path) if settings.source_path else "using defaults",
            required=False,
        ),
        Check("library root", root.is_dir(), str(root)),
        Check(
            "library index",
            settings.index.path.parent.is_dir() or not settings.index.path.exists(),
            str(settings.index.path),
        ),
        Check("ffmpeg", ffmpeg is not None, ffmpeg or "needed to verify lossless files"),
        Check("ffprobe", ffprobe is not None, ffprobe or "needed to verify lossless files"),
        Check(
            "fpcalc",
            fpcalc is not None,
            fpcalc or "content duplicates fall back to a decoded-audio hash",
            required=False,
        ),
        Check("slskd url", bool(settings.slskd.url), settings.slskd.url),
        Check(
            "slskd api key",
            settings.slskd.api_key is not None,
            "set" if settings.slskd.api_key else "export SLSKD_API_KEY to enable downloads",
            required=False,
        ),
    ]


@app.callback(invoke_without_command=True)
def doctor(ctx: typer.Context) -> None:
    """Report whether the environment can run every command."""
    app_context = context(ctx)
    checks = run_checks(ctx)

    if app_context.as_json:
        console.print_json(
            json.dumps(
                [
                    {"name": c.name, "status": c.status, "detail": c.detail, "ok": c.ok}
                    for c in checks
                ]
            )
        )
    else:
        table = Table(box=None, pad_edge=False)
        table.add_column("")
        table.add_column("Check")
        # Folded rather than truncated: a diagnostic that elides the path it found
        # cannot be acted on.
        table.add_column("Detail", overflow="fold")
        for check in checks:
            marker = (
                "[green]OK[/]"
                if check.ok
                else ("[red]--[/]" if check.required else "[yellow]--[/]")
            )
            table.add_row(marker, check.name, check.detail)
        console.print(table)

    if any(not check.ok and check.required for check in checks):
        raise typer.Exit(1)
