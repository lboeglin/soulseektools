import json
import os
from pathlib import Path

import pytest
from typer.testing import CliRunner

from soulseektools import __version__
from soulseektools.cli.main import app
from soulseektools.config import CONFIG_FILENAME

runner = CliRunner()


@pytest.fixture(autouse=True)
def isolated_environment(monkeypatch, tmp_path):
    for name in [n for n in os.environ if n.startswith("STK_") or n == "SLSKD_API_KEY"]:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.chdir(tmp_path)


@pytest.fixture
def config_file(tmp_path, library_root):
    path = tmp_path / CONFIG_FILENAME
    path.write_text(
        f'[library]\nroot = "{library_root.as_posix()}"\n\n'
        f'[index]\npath = "{(tmp_path / "index.db").as_posix()}"\n',
        encoding="utf-8",
    )
    return path


def invoke(*args):
    return runner.invoke(app, list(args), catch_exceptions=False)


class TestRootCommand:
    def test_version(self):
        result = invoke("--version")

        assert result.exit_code == 0
        assert __version__ in result.stdout

    def test_help_lists_every_command_group(self):
        result = invoke("--help")

        assert result.exit_code == 0
        for group in ("config", "doctor", "library"):
            assert group in result.stdout

    def test_a_missing_config_file_fails_with_a_clear_message(self, tmp_path):
        result = invoke("--config", str(tmp_path / "absent.toml"), "config", "show")

        assert result.exit_code == 1
        assert "not found" in result.output


class TestConfigCommands:
    def test_init_writes_a_reloadable_file(self, tmp_path):
        assert invoke("config", "init").exit_code == 0

        written = tmp_path / CONFIG_FILENAME
        assert written.is_file()
        assert invoke("config", "show").exit_code == 0

    def test_init_refuses_to_overwrite(self, tmp_path):
        invoke("config", "init")

        result = invoke("config", "init")

        assert result.exit_code == 1
        assert "already exists" in result.output

    def test_show_emits_json_on_request(self, config_file):
        result = invoke("--config", str(config_file), "--json", "config", "show")

        assert result.exit_code == 0
        assert json.loads(result.stdout)["matching"]["artist"] == 0.6

    def test_show_never_prints_the_api_key(self, config_file, monkeypatch):
        monkeypatch.setenv("SLSKD_API_KEY", "s3cret")

        result = invoke("--config", str(config_file), "config", "show")

        assert "s3cret" not in result.stdout

    def test_path_reports_the_file_in_use_unwrapped(self, config_file):
        # The output is meant to be substituted into other commands, so a long
        # path must survive intact rather than being wrapped to the terminal.
        result = invoke("--config", str(config_file), "config", "path")

        assert result.stdout.strip() == str(config_file)

    def test_path_reports_defaults_when_there_is_no_file(self):
        result = invoke("config", "path")

        assert "defaults" in result.stdout


class TestDoctor:
    def test_passes_when_the_library_exists(self, config_file):
        result = invoke("--config", str(config_file), "doctor")

        assert result.exit_code == 0
        assert "library root" in result.stdout

    def test_fails_when_the_library_root_is_missing(self, tmp_path):
        path = tmp_path / CONFIG_FILENAME
        path.write_text('[library]\nroot = "does-not-exist"\n', encoding="utf-8")

        result = invoke("--config", str(path), "doctor")

        assert result.exit_code == 1

    def test_json_output_lists_each_check(self, config_file):
        result = invoke("--config", str(config_file), "--json", "doctor")

        checks = json.loads(result.stdout)
        assert {check["name"] for check in checks} >= {"library root", "ffmpeg", "slskd url"}

    def test_a_missing_optional_tool_does_not_fail_the_run(self, config_file, monkeypatch):
        monkeypatch.setattr(
            "soulseektools.cli.doctor_cmd.shutil.which",
            lambda name: None if name == "fpcalc" else f"/usr/bin/{name}",
        )

        assert invoke("--config", str(config_file), "doctor").exit_code == 0


class TestLibraryCommands:
    def test_scan_indexes_the_library(self, config_file):
        result = invoke("--config", str(config_file), "--json", "library", "scan")

        payload = json.loads(result.stdout)
        assert payload["added"] == 4
        assert payload["total"] == 4

    def test_a_second_scan_rereads_nothing(self, config_file):
        invoke("--config", str(config_file), "--json", "library", "scan")

        result = invoke("--config", str(config_file), "--json", "library", "scan")

        assert json.loads(result.stdout)["unchanged"] == 4

    def test_full_scan_rereads_everything(self, config_file):
        invoke("--config", str(config_file), "--json", "library", "scan")

        result = invoke("--config", str(config_file), "--json", "library", "scan", "--full")

        assert json.loads(result.stdout)["updated"] == 4

    def test_the_library_flag_overrides_the_configured_root(self, config_file, tmp_path):
        empty = tmp_path / "Empty"
        empty.mkdir()

        result = invoke(
            "--config", str(config_file), "--library", str(empty), "--json", "library", "scan"
        )

        assert json.loads(result.stdout)["added"] == 0

    def test_scanning_a_missing_root_fails_with_a_hint(self, tmp_path):
        path = tmp_path / CONFIG_FILENAME
        path.write_text('[library]\nroot = "nowhere"\n', encoding="utf-8")

        result = invoke("--config", str(path), "library", "scan")

        assert result.exit_code == 1
        assert "does not exist" in result.output

    def test_stats_summarise_the_index(self, config_file):
        invoke("--config", str(config_file), "library", "scan")

        result = invoke("--config", str(config_file), "--json", "library", "stats")

        payload = json.loads(result.stdout)
        assert payload["total"] == 4
        assert payload["artists"]["DECO_27"] == 2

    def test_stats_say_so_when_the_index_is_empty(self, tmp_path, library_root):
        path = tmp_path / CONFIG_FILENAME
        path.write_text(
            f'[library]\nroot = "{library_root.as_posix()}"\n\n'
            f'[index]\npath = "{(tmp_path / "fresh.db").as_posix()}"\n',
            encoding="utf-8",
        )

        result = invoke("--config", str(path), "library", "stats")

        assert "scan" in result.stdout

    def test_scan_reports_the_indexed_count_in_human_output(self, config_file):
        result = invoke("--config", str(config_file), "library", "scan")

        assert "Indexed 4" in result.stdout


def test_no_command_prints_help():
    result = runner.invoke(app, [])

    assert result.exit_code != 0
    assert "Usage" in result.output


def test_the_package_is_importable_as_a_module():
    assert Path(__file__).exists()
