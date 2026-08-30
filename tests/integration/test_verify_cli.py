import csv
import json
import os
import subprocess

import pytest
from typer.testing import CliRunner

from soulseektools.cli.main import app
from soulseektools.config import CONFIG_FILENAME
from tests.conftest import FFMPEG

runner = CliRunner()


@pytest.fixture(autouse=True)
def isolated_environment(monkeypatch, tmp_path):
    for name in [n for n in os.environ if n.startswith("STK_") or n == "SLSKD_API_KEY"]:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.chdir(tmp_path)


@pytest.fixture
def mixed_library(tmp_path, requires_ffmpeg):
    """One genuine lossless file and one MP3 laundered into a FLAC container."""
    root = tmp_path / "Music" / "Artist"
    root.mkdir(parents=True)

    genuine = root / "genuine.flac"
    _ffmpeg("-f", "lavfi", "-i", "anoisesrc=d=8:c=white:r=44100", "-ac", "2", str(genuine))

    lossy = tmp_path / "intermediate.mp3"
    _ffmpeg("-i", str(genuine), "-c:a", "libmp3lame", "-b:a", "128k", "-ac", "2", str(lossy))
    _ffmpeg("-i", str(lossy), "-c:a", "flac", str(root / "faked.flac"))

    return tmp_path / "Music"


@pytest.fixture
def config_file(tmp_path, mixed_library):
    path = tmp_path / CONFIG_FILENAME
    path.write_text(f'[library]\nroot = "{mixed_library.as_posix()}"\n', encoding="utf-8")
    return path


def _ffmpeg(*args):
    subprocess.run([FFMPEG, "-v", "error", "-y", *args], check=True, capture_output=True)


def invoke(*args):
    return runner.invoke(app, list(args), catch_exceptions=False)


class TestVerifyCommand:
    def test_reports_both_verdicts_for_the_library(self, config_file):
        result = invoke("--config", str(config_file), "--json", "verify")

        verdicts = {
            row["path"].split("\\")[-1].split("/")[-1]: row["verdict"]
            for row in json.loads(result.stdout)
        }
        assert verdicts["genuine.flac"] == "OK"
        assert verdicts["faked.flac"] == "FAKE"

    def test_exits_non_zero_when_a_fake_is_found(self, config_file):
        assert invoke("--config", str(config_file), "verify").exit_code == 1

    def test_exits_zero_when_everything_is_genuine(self, config_file, mixed_library):
        (mixed_library / "Artist" / "faked.flac").unlink()

        assert invoke("--config", str(config_file), "verify").exit_code == 0

    def test_a_single_file_can_be_checked(self, config_file, mixed_library):
        target = mixed_library / "Artist" / "genuine.flac"

        result = invoke("--config", str(config_file), "--json", "verify", str(target))

        assert len(json.loads(result.stdout)) == 1

    def test_a_folder_can_be_checked(self, config_file, mixed_library):
        result = invoke("--config", str(config_file), "--json", "verify", str(mixed_library))

        assert len(json.loads(result.stdout)) == 2

    def test_a_missing_target_fails_clearly(self, config_file, tmp_path):
        result = invoke("--config", str(config_file), "verify", str(tmp_path / "absent.flac"))

        assert result.exit_code == 1
        assert "no such file" in result.output

    def test_a_csv_report_is_written(self, config_file, tmp_path):
        destination = tmp_path / "reports" / "verify.csv"

        invoke("--config", str(config_file), "verify", "--report", str(destination))

        with destination.open(encoding="utf-8-sig", newline="") as handle:
            rows = list(csv.DictReader(handle))
        assert {row["verdict"] for row in rows} == {"OK", "FAKE"}
        assert all(row["cutoff_hz"] for row in rows)

    def test_only_flagged_hides_the_passing_files(self, config_file):
        result = invoke("--config", str(config_file), "verify", "--only-flagged")

        assert "faked.flac" in result.stdout
        assert "genuine.flac" not in result.stdout

    def test_spectrograms_are_written_beside_flagged_files_only(self, config_file, mixed_library):
        invoke("--config", str(config_file), "verify", "--spectrograms")

        artist = mixed_library / "Artist"
        assert (artist / "faked.spectrogram.png").is_file()
        assert not (artist / "genuine.spectrogram.png").exists()

    def test_an_empty_library_says_so(self, tmp_path):
        empty = tmp_path / "Empty"
        empty.mkdir()
        path = tmp_path / CONFIG_FILENAME
        path.write_text(f'[library]\nroot = "{empty.as_posix()}"\n', encoding="utf-8")

        result = invoke("--config", str(path), "verify")

        assert result.exit_code == 0
        assert "No lossless files" in result.stdout

    def test_the_summary_counts_each_verdict(self, config_file):
        result = invoke("--config", str(config_file), "verify")

        assert "Checked 2 file(s)" in result.stdout
