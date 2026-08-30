"""Detecting lossless files that were really transcoded from a lossy source.

A genuine CD-sourced lossless file carries content out to near the Nyquist
frequency. An MP3 or AAC re-encoded as FLAC keeps the lossy encoder's low-pass
baked in, and the *shape* of that edge is what gives it away:

- a lossy encoder's cutoff is a brickwall -- tens of dB lost over a few hundred
  hertz, landing at one of a handful of bitrate-specific frequencies
- a loud modern master rolls off gradually over several kilohertz

This distinction is the whole point. "Is there content above 16 kHz" is not a
usable test on its own: heavily limited masters roll off early and would be
flagged wholesale. Steepness is measured by scanning for the sharpest slope
across a narrow window straddling the cutoff, because averaging one wide span
dilutes a real, narrow cliff until it looks gradual.

Everything here operates on numpy arrays, so the classifier is testable against
synthesised signals with known cutoffs and needs no decoder.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

import numpy as np

from soulseektools.models import Verdict

if TYPE_CHECKING:
    from collections.abc import Sequence

    from numpy.typing import NDArray

BAND_WIDTH_HZ = 150
DEFAULT_FFT_SIZE = 8192
DEFAULT_NOISE_FLOOR_DB = -50.0
DEFAULT_STEEP_DB_PER_KHZ = 25.0
DEFAULT_NEAR_NYQUIST_MARGIN_HZ = 1500
_STEEPNESS_WINDOW_HZ = 800
# Below this averaged magnitude there is no signal to characterise. Without the
# guard, normalising a silent spectrum to its own peak makes every band read as
# 0 dB, and a silent file is confidently reported as genuinely lossless.
_SILENCE_MAGNITUDE = 1e-9
MIN_ANALYSABLE_CUTOFF_HZ = 8000
"""Below this, there is no cutoff to judge.

Every lossy encoder's low-pass sits well above this, so content stopping
lower means the file is not ordinary music -- a tone, a field recording, an
early transfer -- and saying it looks genuinely lossless would be claiming
more than the spectrum supports.
"""

KNOWN_LOSSY_CUTOFFS: tuple[tuple[int, int, str], ...] = (
    (15000, 16800, "~128kbps source"),
    (16800, 19000, "~160-192kbps source"),
    (19000, 20600, "~256-320kbps source"),
)
"""Frequency bands where common encoders place their low-pass, and what they imply.

Measured from LAME stereo transcodes, which land at roughly 16.6, 18.8 and
20.1 kHz -- see ``tests/unit/test_verify.py``. The bands are wider than those
nominal values and deliberately contiguous: encoders differ, and band averaging
quantises the measured cutoff. A gap between them would only downgrade a real
transcode to SUSPECT for being 150 Hz off.
"""


@dataclass(frozen=True, slots=True)
class Bands:
    """An averaged spectrum, in fixed-width frequency bands relative to the peak."""

    lower_edges_hz: NDArray[np.float64]
    db: NDArray[np.float64]

    def __len__(self) -> int:
        return int(self.db.size)


@dataclass(frozen=True, slots=True)
class SpectralAnalysis:
    """Where the audio stops, how abruptly, and what that means."""

    cutoff_hz: float
    steepness_db_per_khz: float
    verdict: Verdict
    detail: str


def average_spectrum_db(
    chunks: Sequence[NDArray[np.float32]],
    sample_rate: int,
    fft_size: int = DEFAULT_FFT_SIZE,
) -> tuple[NDArray[np.float64], NDArray[np.float64]] | None:
    """Average the magnitude spectrum over every frame, normalised to its peak.

    Returns ``None`` when the input holds no complete frame or no audible signal,
    which is the honest answer for a file too short or too quiet to characterise.
    """
    window = np.hanning(fft_size)
    total: NDArray[np.float64] | None = None
    frames = 0

    for chunk in chunks:
        for start in range(0, chunk.size - fft_size + 1, fft_size // 2):
            magnitude = np.abs(np.fft.rfft(chunk[start : start + fft_size] * window))
            total = magnitude if total is None else total + magnitude
            frames += 1

    if total is None or frames == 0:
        return None

    averaged = total / frames
    if averaged.max() < _SILENCE_MAGNITUDE:
        return None

    frequencies = np.fft.rfftfreq(fft_size, d=1.0 / sample_rate)
    db = 20 * np.log10(averaged + 1e-12)
    return frequencies, db - db.max()


def band_average(
    frequencies: NDArray[np.float64],
    db: NDArray[np.float64],
    band_width_hz: int = BAND_WIDTH_HZ,
) -> Bands:
    """Collapse the spectrum into fixed-width bands, smoothing per-bin noise."""
    edges = np.arange(0.0, float(frequencies[-1]), band_width_hz)
    indices = np.digitize(frequencies, edges) - 1

    averages = np.full(edges.size, -999.0)
    for band in range(edges.size):
        selected = db[indices == band]
        if selected.size:
            averages[band] = selected.mean()
    return Bands(lower_edges_hz=edges, db=averages)


def find_cutoff(bands: Bands, noise_floor_db: float = DEFAULT_NOISE_FLOOR_DB) -> float:
    """The highest frequency still carrying audible content."""
    above = np.flatnonzero(bands.db > noise_floor_db)
    return float(bands.lower_edges_hz[above[-1]]) if above.size else 0.0


def cliff_steepness(bands: Bands, cutoff_hz: float, window_hz: int = _STEEPNESS_WINDOW_HZ) -> float:
    """Steepest local slope, in dB per kHz, across the region around the cutoff.

    Scanning for the sharpest narrow slope rather than measuring one wide span
    is what separates a brickwall from a roll-off: averaged over several
    kilohertz, both look similar.
    """
    low = int(np.searchsorted(bands.lower_edges_hz, max(cutoff_hz - window_hz * 2, 0.0)))
    high = int(np.searchsorted(bands.lower_edges_hz, cutoff_hz + window_hz))
    high = min(high, len(bands) - 1)
    step = max(1, window_hz // BAND_WIDTH_HZ)
    if high <= low + step:
        return 0.0

    drops = bands.db[low : high - step] - bands.db[low + step : high]
    per_khz = drops / (step * BAND_WIDTH_HZ / 1000.0)
    return float(max(per_khz.max(), 0.0))


def matched_bitrate(cutoff_hz: float) -> str | None:
    """The source bitrate a cutoff at this frequency implies, if any."""
    for low, high, label in KNOWN_LOSSY_CUTOFFS:
        if low <= cutoff_hz <= high:
            return label
    return None


def classify(
    cutoff_hz: float,
    steepness_db_per_khz: float,
    sample_rate: int,
    *,
    steep_db_per_khz: float = DEFAULT_STEEP_DB_PER_KHZ,
    near_nyquist_margin_hz: int = DEFAULT_NEAR_NYQUIST_MARGIN_HZ,
) -> tuple[Verdict, str]:
    """Judge a cutoff and its sharpness."""
    nyquist = sample_rate / 2

    if cutoff_hz >= nyquist - near_nyquist_margin_hz:
        return Verdict.OK, "content extends to near the Nyquist limit -- genuinely lossless"

    if cutoff_hz < MIN_ANALYSABLE_CUTOFF_HZ:
        return (
            Verdict.ERROR,
            f"content stops at {cutoff_hz:.0f}Hz, below any encoder cutoff "
            "-- too band-limited to judge",
        )

    is_steep = steepness_db_per_khz >= steep_db_per_khz
    label = matched_bitrate(cutoff_hz)
    measured = f"{cutoff_hz:.0f}Hz ({steepness_db_per_khz:.0f}dB/kHz)"

    # A hi-res container whose content does not even fill the CD range, cut off
    # sharply, is an upsample of a lossy file rather than a hi-res master.
    if is_steep and sample_rate >= 48000 and cutoff_hz < 20500:
        return (
            Verdict.FAKE,
            f"{sample_rate}Hz container but content stops sharply at {measured} "
            "-- upsampled, not hi-res",
        )
    if is_steep and label:
        return Verdict.FAKE, f"brickwall cutoff at {measured} matches a {label} -- transcoded"
    if is_steep:
        return (
            Verdict.SUSPECT,
            f"steep cutoff at {measured} matches no standard bitrate, "
            "but the sharpness looks artificial -- inspect manually",
        )
    return (
        Verdict.OK,
        f"gradual roll-off from {measured} -- consistent with mastering, not a transcode",
    )


def analyse(
    chunks: Sequence[NDArray[np.float32]],
    sample_rate: int,
    *,
    noise_floor_db: float = DEFAULT_NOISE_FLOOR_DB,
    steep_db_per_khz: float = DEFAULT_STEEP_DB_PER_KHZ,
    near_nyquist_margin_hz: int = DEFAULT_NEAR_NYQUIST_MARGIN_HZ,
    fft_size: int = DEFAULT_FFT_SIZE,
) -> SpectralAnalysis | None:
    """Run the whole check over decoded samples."""
    spectrum = average_spectrum_db(chunks, sample_rate, fft_size)
    if spectrum is None:
        return None

    bands = band_average(*spectrum)
    cutoff = find_cutoff(bands, noise_floor_db)
    steepness = cliff_steepness(bands, cutoff)
    verdict, detail = classify(
        cutoff,
        steepness,
        sample_rate,
        steep_db_per_khz=steep_db_per_khz,
        near_nyquist_margin_hz=near_nyquist_margin_hz,
    )
    return SpectralAnalysis(
        cutoff_hz=cutoff,
        steepness_db_per_khz=steepness,
        verdict=verdict,
        detail=detail,
    )
