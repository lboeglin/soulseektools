"""Acoustic fingerprints, for finding duplicates that share no metadata.

Grouping by duration and file size -- what the original script did -- produces
coincidental matches between unrelated tracks of similar length and bitrate. A
fingerprint compares the audio itself.

Two backends, chosen at runtime:

- **chromaprint** (``fpcalc``), the same fingerprint AcoustID uses. Compared
  locally by bit error rate, so no web service and no API key are involved.
- **loudness envelope**, the fallback when ``fpcalc`` is absent: the track's
  per-frame loudness curve, compared by correlation. It survives re-encoding and
  differing bitrates, which a byte hash does not, while needing only ffmpeg.

Fingerprints of different kinds are never compared.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass
from enum import StrEnum
from typing import TYPE_CHECKING, Any

import numpy as np

from soulseektools.audio import ffmpeg
from soulseektools.errors import ExternalToolError

if TYPE_CHECKING:
    from pathlib import Path

    from numpy.typing import NDArray

ENVELOPE_SAMPLE_RATE = 8000
ENVELOPE_FRAME_SECONDS = 0.25
MAX_ALIGNMENT_FRAMES = 8
"""How far two fingerprints may be shifted against each other when comparing.

Rips of the same track routinely differ by a second or two of leading silence.
"""

_FPCALC_TIMEOUT_SECONDS = 60
_MIN_COMPARABLE_FRAMES = 8
_FLAT_ENVELOPE_DB = 1.0
_NO_EVIDENCE = 0.5


class FingerprintKind(StrEnum):
    CHROMAPRINT = "chromaprint"
    ENVELOPE = "envelope"


@dataclass(frozen=True, slots=True)
class Fingerprint:
    """A comparable summary of a track's audio."""

    kind: FingerprintKind
    values: tuple[int, ...] | tuple[float, ...]
    duration_seconds: float | None = None

    def __len__(self) -> int:
        return len(self.values)


def fpcalc_path() -> str | None:
    return shutil.which("fpcalc")


def available_kind() -> FingerprintKind:
    """Which backend this machine will use."""
    return FingerprintKind.CHROMAPRINT if fpcalc_path() else FingerprintKind.ENVELOPE


def fingerprint(path: Path, kind: FingerprintKind | None = None) -> Fingerprint:
    """Compute a fingerprint using the requested backend, or the best available."""
    chosen = kind or available_kind()
    if chosen is FingerprintKind.CHROMAPRINT:
        return _chromaprint(path)
    return _envelope(path)


def similarity(left: Fingerprint, right: Fingerprint) -> float:
    """How alike two fingerprints are, in ``[0, 1]``."""
    if left.kind is not right.kind:
        raise ValueError(f"cannot compare {left.kind} with {right.kind}")
    if not len(left) or not len(right):
        return 0.0
    if left.kind is FingerprintKind.CHROMAPRINT:
        return _chromaprint_similarity(left, right)
    return _envelope_similarity(left, right)


def _chromaprint(path: Path) -> Fingerprint:
    binary = fpcalc_path()
    if binary is None:
        raise ExternalToolError("fpcalc is not on PATH; install chromaprint to use it")
    try:
        result = subprocess.run(
            [binary, "-raw", "-json", str(path)],
            capture_output=True,
            check=True,
            timeout=_FPCALC_TIMEOUT_SECONDS,
        )
    except subprocess.CalledProcessError as error:
        raise ExternalToolError(error.stderr.decode("utf-8", "replace").strip()) from error
    except subprocess.TimeoutExpired as error:
        raise ExternalToolError("fpcalc timed out") from error

    payload = json.loads(result.stdout)
    return Fingerprint(
        kind=FingerprintKind.CHROMAPRINT,
        values=tuple(int(value) for value in payload.get("fingerprint", ())),
        duration_seconds=float(payload["duration"]) if payload.get("duration") else None,
    )


def _envelope(path: Path) -> Fingerprint:
    stream = ffmpeg.probe(path)
    samples = ffmpeg.decode(path, sample_rate=ENVELOPE_SAMPLE_RATE)
    return Fingerprint(
        kind=FingerprintKind.ENVELOPE,
        values=tuple(envelope_of(samples).tolist()),
        duration_seconds=stream.duration_seconds,
    )


def envelope_of(
    samples: NDArray[np.float32], sample_rate: int = ENVELOPE_SAMPLE_RATE
) -> NDArray[np.float64]:
    """The per-frame loudness curve, in dB and mean-centred.

    Centring removes the overall level, so the same track ripped at two different
    volumes still produces the same curve.
    """
    frame = max(1, int(sample_rate * ENVELOPE_FRAME_SECONDS))
    usable = samples[: samples.size - samples.size % frame]
    if usable.size < frame:
        return np.zeros(0)

    rms = np.sqrt((usable.astype(np.float64).reshape(-1, frame) ** 2).mean(axis=1))
    db = 20 * np.log10(rms + 1e-9)
    return db - db.mean()


def _chromaprint_similarity(left: Fingerprint, right: Fingerprint) -> float:
    best = 0.0
    first = np.array(left.values, dtype=np.uint32)
    second = np.array(right.values, dtype=np.uint32)

    for offset in range(-MAX_ALIGNMENT_FRAMES, MAX_ALIGNMENT_FRAMES + 1):
        a, b = _align(first, second, offset)
        if a.size < _MIN_COMPARABLE_FRAMES:
            continue
        differing_bits = np.unpackbits(np.bitwise_xor(a, b).astype(">u4").view(np.uint8)).sum()
        best = max(best, 1.0 - differing_bits / (a.size * 32))
    return best


def _envelope_similarity(left: Fingerprint, right: Fingerprint) -> float:
    first = np.array(left.values, dtype=np.float64)
    second = np.array(right.values, dtype=np.float64)

    # Audio with no dynamics -- a sustained tone, a drone -- has a flat curve
    # carrying no shape to correlate, and correlating the residual numerical
    # noise would invent a confident answer. Say "no evidence" instead, a score
    # deliberately too low to group files by and too high to rule them out.
    flat = [curve.std() < _FLAT_ENVELOPE_DB for curve in (first, second)]
    if all(flat):
        return _NO_EVIDENCE
    if any(flat):
        return 0.0

    best = 0.0
    for offset in range(-MAX_ALIGNMENT_FRAMES, MAX_ALIGNMENT_FRAMES + 1):
        a, b = _align(first, second, offset)
        if a.size < _MIN_COMPARABLE_FRAMES or a.std() == 0 or b.std() == 0:
            continue
        correlation = float(((a - a.mean()) * (b - b.mean())).mean() / (a.std() * b.std()))
        best = max(best, (correlation + 1.0) / 2.0)
    return best


def _align(
    left: np.ndarray[Any, Any], right: np.ndarray[Any, Any], offset: int
) -> tuple[np.ndarray[Any, Any], np.ndarray[Any, Any]]:
    if offset >= 0:
        shifted, other = left[offset:], right
    else:
        shifted, other = left, right[-offset:]
    length = min(shifted.size, other.size)
    return shifted[:length], other[:length]
