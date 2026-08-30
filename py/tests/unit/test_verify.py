"""End-to-end checks of the transcode detector against real encoded files.

`tests/unit/test_spectral.py` covers the classifier on synthesised spectra; this
covers the path through ffprobe and ffmpeg, including a genuine MP3 re-encoded
as FLAC -- the exact deception the check exists to catch.
"""

import subprocess

import pytest

from soulseektools.audio import ffmpeg
from soulseektools.audio.verify import verify_file
from soulseektools.config import VerifySettings
from soulseektools.errors import ExternalToolError
from soulseektools.models import Verdict
from tests.conftest import FFMPEG


@pytest.fixture
def noise_source(tmp_path, requires_ffmpeg):
    """Broadband noise: content all the way to Nyquist, like a real recording."""
    path = tmp_path / "source.flac"
    subprocess.run(
        [
            FFMPEG,
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=d=8:c=white:r=44100",
            # Stereo, because a bitrate is shared across the pair: the same
            # nominal 128 kbps applied to one channel barely low-passes at all.
            "-ac",
            "2",
            str(path),
        ],
        check=True,
        capture_output=True,
    )
    return path


def transcode_to_flac(source, tmp_path, name, bitrate):
    """Encode to MP3 and back to FLAC -- a fake lossless file."""
    lossy = tmp_path / f"{name}.mp3"
    subprocess.run(
        [
            FFMPEG,
            "-v",
            "error",
            "-y",
            "-i",
            str(source),
            "-c:a",
            "libmp3lame",
            "-b:a",
            bitrate,
            "-ac",
            "2",
            str(lossy),
        ],
        check=True,
        capture_output=True,
    )
    faked = tmp_path / f"{name}.flac"
    subprocess.run(
        [FFMPEG, "-v", "error", "-y", "-i", str(lossy), "-c:a", "flac", str(faked)],
        check=True,
        capture_output=True,
    )
    return faked


class TestVerifyFile:
    def test_genuine_lossless_audio_passes(self, noise_source):
        result = verify_file(noise_source)

        assert result.verdict is Verdict.OK
        assert result.codec == "flac"
        assert result.sample_rate == 44100

    @pytest.mark.parametrize("bitrate", ["128k", "192k", "320k"])
    def test_an_mp3_re_encoded_as_flac_is_caught(self, noise_source, tmp_path, bitrate):
        faked = transcode_to_flac(noise_source, tmp_path, f"fake{bitrate}", bitrate)

        result = verify_file(faked)

        assert result.verdict is Verdict.FAKE
        assert result.cutoff_hz is not None
        assert result.cutoff_hz < result.nyquist_hz

    def test_the_reported_cutoff_rises_with_the_source_bitrate(self, noise_source, tmp_path):
        low = verify_file(transcode_to_flac(noise_source, tmp_path, "low", "128k"))
        high = verify_file(transcode_to_flac(noise_source, tmp_path, "high", "320k"))

        assert low.cutoff_hz < high.cutoff_hz

    def test_the_verdict_explains_itself(self, noise_source, tmp_path):
        result = verify_file(transcode_to_flac(noise_source, tmp_path, "explained", "128k"))

        assert "transcoded" in result.detail
        assert result.steepness_db_per_khz is not None

    def test_a_silent_file_is_reported_as_unanalysable_not_as_lossless(self, tmp_path):
        silent = tmp_path / "silent.flac"
        subprocess.run(
            [FFMPEG, "-v", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:d=5", str(silent)],
            check=True,
            capture_output=True,
        )

        result = verify_file(silent)

        assert result.verdict is Verdict.ERROR
        assert "no audible content" in result.detail

    def test_an_unreadable_file_is_reported_rather_than_raised(self, tmp_path):
        broken = tmp_path / "broken.flac"
        broken.write_bytes(b"not audio at all")

        result = verify_file(broken)

        assert result.verdict is Verdict.ERROR
        assert result.path == broken

    def test_a_stricter_threshold_can_flag_more(self, noise_source):
        strict = verify_file(noise_source, VerifySettings(steep_db_per_khz=0.1))

        assert strict.verdict is Verdict.OK  # content still reaches Nyquist

    def test_probe_settings_are_honoured(self, noise_source):
        result = verify_file(noise_source, VerifySettings(probe_seconds=2.0, probe_count=1))

        assert result.verdict is Verdict.OK


class TestFfmpegWrappers:
    def test_probe_reports_the_stream_properties(self, noise_source):
        stream = ffmpeg.probe(noise_source)

        assert stream.sample_rate == 44100
        assert stream.codec == "flac"
        assert stream.nyquist_hz == 22050
        assert stream.duration_seconds == pytest.approx(8.0, abs=0.2)

    def test_probing_a_non_audio_file_is_an_explicit_error(self, tmp_path):
        path = tmp_path / "notes.txt"
        path.write_text("hello", encoding="utf-8")

        with pytest.raises(ExternalToolError):
            ffmpeg.probe(path)

    def test_decode_returns_mono_float_samples(self, noise_source):
        samples = ffmpeg.decode(noise_source, sample_rate=8000, duration_seconds=1.0)

        assert samples.size == pytest.approx(8000, rel=0.05)
        assert float(samples.min()) >= -1.0 and float(samples.max()) <= 1.0

    def test_decode_probes_samples_several_windows_of_a_long_track(self, noise_source):
        chunks = ffmpeg.decode_probes(
            noise_source, sample_rate=8000, total_seconds=8.0, window_seconds=1.0, probes=4
        )

        assert len(chunks) == 4

    def test_decode_probes_reads_a_short_track_in_one_window(self, noise_source):
        chunks = ffmpeg.decode_probes(
            noise_source, sample_rate=8000, total_seconds=8.0, window_seconds=20.0, probes=4
        )

        assert len(chunks) == 1

    def test_requiring_a_missing_tool_explains_what_to_install(self, monkeypatch):
        monkeypatch.setattr(ffmpeg.shutil, "which", lambda _name: None)

        with pytest.raises(ExternalToolError, match="install ffmpeg"):
            ffmpeg.require("ffprobe")

    def test_render_spectrogram_writes_an_image(self, noise_source, tmp_path):
        destination = ffmpeg.render_spectrogram(noise_source, tmp_path / "spectrum.png")

        assert destination.is_file()
        assert destination.stat().st_size > 0
