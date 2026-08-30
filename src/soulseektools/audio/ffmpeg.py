"""Thin wrappers around ffmpeg and ffprobe.

Every subprocess this package spawns lives here, so the rest of the audio code
works on numpy arrays and can be tested without either binary installed.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass
from typing import TYPE_CHECKING

import numpy as np

from soulseektools.errors import ExternalToolError

if TYPE_CHECKING:
    from pathlib import Path

    from numpy.typing import NDArray

_TIMEOUT_SECONDS = 300


@dataclass(frozen=True, slots=True)
class StreamInfo:
    """The audio stream properties the checks depend on."""

    sample_rate: int
    codec: str
    channels: int
    bit_depth: int | None = None
    duration_seconds: float | None = None

    @property
    def nyquist_hz(self) -> float:
        return self.sample_rate / 2


def ffmpeg_path() -> str | None:
    return shutil.which("ffmpeg")


def ffprobe_path() -> str | None:
    return shutil.which("ffprobe")


def require(tool: str) -> str:
    """Return the path to ``tool``, or explain what to install."""
    resolved = shutil.which(tool)
    if resolved is None:
        raise ExternalToolError(f"{tool} is not on PATH; install ffmpeg to use this command")
    return resolved


def _run(command: list[str]) -> subprocess.CompletedProcess[bytes]:
    try:
        return subprocess.run(command, capture_output=True, check=True, timeout=_TIMEOUT_SECONDS)
    except subprocess.CalledProcessError as error:
        detail = error.stderr.decode("utf-8", "replace").strip().splitlines()
        raise ExternalToolError(detail[-1] if detail else f"{command[0]} failed") from error
    except subprocess.TimeoutExpired as error:
        raise ExternalToolError(f"{command[0]} timed out") from error


def probe(path: Path) -> StreamInfo:
    """Read the audio stream's properties."""
    result = _run(
        [
            require("ffprobe"),
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            str(path),
        ]
    )
    data = json.loads(result.stdout)
    stream = next((s for s in data.get("streams", []) if s.get("codec_type") == "audio"), None)
    if stream is None:
        raise ExternalToolError(f"{path}: no audio stream")

    raw_depth = stream.get("bits_per_raw_sample") or stream.get("bits_per_sample")
    duration = data.get("format", {}).get("duration") or stream.get("duration")

    return StreamInfo(
        sample_rate=int(stream["sample_rate"]),
        codec=str(stream.get("codec_name", "")),
        channels=int(stream.get("channels", 1)),
        bit_depth=int(raw_depth) if raw_depth and int(raw_depth) else None,
        duration_seconds=float(duration) if duration else None,
    )


def decode(
    path: Path,
    *,
    sample_rate: int,
    start_seconds: float = 0.0,
    duration_seconds: float | None = None,
) -> NDArray[np.float32]:
    """Decode a window of the file to mono float32 samples."""
    command = [require("ffmpeg"), "-v", "quiet"]
    if start_seconds:
        command += ["-ss", f"{start_seconds:.3f}"]
    if duration_seconds:
        command += ["-t", f"{duration_seconds:.3f}"]
    command += ["-i", str(path), "-f", "f32le", "-ac", "1", "-ar", str(sample_rate), "-"]

    return np.frombuffer(_run(command).stdout, dtype=np.float32)


def decode_probes(
    path: Path,
    *,
    sample_rate: int,
    total_seconds: float | None,
    window_seconds: float = 20.0,
    probes: int = 4,
) -> list[NDArray[np.float32]]:
    """Decode several windows spread across the track rather than the whole file.

    A handful of samples is enough to characterise the spectrum, and decoding a
    few minutes of every file in a library is not.
    """
    chunks = []
    for start, length in _probe_windows(total_seconds, window_seconds, probes):
        chunk = decode(path, sample_rate=sample_rate, start_seconds=start, duration_seconds=length)
        if chunk.size:
            chunks.append(chunk)
    return chunks


def render_spectrogram(path: Path, destination: Path) -> Path:
    """Write a spectrogram image, for eyeballing a flagged file."""
    _run(
        [
            require("ffmpeg"),
            "-v",
            "quiet",
            "-y",
            "-i",
            str(path),
            "-lavfi",
            "showspectrumpic=s=1024x512:legend=1",
            str(destination),
        ]
    )
    return destination


def _probe_windows(
    total_seconds: float | None, window_seconds: float, probes: int
) -> list[tuple[float, float]]:
    if not total_seconds or total_seconds <= window_seconds * probes:
        return [(0.0, max(total_seconds - 0.5, 1.0) if total_seconds else window_seconds)]

    # The first and last tenth are skipped: intros and fade-outs are quiet and
    # would drag the averaged spectrum toward the noise floor.
    margin = total_seconds * 0.1
    span = total_seconds - 2 * margin
    step = span / max(probes - 1, 1)
    return [(margin + step * index, window_seconds) for index in range(probes)]
