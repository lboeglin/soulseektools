"""The ``stk`` entry point."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer

from soulseektools import __version__
from soulseektools.cli import config_cmd, doctor_cmd, library_cmd
from soulseektools.cli.context import AppContext, console, fail
from soulseektools.config import load_settings
from soulseektools.errors import ConfigError

app = typer.Typer(
    name="stk",
    help="Build a music library from a streaming export via Soulseek, and keep it clean.",
    no_args_is_help=True,
    add_completion=False,
)
app.add_typer(config_cmd.app, name="config")
app.add_typer(doctor_cmd.app, name="doctor")
app.add_typer(library_cmd.app, name="library")


def _version(value: bool) -> None:
    if value:
        console.print(f"stk {__version__}")
        raise typer.Exit


@app.callback()
def main(
    ctx: typer.Context,
    config: Annotated[
        Path | None,
        typer.Option("--config", "-c", help="Configuration file to use.", envvar="STK_CONFIG"),
    ] = None,
    library: Annotated[
        Path | None,
        typer.Option("--library", "-l", help="Override the configured library root."),
    ] = None,
    as_json: Annotated[
        bool,
        typer.Option("--json", help="Emit machine-readable output."),
    ] = False,
    verbose: Annotated[bool, typer.Option("--verbose", "-v", help="Show more detail.")] = False,
    _show_version: Annotated[
        bool,
        typer.Option("--version", callback=_version, is_eager=True, help="Show the version."),
    ] = False,
) -> None:
    """Resolve the configuration once and hand it to every command."""
    overrides = {"library": {"root": library}} if library else {}
    try:
        settings = load_settings(config, **overrides)
    except ConfigError as error:
        fail(str(error))
        return
    ctx.obj = AppContext(settings=settings, verbose=verbose, as_json=as_json)


if __name__ == "__main__":
    app()
