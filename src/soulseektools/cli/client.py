"""Opening a connection to the Soulseek backend, with one clear failure message."""

from __future__ import annotations

from typing import TYPE_CHECKING

from soulseektools.cli.context import fail
from soulseektools.soulseek.slskd import SlskdClient

if TYPE_CHECKING:
    from soulseektools.cli.context import AppContext


def open_client(app_context: AppContext) -> SlskdClient:
    """Connect to slskd, or explain what to check."""
    settings = app_context.settings.slskd
    client = SlskdClient(settings)
    if not client.is_connected():
        client.close()
        fail(
            f"slskd at {settings.url} is not connected to the Soulseek server",
            hint="check the daemon is running, SLSKD_API_KEY is set, and its credentials are valid",
        )
    return client
