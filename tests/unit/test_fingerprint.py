import json
import subprocess

import numpy as np
import pytest

from soulseektools.audio import fingerprint as fp
from soulseektools.errors import ExternalToolError

DECIBELS = 10.0


def envelope(values) -> fp.Fingerprint:
    return fp.Fingerprint(kind=fp.FingerprintKind.ENVELOPE, values=tuple(float(v) for v in values))


def chromaprint(values) -> fp.Fingerprint:
    return fp.Fingerprint(kind=fp.FingerprintKind.CHROMAPRINT, values=tuple(int(v) for v in values))


class TestEnvelopeOf:
    def test_a_steady_tone_has_a_flat_centred_curve(self):
        samples = np.full(fp.ENVELOPE_SAMPLE_RATE * 4, 0.5, dtype=np.float32)

        curve = fp.envelope_of(samples)

        assert curve.size == 16
        assert np.allclose(curve, 0.0)

    def test_the_curve_follows_the_loudness_over_time(self):
        quiet = np.full(fp.ENVELOPE_SAMPLE_RATE, 0.01, dtype=np.float32)
        loud = np.full(fp.ENVELOPE_SAMPLE_RATE, 0.9, dtype=np.float32)

        curve = fp.envelope_of(np.concatenate([quiet, loud]))

        assert curve[0] < 0 < curve[-1]

    def test_overall_volume_does_not_change_the_curve(self):
        # The same rip at two mastering levels must fingerprint identically.
        pattern = np.concatenate(
            [
                np.full(fp.ENVELOPE_SAMPLE_RATE, 0.2, dtype=np.float32),
                np.full(fp.ENVELOPE_SAMPLE_RATE, 0.8, dtype=np.float32),
            ]
        )

        assert np.allclose(fp.envelope_of(pattern), fp.envelope_of(pattern * 0.25), atol=1e-6)

    def test_audio_shorter_than_one_frame_yields_nothing(self):
        assert fp.envelope_of(np.zeros(10, dtype=np.float32)).size == 0


class TestSimilarity:
    def test_a_fingerprint_matches_itself(self):
        curve = envelope(DECIBELS * np.sin(np.linspace(0, 12, 40)))

        assert fp.similarity(curve, curve) == pytest.approx(1.0)

    def test_the_same_track_shifted_in_time_still_matches(self):
        curve = DECIBELS * np.sin(np.linspace(0, 12, 60))

        assert fp.similarity(envelope(curve), envelope(curve[3:])) > 0.95

    def test_unrelated_curves_do_not_match(self):
        generator = np.random.default_rng(7)

        left = envelope(DECIBELS * generator.standard_normal(60))
        right = envelope(DECIBELS * generator.standard_normal(60))

        assert fp.similarity(left, right) < 0.9

    def test_an_empty_fingerprint_matches_nothing(self):
        assert fp.similarity(envelope([]), envelope([1, 2, 3])) == 0.0

    def test_fingerprints_of_different_kinds_are_never_compared(self):
        with pytest.raises(ValueError, match="cannot compare"):
            fp.similarity(envelope([1.0] * 20), chromaprint([1] * 20))

    def test_a_featureless_curve_yields_no_evidence_either_way(self):
        flat = envelope(np.zeros(40))
        varied = envelope(DECIBELS * np.sin(np.linspace(0, 12, 40)))

        assert fp.similarity(flat, flat) == pytest.approx(0.5)
        assert fp.similarity(flat, varied) == 0.0

    def test_too_few_frames_to_compare_scores_zero(self):
        short = envelope([0.0, DECIBELS, -DECIBELS])

        assert fp.similarity(short, short) == 0.0

    def test_identical_chromaprints_match(self):
        values = list(range(1000, 1030))

        assert fp.similarity(chromaprint(values), chromaprint(values)) == pytest.approx(1.0)

    def test_chromaprints_differing_in_every_bit_do_not_match(self):
        values = [0] * 30
        inverted = [0xFFFFFFFF] * 30

        assert fp.similarity(chromaprint(values), chromaprint(inverted)) < 0.1

    def test_a_chromaprint_with_a_few_altered_bits_still_matches(self):
        values = list(range(2000, 2040))
        altered = [value ^ 0b1 for value in values]

        assert fp.similarity(chromaprint(values), chromaprint(altered)) > 0.95


class TestBackendSelection:
    def test_chromaprint_is_preferred_when_fpcalc_is_installed(self, monkeypatch):
        monkeypatch.setattr(fp.shutil, "which", lambda _name: "/usr/bin/fpcalc")

        assert fp.available_kind() is fp.FingerprintKind.CHROMAPRINT

    def test_the_envelope_backend_is_used_when_fpcalc_is_absent(self, monkeypatch):
        monkeypatch.setattr(fp.shutil, "which", lambda _name: None)

        assert fp.available_kind() is fp.FingerprintKind.ENVELOPE

    def test_asking_for_chromaprint_without_fpcalc_is_an_explicit_error(
        self, monkeypatch, tmp_path
    ):
        monkeypatch.setattr(fp.shutil, "which", lambda _name: None)

        with pytest.raises(ExternalToolError, match="fpcalc"):
            fp.fingerprint(tmp_path / "track.flac", fp.FingerprintKind.CHROMAPRINT)

    def test_the_chromaprint_backend_parses_fpcalc_output(self, monkeypatch, tmp_path):
        monkeypatch.setattr(fp.shutil, "which", lambda _name: "/usr/bin/fpcalc")
        payload = json.dumps({"duration": 212.5, "fingerprint": [1, 2, 3]}).encode()
        monkeypatch.setattr(
            fp.subprocess,
            "run",
            lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0, payload, b""),
        )

        result = fp.fingerprint(tmp_path / "track.flac", fp.FingerprintKind.CHROMAPRINT)

        assert result.kind is fp.FingerprintKind.CHROMAPRINT
        assert result.values == (1, 2, 3)
        assert result.duration_seconds == pytest.approx(212.5)

    def test_an_fpcalc_failure_is_reported_as_a_tool_error(self, monkeypatch, tmp_path):
        monkeypatch.setattr(fp.shutil, "which", lambda _name: "/usr/bin/fpcalc")

        def explode(*_args, **_kwargs):
            raise subprocess.CalledProcessError(1, "fpcalc", b"", b"bad file")

        monkeypatch.setattr(fp.subprocess, "run", explode)

        with pytest.raises(ExternalToolError, match="bad file"):
            fp.fingerprint(tmp_path / "track.flac", fp.FingerprintKind.CHROMAPRINT)


class TestAgainstRealFiles:
    def test_the_same_audio_in_two_containers_fingerprints_alike(self, tmp_path, encode_audio):
        # The pairing the duplicate scan exists for: one track, ripped twice, as
        # lossless and as lossy, sharing no metadata and differing in file size.
        options = {"seconds": 8.0, "frequency": 300, "modulation_hz": 0.7}
        lossless = encode_audio(tmp_path / "a.flac", **options)
        lossy = encode_audio(tmp_path / "a.mp3", **options)

        left = fp.fingerprint(lossless, fp.FingerprintKind.ENVELOPE)
        right = fp.fingerprint(lossy, fp.FingerprintKind.ENVELOPE)

        assert fp.similarity(left, right) > 0.9

    def test_two_different_tracks_do_not_fingerprint_alike(self, tmp_path, encode_audio):
        first = encode_audio(tmp_path / "a.flac", seconds=8.0, frequency=300, modulation_hz=0.7)
        second = encode_audio(tmp_path / "b.flac", seconds=8.0, frequency=300, modulation_hz=2.9)

        left = fp.fingerprint(first, fp.FingerprintKind.ENVELOPE)
        right = fp.fingerprint(second, fp.FingerprintKind.ENVELOPE)

        assert fp.similarity(left, right) < 0.9

    def test_featureless_audio_reports_no_evidence_either_way(self, tmp_path, encode_audio):
        # A sustained tone has no loudness shape to correlate. The score must be
        # too low to group files by and too high to claim they differ.
        steady = fp.fingerprint(
            encode_audio(tmp_path / "tone.flac", seconds=8.0), fp.FingerprintKind.ENVELOPE
        )
        dynamic = fp.fingerprint(
            encode_audio(tmp_path / "music.flac", seconds=8.0, modulation_hz=0.7),
            fp.FingerprintKind.ENVELOPE,
        )

        assert fp.similarity(steady, steady) == pytest.approx(0.5)
        assert fp.similarity(steady, dynamic) == 0.0

    def test_a_fingerprint_records_the_duration(self, tmp_path, encode_audio):
        track = encode_audio(tmp_path / "a.flac", seconds=5.0)

        result = fp.fingerprint(track, fp.FingerprintKind.ENVELOPE)

        assert result.duration_seconds == pytest.approx(5.0, abs=0.2)
