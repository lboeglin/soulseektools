import os
from pathlib import Path

import pytest

from soulseektools.config import (
    CONFIG_FILENAME,
    Settings,
    find_config_file,
    load_settings,
    read_toml,
    render_toml,
    write_config,
)
from soulseektools.errors import ConfigError


@pytest.fixture(autouse=True)
def isolated_environment(monkeypatch, tmp_path):
    for name in [n for n in os.environ if n.startswith("STK_") or n == "SLSKD_API_KEY"]:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.chdir(tmp_path)


def write_toml(path: Path, body: str) -> Path:
    path.write_text(body, encoding="utf-8")
    return path


class TestDefaults:
    def test_settings_load_without_any_config_file(self):
        settings = load_settings()

        assert settings.source_path is None
        assert settings.matching.artist == pytest.approx(0.6)

    def test_extensions_are_normalised_to_lowercase_with_a_leading_dot(self):
        settings = Settings(library={"root": "Music", "extensions": ["FLAC", ".Mp3"]})

        assert settings.library.extensions == (".flac", ".mp3")

    def test_matching_settings_convert_to_match_thresholds(self):
        thresholds = load_settings().matching.thresholds()

        assert thresholds.artist_weak < thresholds.artist < thresholds.title_strong

    def test_ranking_terms_cover_the_folder_names_that_caused_wrong_downloads(self):
        ranking = load_settings().ranking

        assert {"live", "symphony", "orchestra", "medley"} <= set(ranking.penalise_terms)


class TestFileDiscovery:
    def test_finds_the_config_beside_the_working_directory(self, tmp_path):
        expected = write_toml(tmp_path / CONFIG_FILENAME, "")

        assert find_config_file(tmp_path) == expected

    def test_finds_the_config_in_a_parent_directory(self, tmp_path):
        expected = write_toml(tmp_path / CONFIG_FILENAME, "")
        nested = tmp_path / "a" / "b"
        nested.mkdir(parents=True)

        assert find_config_file(nested) == expected

    def test_values_come_from_the_config_file(self, tmp_path):
        path = write_toml(
            tmp_path / CONFIG_FILENAME,
            '[library]\nroot = "E:/Music"\n\n[matching]\nartist = 0.75\n',
        )

        settings = load_settings(path)

        assert settings.library.root == Path("E:/Music")
        assert settings.matching.artist == pytest.approx(0.75)
        assert settings.source_path == path

    def test_unset_keys_keep_their_defaults(self, tmp_path):
        path = write_toml(tmp_path / CONFIG_FILENAME, "[matching]\nartist = 0.75\n")

        settings = load_settings(path)

        assert settings.matching.title == pytest.approx(0.62)

    def test_a_missing_explicit_config_is_an_error(self, tmp_path):
        with pytest.raises(ConfigError, match="not found"):
            load_settings(tmp_path / "nope.toml")

    def test_an_unknown_key_is_rejected_rather_than_silently_ignored(self, tmp_path):
        path = write_toml(tmp_path / CONFIG_FILENAME, "[library]\nrooot = 1\n")

        with pytest.raises(ConfigError, match="invalid configuration"):
            load_settings(path)

    def test_an_out_of_range_value_is_rejected(self, tmp_path):
        path = write_toml(tmp_path / CONFIG_FILENAME, "[matching]\nartist = 4.2\n")

        with pytest.raises(ConfigError):
            load_settings(path)

    def test_malformed_toml_is_reported_as_a_config_error(self, tmp_path):
        path = write_toml(tmp_path / CONFIG_FILENAME, "not = = toml")

        with pytest.raises(ConfigError, match="cannot read"):
            read_toml(path)


class TestPrecedence:
    def test_the_environment_overrides_the_file(self, tmp_path, monkeypatch):
        path = write_toml(tmp_path / CONFIG_FILENAME, "[matching]\nartist = 0.75\n")
        monkeypatch.setenv("STK_MATCHING__ARTIST", "0.9")

        assert load_settings(path).matching.artist == pytest.approx(0.9)

    def test_an_explicit_override_beats_the_environment(self, tmp_path, monkeypatch):
        path = write_toml(tmp_path / CONFIG_FILENAME, '[library]\nroot = "from-file"\n')
        monkeypatch.setenv("STK_LIBRARY__ROOT", "from-env")

        settings = load_settings(path, library={"root": Path("from-flag")})

        assert settings.library.root == Path("from-flag")

    def test_a_none_override_does_not_erase_a_configured_value(self, tmp_path):
        path = write_toml(tmp_path / CONFIG_FILENAME, '[library]\nroot = "from-file"\n')

        assert load_settings(path, library=None).library.root == Path("from-file")


class TestSecrets:
    def test_the_api_key_is_read_from_the_environment(self, monkeypatch):
        monkeypatch.setenv("SLSKD_API_KEY", "s3cret")

        settings = load_settings()

        assert settings.slskd.api_key is not None
        assert settings.slskd.api_key.get_secret_value() == "s3cret"

    def test_the_api_key_never_appears_in_rendered_config(self, monkeypatch):
        monkeypatch.setenv("SLSKD_API_KEY", "s3cret")

        rendered = render_toml(load_settings())

        assert "s3cret" not in rendered
        assert "api_key" not in rendered


class TestPathResolution:
    def test_a_relative_library_root_resolves_against_the_config_file(self, tmp_path):
        (tmp_path / "config").mkdir()
        path = write_toml(tmp_path / "config" / CONFIG_FILENAME, '[library]\nroot = "../Music"\n')

        assert load_settings(path).resolved_library_root() == (tmp_path / "Music").resolve()

    def test_an_absolute_library_root_is_left_alone(self, tmp_path):
        absolute = (tmp_path / "Elsewhere").resolve()
        path = write_toml(
            tmp_path / CONFIG_FILENAME, f'[library]\nroot = "{absolute.as_posix()}"\n'
        )

        assert load_settings(path).resolved_library_root() == absolute


class TestWriteConfig:
    def test_written_config_reloads_to_the_same_settings(self, tmp_path):
        original = load_settings()

        written = write_config(tmp_path / CONFIG_FILENAME, original)
        reloaded = load_settings(written)

        assert reloaded.model_dump(exclude={"source_path"}) == original.model_dump(
            exclude={"source_path"}
        )

    def test_writing_over_an_existing_config_is_refused(self, tmp_path):
        path = write_toml(tmp_path / CONFIG_FILENAME, "")

        with pytest.raises(ConfigError, match="already exists"):
            write_config(path, load_settings())

    def test_the_written_file_says_where_secrets_come_from(self, tmp_path):
        written = write_config(tmp_path / CONFIG_FILENAME, load_settings())

        assert "SLSKD_API_KEY" in written.read_text(encoding="utf-8")
