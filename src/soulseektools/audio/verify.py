"""Running the spectral check against files on disk."""

from __future__ import annotations

from typing import TYPE_CHECKING

from soulseektools.audio import ffmpeg, spectral
from soulseektools.config import VerifySettings
from soulseektools.errors import ExternalToolError
from soulseektools.models import Verdict, VerifyResult

if TYPE_CHECKING:
    from pathlib import Path


def verify_file(path: Path, settings: VerifySettings | None = None) -> VerifyResult:
    """Decide whether a lossless container really holds lossless audio.

    Decoding or probing failures are reported as :attr:`Verdict.ERROR` rather
    than raised, so one unreadable file cannot abort a library-wide run.
    """
    options = settings or VerifySettings()

    try:
        stream = ffmpeg.probe(path)
        chunks = ffmpeg.decode_probes(
            path,
            sample_rate=stream.sample_rate,
            total_seconds=stream.duration_seconds,
            window_seconds=options.probe_seconds,
            probes=options.probe_count,
        )
    except ExternalToolError as error:
        return VerifyResult(path=path, verdict=Verdict.ERROR, detail=str(error))

    analysis = spectral.analyse(
        chunks,
        stream.sample_rate,
        noise_floor_db=options.noise_floor_db,
        steep_db_per_khz=options.steep_db_per_khz,
        near_nyquist_margin_hz=options.near_nyquist_margin_hz,
    )
    if analysis is None:
        return VerifyResult(
            path=path,
            verdict=Verdict.ERROR,
            detail="no audible content to analyse",
            sample_rate=stream.sample_rate,
            bit_depth=stream.bit_depth,
            codec=stream.codec,
        )

    return VerifyResult(
        path=path,
        verdict=analysis.verdict,
        detail=analysis.detail,
        sample_rate=stream.sample_rate,
        bit_depth=stream.bit_depth,
        codec=stream.codec,
        cutoff_hz=round(analysis.cutoff_hz),
        nyquist_hz=round(stream.nyquist_hz),
        steepness_db_per_khz=round(analysis.steepness_db_per_khz, 1),
    )
