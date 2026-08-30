"""Exception types raised across the package."""

from __future__ import annotations


class SoulseekToolsError(Exception):
    """Base class for every error this package raises deliberately."""


class UnsupportedFormatError(SoulseekToolsError):
    """The file's container is not one this package can read or write tags for."""


class TagError(SoulseekToolsError):
    """A tag could not be read from or written to a file."""


class ExternalToolError(SoulseekToolsError):
    """An external binary (ffmpeg, ffprobe, fpcalc) failed or is unavailable."""


class ConfigError(SoulseekToolsError):
    """The configuration is missing, malformed or internally inconsistent."""


class SoulseekClientError(SoulseekToolsError):
    """The Soulseek backend rejected a request or could not be reached."""
