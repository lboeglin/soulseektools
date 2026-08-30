"""``stk config`` -- inspect and create the configuration file."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Annotated

import typer

from soulseektools.cli.context import console, context, fail
from soulseektools.config import CONFIG_FILENAME, default_config_path, render_toml, write_config
from soulseektools.errors import ConfigError

app = typer.Typer(help="Inspect and create the configuration file.", no_args_is_help=True)


@app.command("init")
def init(
    ctx: typer.Context,
    path: Annotated[
        Path | None,
        typer.Option("--path", help="Where to write the file."),
    ] = None,
    user: Annotated[
        bool,
        typer.Option("--user", help=f"Write the per-user config instead of ./{CONFIG_FILENAME}."),
    ] = False,
) -> None:
    """Write a configuration file containing the current effective settings."""
    target = path or (default_config_path() if user else Path.cwd() / CONFIG_FILENAME)
    try:
        written = write_config(target, context(ctx).settings)
    except ConfigError as error:
        fail(str(error), hint="edit it directly, or pass --path to write elsewhere")
        return
    console.print(f"Wrote {written}")


@app.command("show")
def show(ctx: typer.Context) -> None:
    """Print the effective settings, secrets omitted."""
    app_context = context(ctx)
    if app_context.as_json:
        console.print_json(
            json.dumps(app_context.settings.model_dump(mode="json", exclude={"source_path"}))
        )
        return
    console.print(render_toml(app_context.settings))


@app.command("path")
def show_path(ctx: typer.Context) -> None:
    """Print which configuration file is in effect."""
    source = context(ctx).settings.source_path
    # Printed unwrapped: this line is meant to be substituted into other commands.
    console.print(
        str(source) if source else "no config file found; using defaults",
        soft_wrap=True,
        highlight=False,
    )
