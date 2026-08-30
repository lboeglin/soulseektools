"""The review app.

Approving candidates was the workflow's real bottleneck: the old plugin wrote
three candidates per row into a CSV and waited for someone to type ``1``, ``2``
or ``3`` into an ``approve`` column in a spreadsheet. Here the same decision is
one keystroke, and -- unlike a spreadsheet cell -- each candidate is shown with
its full remote path and any warnings raised about it, which is exactly the
information the old workflow hid.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, ClassVar

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.widgets import DataTable, Footer, Header, Static

from soulseektools.models import TrackStatus

if TYPE_CHECKING:
    from soulseektools.review.store import ReviewStore, ReviewTrack

REVIEWABLE = (TrackStatus.AWAITING_REVIEW, TrackStatus.APPROVED, TrackStatus.REJECTED)

_STATUS_STYLE = {
    TrackStatus.AWAITING_REVIEW: "cyan",
    TrackStatus.APPROVED: "green",
    TrackStatus.REJECTED: "red",
    TrackStatus.NO_CANDIDATES: "yellow",
}


class ReviewApp(App[None]):
    """Pick which candidate to download for each track found."""

    CSS = """
    #tracks { width: 45%; border-right: solid $panel; }
    #candidates { padding: 0 1; }
    #summary { dock: bottom; padding: 0 1; color: $text-muted; }
    """

    BINDINGS: ClassVar = [
        Binding("1", "approve(1)", "Take 1"),
        Binding("2", "approve(2)", "Take 2"),
        Binding("3", "approve(3)", "Take 3"),
        Binding("r", "reject", "Reject"),
        Binding("u", "unset", "Undo"),
        Binding("a", "toggle_all", "Show all"),
        Binding("q", "quit", "Quit"),
    ]

    def __init__(self, store: ReviewStore) -> None:
        super().__init__()
        self.store = store
        self.entries: list[ReviewTrack] = []
        self.show_all = False

    def compose(self) -> ComposeResult:
        yield Header()
        with Horizontal():
            yield DataTable(id="tracks", cursor_type="row", zebra_stripes=True)
            with Vertical():
                yield Static(id="candidates")
        yield Static(id="summary")
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#tracks", DataTable)
        table.add_columns("Status", "Track")
        self.reload()

    def reload(self, keep_row: int = 0) -> None:
        """Re-read the queue and redraw, holding the cursor where it was."""
        statuses = () if self.show_all else REVIEWABLE
        self.entries = self.store.tracks(*statuses)

        table = self.query_one("#tracks", DataTable)
        table.clear()
        for entry in self.entries:
            style = _STATUS_STYLE.get(entry.status, "dim")
            table.add_row(
                f"[{style}]{entry.status}[/]", f"{entry.track.artist} - {entry.track.title}"
            )

        if self.entries:
            table.move_cursor(row=min(keep_row, len(self.entries) - 1))
        self.render_selected()
        self.render_summary()

    @property
    def selected(self) -> ReviewTrack | None:
        table = self.query_one("#tracks", DataTable)
        row = table.cursor_row
        return self.entries[row] if 0 <= row < len(self.entries) else None

    def render_selected(self) -> None:
        panel = self.query_one("#candidates", Static)
        entry = self.selected
        if entry is None:
            panel.update(EMPTY_QUEUE_MESSAGE)
            return
        panel.update(render_candidates(entry))

    def render_summary(self) -> None:
        self.query_one("#summary", Static).update(render_summary(self.store))

    def on_data_table_row_highlighted(self, _event: DataTable.RowHighlighted) -> None:
        self.render_selected()

    def action_approve(self, rank: int) -> None:
        entry = self.selected
        if entry is None or rank > len(entry.candidates):
            self.bell()
            return
        self.store.approve(entry.id, rank)
        self.advance()

    def action_reject(self) -> None:
        entry = self.selected
        if entry is None:
            return
        self.store.reject(entry.id, "rejected in review")
        self.advance()

    def action_unset(self) -> None:
        """Put a decided track back into review, for a change of mind."""
        entry = self.selected
        if entry is None:
            return
        self.store.set_candidates(entry.id, entry.candidates)
        self.reload(self.query_one("#tracks", DataTable).cursor_row)

    def action_toggle_all(self) -> None:
        self.show_all = not self.show_all
        self.reload()

    def advance(self) -> None:
        """Redraw, leaving the cursor on the next track needing a decision.

        In the filtered view a decided track drops out of the list, so holding
        the row index already lands on the next one; when everything is shown,
        the cursor has to step down explicitly.
        """
        row = self.query_one("#tracks", DataTable).cursor_row
        self.reload(row + 1 if self.show_all else row)


def render_summary(store: ReviewStore) -> str:
    """One line counting the queue by state."""
    counts = store.counts()
    return ", ".join(f"{value} {key}" for key, value in sorted(counts.items())) or "queue is empty"


EMPTY_QUEUE_MESSAGE = "Nothing to review. Run [bold]stk fetch search[/] first."


def render_candidates(entry: ReviewTrack) -> str:
    lines = [f"[bold]{entry.track.artist} - {entry.track.title}[/]"]
    if entry.track.album:
        lines.append(f"[dim]{entry.track.album}[/]")
    if entry.track.duration_seconds:
        lines.append(f"[dim]{entry.track.duration_seconds:.0f}s[/]")
    lines.append("")

    if not entry.candidates:
        lines.append("[yellow]No candidates were found for this track.[/]")
        return "\n".join(lines)

    for rank, scored in enumerate(entry.candidates, start=1):
        marker = "[green]*[/]" if rank == entry.approved_rank else " "
        lines.append(f"{marker}[bold]{rank}.[/] {scored.describe()}")
        # The full remote path is the point: a filename alone cannot tell a
        # studio track from a live recording sitting in a concert folder.
        lines.append(f"   [dim]{scored.candidate.remote_path}[/]")
        if scored.flags:
            lines.append(f"   [yellow]warnings: {', '.join(scored.flags)}[/]")
        lines.append("")

    return "\n".join(lines)


def run(store: ReviewStore) -> None:
    """Open the review app against a queue."""
    ReviewApp(store).run()
