"""Shared plumbing for the command groups: settings, console output, exit codes."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

import typer
from rich.console import Console

from soulseektools.config import Settings

if TYPE_CHECKING:
    from pathlib import Path

    from soulseektools.library.index import LibraryIndex
    from soulseektools.review.store import ReviewStore

console = Console()
error_console = Console(stderr=True)


@dataclass(slots=True)
class AppContext:
    """What every command needs: resolved settings and output verbosity."""

    settings: Settings
    verbose: bool = False
    as_json: bool = False

    @property
    def library_root(self) -> Path:
        return self.settings.resolved_library_root()

    def open_index(self) -> LibraryIndex:
        from soulseektools.library.index import LibraryIndex

        return LibraryIndex(self.settings.index.path)

    def open_review(self) -> ReviewStore:
        from soulseektools.review.store import ReviewStore

        return ReviewStore(self.settings.review.path)

    def require_library_root(self) -> Path:
        root = self.library_root
        if not root.is_dir():
            fail(
                f"library root does not exist: {root}",
                hint="set it with 'stk config init' or --library",
            )
        return root


def context(ctx: typer.Context) -> AppContext:
    """The :class:`AppContext` attached by the root callback."""
    assert isinstance(ctx.obj, AppContext)
    return ctx.obj


def fail(message: str, *, hint: str | None = None, code: int = 1) -> None:
    error_console.print(f"[bold red]error:[/] {message}")
    if hint:
        error_console.print(f"[dim]hint: {hint}[/]")
    raise typer.Exit(code)
