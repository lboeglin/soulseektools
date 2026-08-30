"""``stk verify`` -- check that lossless files really hold lossless audio."""

from __future__ import annotations

import csv
import json
from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from soulseektools.audio import ffmpeg
from soulseektools.audio.verify import verify_file
from soulseektools.cli.context import AppContext, console, context, fail, progress_bar
from soulseektools.errors import ExternalToolError
from soulseektools.library.scan import LOSSLESS_EXTENSIONS, iter_audio_files
from soulseektools.models import Verdict, VerifyResult

app = typer.Typer(help="Detect lossless files transcoded from a lossy source.")

REPORT_FIELDS = (
    "path",
    "verdict",
    "detail",
    "codec",
    "sample_rate",
    "bit_depth",
    "cutoff_hz",
    "nyquist_hz",
    "steepness_db_per_khz",
)

_VERDICT_STYLE = {
    Verdict.OK: "green",
    Verdict.SUSPECT: "yellow",
    Verdict.FAKE: "red",
    Verdict.ERROR: "dim",
}


@app.callback(invoke_without_command=True)
def verify(
    ctx: typer.Context,
    target: Annotated[
        Path | None,
        typer.Argument(help="File or folder to check. Defaults to the whole library."),
    ] = None,
    report: Annotated[
        Path | None,
        typer.Option("--report", help="Write a CSV report here."),
    ] = None,
    spectrograms: Annotated[
        bool,
        typer.Option("--spectrograms", help="Save a spectrogram beside each flagged file."),
    ] = False,
    only_flagged: Annotated[
        bool,
        typer.Option("--only-flagged", help="Print only SUSPECT and FAKE results."),
    ] = False,
) -> None:
    """Analyse the frequency spectrum of every lossless file and report the verdicts."""
    app_context = context(ctx)
    paths = _targets(ctx, target)
    if not paths:
        console.print("No lossless files to check.")
        return

    results = _analyse(app_context, paths)

    if spectrograms:
        _render_spectrograms(results)
    if report:
        _write_report(report, results)

    if app_context.as_json:
        console.print_json(json.dumps([_as_row(result) for result in results]))
        return

    _print_table(results, only_flagged=only_flagged)
    if report:
        console.print(f"Report written to {report}")
    if any(result.verdict is Verdict.FAKE for result in results):
        raise typer.Exit(1)


def _targets(ctx: typer.Context, target: Path | None) -> list[Path]:
    app_context = context(ctx)
    if target is None:
        return list(iter_audio_files(app_context.require_library_root(), LOSSLESS_EXTENSIONS))
    if target.is_file():
        return [target]
    if target.is_dir():
        return list(iter_audio_files(target, LOSSLESS_EXTENSIONS))
    fail(f"no such file or folder: {target}")
    return []


def _analyse(app_context: AppContext, paths: list[Path]) -> list[VerifyResult]:
    settings = app_context.settings.verify
    with progress_bar("Verifying", len(paths), enabled=not app_context.as_json) as advance:
        results = []
        for path in paths:
            results.append(verify_file(path, settings))
            advance()
    return results


def _render_spectrograms(results: list[VerifyResult]) -> None:
    for result in results:
        if result.verdict not in (Verdict.FAKE, Verdict.SUSPECT):
            continue
        destination = result.path.with_suffix(".spectrogram.png")
        try:
            ffmpeg.render_spectrogram(result.path, destination)
        except ExternalToolError as error:
            console.print(f"[yellow]could not render {destination}: {error}[/]")


def _write_report(destination: Path, results: list[VerifyResult]) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=REPORT_FIELDS)
        writer.writeheader()
        writer.writerows(_as_row(result) for result in results)


def _as_row(result: VerifyResult) -> dict[str, object]:
    return {
        "path": str(result.path),
        "verdict": str(result.verdict),
        "detail": result.detail,
        "codec": result.codec,
        "sample_rate": result.sample_rate,
        "bit_depth": result.bit_depth,
        "cutoff_hz": result.cutoff_hz,
        "nyquist_hz": result.nyquist_hz,
        "steepness_db_per_khz": result.steepness_db_per_khz,
    }


def _print_table(results: list[VerifyResult], *, only_flagged: bool) -> None:
    shown = [
        result
        for result in results
        if not only_flagged or result.verdict in (Verdict.FAKE, Verdict.SUSPECT)
    ]
    if shown:
        table = Table("Verdict", "File", "Detail", box=None, pad_edge=False)
        for result in shown:
            table.add_row(
                f"[{_VERDICT_STYLE[result.verdict]}]{result.verdict}[/]",
                result.path.name,
                result.detail,
            )
        console.print(table)

    counts = dict.fromkeys(Verdict, 0)
    for result in results:
        counts[result.verdict] += 1
    console.print(
        f"Checked {len(results)} file(s): "
        + ", ".join(f"{count} {verdict}" for verdict, count in counts.items() if count)
    )
