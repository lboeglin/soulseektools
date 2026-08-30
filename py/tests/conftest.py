"""Shared fixtures.

Audio fixtures are generated with ffmpeg rather than committed as binaries, so
the suite exercises real containers written by an independent encoder instead of
files this package produced itself.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    from collections.abc import Callable, Mapping

FFMPEG = shutil.which("ffmpeg")

_CODEC_BY_SUFFIX = {
    ".flac": ["-c:a", "flac"],
    ".mp3": ["-c:a", "libmp3lame", "-b:a", "192k"],
    ".m4a": ["-c:a", "aac", "-b:a", "192k"],
    ".ogg": ["-c:a", "libvorbis", "-q:a", "4"],
    ".opus": ["-c:a", "libopus", "-b:a", "128k"],
    ".wav": ["-c:a", "pcm_s16le"],
}


def encode(
    path: Path,
    *,
    seconds: float = 1.0,
    frequency: int = 440,
    sample_rate: int = 44100,
    modulation_hz: float = 0.0,
    noise: bool = False,
    metadata: Mapping[str, str] | None = None,
) -> Path:
    """Write a short test tone to ``path``, tagged with ``metadata``.

    ``modulation_hz`` adds a slow amplitude sweep, giving the tone the loudness
    dynamics that fingerprinting needs and a steady sine lacks. ``noise`` writes
    broadband noise instead, which is what the spectral check needs: a pure tone
    has no high-frequency content to judge a cutoff by.
    """
    if FFMPEG is None:
        pytest.skip("ffmpeg is not on PATH")

    path.parent.mkdir(parents=True, exist_ok=True)
    if noise:
        source = f"anoisesrc=d={seconds}:c=white:r={sample_rate}"
    elif modulation_hz:
        source = (
            f"aevalsrc=0.5*sin(2*PI*{frequency}*t)*"
            f"(0.55+0.45*sin(2*PI*{modulation_hz}*t)):"
            f"d={seconds}:s={sample_rate}"
        )
    else:
        source = f"sine=frequency={frequency}:duration={seconds}:sample_rate={sample_rate}"

    command = [
        FFMPEG,
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        source,
        "-ac",
        "2",
        *_CODEC_BY_SUFFIX[path.suffix.lower()],
    ]
    for key, value in (metadata or {}).items():
        command += ["-metadata", f"{key}={value}"]
    command.append(str(path))

    subprocess.run(command, check=True, capture_output=True)
    return path


@pytest.fixture(scope="session")
def requires_ffmpeg() -> None:
    if FFMPEG is None:
        pytest.skip("ffmpeg is not on PATH")


@pytest.fixture
def encode_audio(requires_ffmpeg: None) -> Callable[..., Path]:
    return encode


@pytest.fixture
def library_root(tmp_path: Path, encode_audio: Callable[..., Path]) -> Path:
    """A small library laid out the way the real one is: ``<root>/<artist>/...``."""
    root = tmp_path / "Music"
    encode_audio(
        root / "DECO_27" / "01 - Roki.flac",
        metadata={"artist": "DECO*27", "title": "Roki", "album": "Ghost", "genre": "Vocaloid"},
    )
    encode_audio(
        root / "DECO_27" / "02 - Yowamushi Montblanc.mp3",
        metadata={"artist": "DECO*27", "title": "Yowamushi Montblanc"},
    )
    encode_audio(root / "ZUN" / "Nested" / "15 Heian Alien.flac", metadata={"artist": "ZUN"})
    encode_audio(root / "beatMARIO" / "Night of Knights.m4a")
    return root
