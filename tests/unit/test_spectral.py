"""Regression tests for the transcode detector.

The original script was validated once, by hand, against a synthetic transcode.
These tests synthesise signals with known cutoffs and known edge shapes, so that
validation is permanent and covers every case the classifier distinguishes.
"""

import itertools

import numpy as np
import pytest

from soulseektools.audio.spectral import (
    KNOWN_LOSSY_CUTOFFS,
    analyse,
    average_spectrum_db,
    band_average,
    classify,
    cliff_steepness,
    find_cutoff,
    matched_bitrate,
)
from soulseektools.models import Verdict

SAMPLE_RATE = 44100
DURATION_SECONDS = 6.0


def noise(sample_rate: int = SAMPLE_RATE, seconds: float = DURATION_SECONDS) -> np.ndarray:
    generator = np.random.default_rng(20260830)
    return generator.standard_normal(int(sample_rate * seconds)).astype(np.float32)


def shape_spectrum(samples: np.ndarray, sample_rate: int, gain) -> np.ndarray:
    """Re-synthesise ``samples`` with a per-frequency gain applied."""
    spectrum = np.fft.rfft(samples)
    frequencies = np.fft.rfftfreq(samples.size, d=1.0 / sample_rate)
    shaped = np.fft.irfft(spectrum * gain(frequencies), n=samples.size)
    return (shaped / np.abs(shaped).max()).astype(np.float32)


def brickwall(cutoff_hz: float, sample_rate: int = SAMPLE_RATE, seconds=DURATION_SECONDS):
    """A lossy encoder's low-pass: full level, then nothing."""
    return shape_spectrum(
        noise(sample_rate, seconds),
        sample_rate,
        lambda frequencies: (frequencies < cutoff_hz).astype(float) + 1e-7,
    )


def gradual_rolloff(knee_hz: float, db_per_khz: float, sample_rate: int = SAMPLE_RATE):
    """A mastering roll-off: level falls away smoothly above the knee."""

    def gain(frequencies: np.ndarray) -> np.ndarray:
        above = np.maximum(frequencies - knee_hz, 0.0) / 1000.0
        return 10 ** (-db_per_khz * above / 20.0)

    return shape_spectrum(noise(sample_rate), sample_rate, gain)


def full_band(sample_rate: int = SAMPLE_RATE) -> np.ndarray:
    samples = noise(sample_rate)
    return (samples / np.abs(samples).max()).astype(np.float32)


class TestAverageSpectrum:
    def test_returns_frequencies_up_to_nyquist(self):
        result = average_spectrum_db([full_band()], SAMPLE_RATE)

        assert result is not None
        frequencies, db = result
        assert frequencies[-1] == pytest.approx(SAMPLE_RATE / 2)
        assert db.max() == pytest.approx(0.0)

    def test_returns_none_when_no_complete_frame_exists(self):
        assert average_spectrum_db([np.zeros(16, dtype=np.float32)], SAMPLE_RATE) is None

    def test_returns_none_for_no_input(self):
        assert average_spectrum_db([], SAMPLE_RATE) is None


class TestFindCutoff:
    @pytest.mark.parametrize("cutoff", [16000, 18000, 19500])
    def test_locates_a_brickwall_within_a_band_width(self, cutoff):
        spectrum = average_spectrum_db([brickwall(cutoff)], SAMPLE_RATE)
        assert spectrum is not None

        found = find_cutoff(band_average(*spectrum))

        assert found == pytest.approx(cutoff, abs=400)

    def test_full_band_audio_reaches_near_nyquist(self):
        spectrum = average_spectrum_db([full_band()], SAMPLE_RATE)
        assert spectrum is not None

        assert find_cutoff(band_average(*spectrum)) > SAMPLE_RATE / 2 - 1500

    def test_a_silent_file_is_not_mistaken_for_genuine_lossless(self):
        # Normalising a silent spectrum to its own peak puts every band at 0 dB,
        # which reads as content all the way to Nyquist unless silence is caught.
        silence = [np.zeros(SAMPLE_RATE * 2, dtype=np.float32)]

        assert average_spectrum_db(silence, SAMPLE_RATE) is None
        assert analyse(silence, SAMPLE_RATE) is None


class TestCliffSteepness:
    def test_a_brickwall_is_far_steeper_than_a_rolloff(self):
        wall = _steepness(brickwall(16000))
        roll = _steepness(gradual_rolloff(knee_hz=13000, db_per_khz=8))

        assert wall > 25.0
        assert roll < 25.0

    def test_a_narrow_cliff_is_not_diluted_by_the_surrounding_spectrum(self):
        # Measured across one wide span this cliff would average out; the scan
        # for the sharpest narrow slope is what keeps it visible.
        assert _steepness(brickwall(19500)) > 25.0


class TestClassify:
    @pytest.mark.parametrize(
        ("cutoff", "expected_label"),
        [
            (16000, "~128kbps source"),
            (18000, "~160-192kbps source"),
            (19500, "~256-320kbps source"),
        ],
    )
    def test_known_bitrate_cutoffs_are_named(self, cutoff, expected_label):
        assert matched_bitrate(cutoff) == expected_label

    def test_an_unusual_cutoff_matches_no_bitrate(self):
        assert matched_bitrate(12000) is None
        assert matched_bitrate(21000) is None

    def test_the_known_bands_leave_no_gap_a_real_transcode_could_fall_into(self):
        for (_, upper, _), (lower, _, _) in itertools.pairwise(KNOWN_LOSSY_CUTOFFS):
            assert upper == lower

    def test_content_near_nyquist_is_accepted_however_steep(self):
        verdict, detail = classify(21500.0, 90.0, SAMPLE_RATE)

        assert verdict is Verdict.OK
        assert "Nyquist" in detail

    def test_a_steep_cutoff_at_a_known_bitrate_is_fake(self):
        verdict, detail = classify(16000.0, 60.0, SAMPLE_RATE)

        assert verdict is Verdict.FAKE
        assert "~128kbps source" in detail

    def test_a_steep_cutoff_at_an_unknown_frequency_is_only_suspect(self):
        verdict, detail = classify(12000.0, 60.0, SAMPLE_RATE)

        assert verdict is Verdict.SUSPECT
        assert "inspect manually" in detail

    def test_a_gradual_rolloff_is_accepted_even_when_it_starts_low(self):
        # The failure mode the steepness test exists to prevent: a loud master
        # rolling off at 16 kHz is not a transcode.
        verdict, detail = classify(16000.0, 9.0, SAMPLE_RATE)

        assert verdict is Verdict.OK
        assert "mastering" in detail

    def test_a_sharply_cut_hi_res_container_is_called_an_upsample(self):
        verdict, detail = classify(19000.0, 60.0, 96000)

        assert verdict is Verdict.FAKE
        assert "upsampled" in detail

    def test_a_gently_rolled_hi_res_container_is_not_flagged(self):
        assert classify(19000.0, 9.0, 96000)[0] is Verdict.OK


class TestAnalyse:
    @pytest.mark.parametrize(
        ("cutoff", "label"),
        [(16000, "~128kbps"), (18000, "~160-192kbps"), (19500, "~256-320kbps")],
    )
    def test_a_transcode_is_detected_and_its_source_bitrate_named(self, cutoff, label):
        result = analyse([brickwall(cutoff)], SAMPLE_RATE)

        assert result is not None
        assert result.verdict is Verdict.FAKE
        assert label in result.detail

    def test_genuine_lossless_audio_passes(self):
        result = analyse([full_band()], SAMPLE_RATE)

        assert result is not None
        assert result.verdict is Verdict.OK

    def test_a_gradual_rolloff_passes(self):
        result = analyse([gradual_rolloff(knee_hz=14000, db_per_khz=7)], SAMPLE_RATE)

        assert result is not None
        assert result.verdict is Verdict.OK

    def test_an_upsampled_hi_res_file_is_detected(self):
        result = analyse([brickwall(19000, sample_rate=96000)], 96000)

        assert result is not None
        assert result.verdict is Verdict.FAKE
        assert "upsampled" in result.detail

    def test_the_steepness_threshold_is_configurable(self):
        gentle = [gradual_rolloff(knee_hz=9000, db_per_khz=12)]

        assert analyse(gentle, SAMPLE_RATE).verdict is Verdict.OK
        strict = analyse(gentle, SAMPLE_RATE, steep_db_per_khz=2.0)
        assert strict is not None
        assert strict.verdict is not Verdict.OK

    def test_the_near_nyquist_margin_is_configurable(self):
        assert classify(19000.0, 60.0, SAMPLE_RATE, near_nyquist_margin_hz=5000)[0] is Verdict.OK

    def test_input_too_short_to_analyse_returns_none(self):
        assert analyse([np.zeros(64, dtype=np.float32)], SAMPLE_RATE) is None


def _steepness(samples: np.ndarray) -> float:
    spectrum = average_spectrum_db([samples], SAMPLE_RATE)
    assert spectrum is not None
    bands = band_average(*spectrum)
    return cliff_steepness(bands, find_cutoff(bands))
