"""Layered configuration.

Every path, threshold and term list that used to be a literal in a script is a
field here. Values resolve in this order, first match winning:

1. explicit overrides passed by a CLI flag
2. ``STK_``-prefixed environment variables (``STK_LIBRARY__ROOT``)
3. a TOML file -- ``stk.toml`` beside the project, else the per-user config
4. the defaults below

Secrets are the exception: an API key is read from the environment only, never
from the TOML file, so a config file stays safe to commit.
"""

from __future__ import annotations

import os
import tomllib
from pathlib import Path
from typing import Any

import tomli_w
from platformdirs import user_config_path, user_data_path
from pydantic import BaseModel, ConfigDict, Field, SecretStr, field_validator
from pydantic_settings import (
    BaseSettings,
    PydanticBaseSettingsSource,
    SettingsConfigDict,
    TomlConfigSettingsSource,
)

from soulseektools.errors import ConfigError
from soulseektools.library.scan import AUDIO_EXTENSIONS
from soulseektools.matching import MatchThresholds

APP_NAME = "soulseektools"
CONFIG_FILENAME = "stk.toml"
API_KEY_ENV_VAR = "SLSKD_API_KEY"


def default_config_path() -> Path:
    return user_config_path(APP_NAME, appauthor=False) / "config.toml"


def default_data_path(name: str) -> Path:
    return user_data_path(APP_NAME, appauthor=False) / name


def find_config_file(start: Path | None = None) -> Path | None:
    """Locate the config file: nearest ``stk.toml`` upwards, else the user's."""
    directory = (start or Path.cwd()).resolve()
    for candidate in (directory, *directory.parents):
        local = candidate / CONFIG_FILENAME
        if local.is_file():
            return local
    user = default_config_path()
    return user if user.is_file() else None


class Section(BaseModel):
    """Base for every config section: a typo in a key is an error, not a no-op."""

    model_config = ConfigDict(extra="forbid")


class LibrarySettings(Section):
    """Where the music lives and what counts as part of it."""

    root: Path = Path("Music")
    extensions: tuple[str, ...] = tuple(sorted(AUDIO_EXTENSIONS))

    @field_validator("extensions")
    @classmethod
    def _normalise(cls, value: tuple[str, ...]) -> tuple[str, ...]:
        return tuple(
            extension.lower() if extension.startswith(".") else f".{extension.lower()}"
            for extension in value
        )


class IndexSettings(Section):
    path: Path = Field(default_factory=lambda: default_data_path("library.db"))


class ReviewSettings(Section):
    path: Path = Field(default_factory=lambda: default_data_path("review.db"))
    candidates_per_track: int = Field(default=3, ge=1, le=10)


class MatchingSettings(Section):
    """Thresholds deciding when two names describe the same track."""

    artist: float = Field(default=0.6, ge=0.0, le=1.0)
    artist_weak: float = Field(default=0.35, ge=0.0, le=1.0)
    title: float = Field(default=0.62, ge=0.0, le=1.0)
    title_strong: float = Field(default=0.85, ge=0.0, le=1.0)
    artist_weight: float = Field(default=0.4, ge=0.0, le=1.0)

    def thresholds(self) -> MatchThresholds:
        return MatchThresholds(
            artist=self.artist,
            artist_weak=self.artist_weak,
            title=self.title,
            title_strong=self.title_strong,
            artist_weight=self.artist_weight,
        )


class SlskdSettings(Section):
    """Connection details for the headless Soulseek daemon."""

    url: str = "http://localhost:5030"
    api_key: SecretStr | None = None
    search_timeout_seconds: int = Field(default=15, ge=1)
    search_response_limit: int = Field(default=200, ge=1)
    request_timeout_seconds: float = Field(default=30.0, gt=0)
    download_directory: Path | None = None


class RankingSettings(Section):
    """How Soulseek search results are scored.

    ``reject_terms`` and ``penalise_terms`` are matched against the whole remote
    path, folder included. Scoring the filename alone was the root cause of most
    wrong downloads: a file named exactly right still turns out to be a live
    recording or an orchestral cover, and only its folder says so.
    """

    min_title_ratio: float = Field(default=0.45, ge=0.0, le=1.0)

    prefer_lossless: bool = True
    """Rank lossless above lossy, whatever the lossy file's bitrate.

    Turn this off to judge results on availability and name match alone, which
    is what you want when a smaller file beats a better one.
    """

    require_lossless: bool = False
    """Discard lossy results outright rather than ranking them lower."""

    min_bitrate: int | None = Field(default=None, ge=0)
    """Discard lossy results below this bitrate. Lossless is never discarded."""
    reject_terms: tuple[str, ...] = ("karaoke", "instrumental", "off vocal", "nightcore")
    penalise_terms: tuple[str, ...] = (
        "live",
        "remix",
        "cover",
        "symphony",
        "orchestra",
        "orchestral",
        "medley",
        "acoustic",
        "sped up",
        "8 bit",
    )
    penalty_per_term: int = Field(default=2_000_000, ge=0)
    duration_tolerance_seconds: float = Field(default=12.0, ge=0)


class VerifySettings(Section):
    """Tuning for the transcoded-lossless check."""

    steep_db_per_khz: float = Field(default=25.0, gt=0)
    noise_floor_db: float = Field(default=-50.0, lt=0)
    near_nyquist_margin_hz: int = Field(default=1500, ge=0)
    probe_seconds: float = Field(default=20.0, gt=0)
    probe_count: int = Field(default=4, ge=1)


class ArtworkSettings(Section):
    """Cover-art lookup and the guard against embedding the wrong artist's art."""

    providers: tuple[str, ...] = ("itunes", "coverartarchive")
    min_bytes: int = Field(default=3000, ge=0)
    require_artist_match: bool = True
    artist_match_threshold: float = Field(default=0.8, ge=0.0, le=1.0)
    request_timeout_seconds: float = Field(default=15.0, gt=0)
    preferred_size: int = Field(default=1200, ge=100)
    min_request_interval_seconds: float | None = Field(default=None, ge=0)
    """Override the pacing between requests to every provider.

    Unset by default, which leaves each provider on its own published limit --
    one second for MusicBrainz, three for iTunes. Setting this to 0 disables
    pacing entirely, which will get you throttled on a large run.
    """

    contact: str = ""
    """A URL or email address identifying you to artwork providers.

    MusicBrainz asks callers to identify themselves and throttles those that do
    not. Optional, and empty by default: nothing is sent that you did not set.
    """


class GenreSettings(Section):
    map_path: Path = Path("genres.toml")


class Settings(BaseSettings):
    """The complete resolved configuration."""

    model_config = SettingsConfigDict(
        env_prefix="STK_",
        env_nested_delimiter="__",
        extra="forbid",
        nested_model_default_partial_update=True,
    )

    library: LibrarySettings = LibrarySettings()
    index: IndexSettings = Field(default_factory=IndexSettings)
    review: ReviewSettings = Field(default_factory=ReviewSettings)
    matching: MatchingSettings = MatchingSettings()
    slskd: SlskdSettings = SlskdSettings()
    ranking: RankingSettings = RankingSettings()
    verify: VerifySettings = VerifySettings()
    artwork: ArtworkSettings = ArtworkSettings()
    genres: GenreSettings = GenreSettings()

    source_path: Path | None = Field(default=None, exclude=True)

    @classmethod
    def settings_customise_sources(
        cls,
        settings_cls: type[BaseSettings],
        init_settings: PydanticBaseSettingsSource,
        env_settings: PydanticBaseSettingsSource,
        dotenv_settings: PydanticBaseSettingsSource,
        file_secret_settings: PydanticBaseSettingsSource,
    ) -> tuple[PydanticBaseSettingsSource, ...]:
        toml_file = settings_cls.model_config.get("toml_file")
        sources: list[PydanticBaseSettingsSource] = [init_settings, env_settings, dotenv_settings]
        if toml_file:
            sources.append(TomlConfigSettingsSource(settings_cls))
        sources.append(file_secret_settings)
        return tuple(sources)

    def resolved_library_root(self) -> Path:
        """The library root as an absolute path, relative to the config file."""
        return self._resolve(self.library.root)

    def resolved_genre_map_path(self) -> Path:
        return self._resolve(self.genres.map_path)

    def _resolve(self, path: Path) -> Path:
        if path.is_absolute():
            return path
        base = self.source_path.parent if self.source_path else Path.cwd()
        return (base / path).resolve()


def load_settings(config_path: Path | None = None, **overrides: Any) -> Settings:
    """Build the settings from the config file, the environment and ``overrides``.

    ``config_path`` is discovered when not given; passing a path that does not
    exist is an error, because it means a ``--config`` flag pointed at nothing.
    """
    if config_path is not None and not config_path.is_file():
        raise ConfigError(f"config file not found: {config_path}")

    path = config_path or find_config_file()
    supplied = {key: value for key, value in overrides.items() if value is not None}

    class _Configured(Settings):
        model_config = SettingsConfigDict(
            env_prefix="STK_",
            env_nested_delimiter="__",
            extra="forbid",
            nested_model_default_partial_update=True,
            toml_file=path,
        )

    try:
        settings = _Configured(**supplied)
    except Exception as error:
        location = path or "<defaults>"
        raise ConfigError(f"invalid configuration ({location}): {error}") from error

    return settings.model_copy(update={"source_path": path, "slskd": _with_api_key(settings.slskd)})


def _with_api_key(slskd: SlskdSettings) -> SlskdSettings:
    """Apply the bare ``SLSKD_API_KEY`` variable, the conventional name for it.

    ``STK_SLSKD__API_KEY`` already works through the normal environment source;
    this additionally accepts the name slskd itself documents, so one exported
    variable serves both the daemon and this client.
    """
    if slskd.api_key is not None:
        return slskd
    key = os.environ.get(API_KEY_ENV_VAR)
    return slskd.model_copy(update={"api_key": SecretStr(key)}) if key else slskd


def render_toml(settings: Settings) -> str:
    """Serialise settings back to TOML, with secrets omitted."""
    data = settings.model_dump(mode="json", exclude={"source_path"})
    data["slskd"].pop("api_key", None)
    return tomli_w.dumps(_drop_nulls(data))


def write_config(path: Path, settings: Settings) -> Path:
    """Write a config file, refusing to clobber one that already exists."""
    if path.exists():
        raise ConfigError(f"config file already exists: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    header = (
        "# SoulseekTools configuration.\n"
        "# Secrets are never read from this file: export SLSKD_API_KEY instead.\n\n"
    )
    path.write_text(header + render_toml(settings), encoding="utf-8")
    return path


def read_toml(path: Path) -> dict[str, Any]:
    """Parse a TOML file, reporting a malformed one as a configuration error."""
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ConfigError(f"cannot read {path}: {error}") from error


def _drop_nulls(data: dict[str, Any]) -> dict[str, Any]:
    cleaned: dict[str, Any] = {}
    for key, value in data.items():
        if value is None:
            continue
        cleaned[key] = _drop_nulls(value) if isinstance(value, dict) else value
    return cleaned
