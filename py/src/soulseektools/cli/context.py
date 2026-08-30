"""Shared plumbing for the command groups: settings, console output, exit codes."""

from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
from typing import TYPE_CHECKING

import typer
from rich.console import Console
from rich.progress import BarColumn, Progress, TextColumn, TimeRemainingColumn

from soulseektools.config import Settings

if TYPE_CHECKING:
    from collections.abc import Callable, Iterator
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


@contextmanager
def progress_bar(
    description: str, total: int | None = None, *, enabled: bool = True
) -> Iterator[Callable[..., None]]:
    """A transient progress bar, yielding a callable that advances it.

    Disabled for machine-readable output, where a redrawing bar would corrupt
    the stream. The yielded callable ignores its arguments, so it can be handed
    straight to a callback that reports what it is working on.
    """
    if not enabled:
        yield lambda *_args: None
        return

    columns = [TextColumn("[progress.description]{task.description}"), BarColumn()]
    columns.append(TextColumn("{task.completed}/{task.total}" if total else "{task.completed}"))
    if total:
        columns.append(TimeRemainingColumn())

    with Progress(*columns, console=console, transient=True) as progress:
        task = progress.add_task(description, total=total)
        yield lambda *_args: progress.advance(task)


def fail(message: str, *, hint: str | None = None, code: int = 1) -> None:
    error_console.print(f"[bold red]error:[/] {message}")
    if hint:
        error_console.print(f"[dim]hint: {hint}[/]")
    raise typer.Exit(code)
