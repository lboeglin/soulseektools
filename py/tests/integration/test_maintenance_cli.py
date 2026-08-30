import csv
import io
import json
import os

import httpx
import pytest
import respx
from typer.testing import CliRunner

from soulseektools.artwork.itunes import SEARCH_URL as ITUNES_URL
from soulseektools.cli.main import app
from soulseektools.config import CONFIG_FILENAME
from soulseektools.library import tags

runner = CliRunner()

PNG = b"\x89PNG\r\n\x1a\n" + b"padding" * 2000


@pytest.fixture(autouse=True)
def isolated_environment(monkeypatch, tmp_path):
    for name in [n for n in os.environ if n.startswith("STK_") or n == "SLSKD_API_KEY"]:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.chdir(tmp_path)


@pytest.fixture
def genre_map(tmp_path):
    path = tmp_path / "genres.toml"
    path.write_text('"DECO*27" = "Vocaloid"\nZUN = ["Touhou", "Game"]\n', encoding="utf-8")
    return path


@pytest.fixture
def config_file(tmp_path, library_root, genre_map):
    path = tmp_path / CONFIG_FILENAME
    path.write_text(
        f'[library]\nroot = "{library_root.as_posix()}"\n\n'
        f'[index]\npath = "{(tmp_path / "index.db").as_posix()}"\n\n'
        f'[genres]\nmap_path = "{genre_map.as_posix()}"\n',
        encoding="utf-8",
    )
    runner.invoke(app, ["--config", str(path), "library", "scan"], catch_exceptions=False)
    return path


def invoke(config_file, *args):
    return runner.invoke(app, ["--config", str(config_file), *args], catch_exceptions=False)


class TestDupes:
    def test_titles_reports_no_duplicates_in_a_clean_library(self, config_file):
        result = invoke(config_file, "--json", "dupes", "titles")

        assert json.loads(result.stdout) == []

    def test_titles_finds_a_copy_filed_under_another_artist(
        self, config_file, library_root, encode_audio
    ):
        encode_audio(
            library_root / "deco27" / "Roki.flac",
            metadata={"artist": "DECO*27", "title": "Roki"},
        )
        invoke(config_file, "library", "scan")

        pairs = json.loads(invoke(config_file, "--json", "dupes", "titles").stdout)

        assert len(pairs) == 1
        assert "Roki" in pairs[0]["keep"]

    def test_titles_names_the_larger_copy_as_the_one_to_keep(
        self, config_file, library_root, encode_audio
    ):
        encode_audio(
            library_root / "deco27" / "Roki.flac",
            seconds=4.0,
            metadata={"artist": "DECO*27", "title": "Roki"},
        )
        invoke(config_file, "library", "scan")

        pair = json.loads(invoke(config_file, "--json", "dupes", "titles").stdout)[0]

        assert pair["keep_bytes"] >= pair["other_bytes"]

    def test_content_finds_the_same_audio_under_different_names(
        self, config_file, library_root, encode_audio
    ):
        options = {"seconds": 8.0, "frequency": 350, "modulation_hz": 0.8}
        encode_audio(library_root / "Unknown" / "track01.flac", **options)
        encode_audio(library_root / "Someone Else" / "18.mp3", **options)
        invoke(config_file, "library", "scan")

        pairs = json.loads(invoke(config_file, "--json", "dupes", "content").stdout)

        assert len(pairs) == 1

    def test_the_human_output_says_nothing_was_deleted(self, config_file):
        result = invoke(config_file, "dupes", "titles")

        assert "Nothing was deleted" in result.stdout or "No duplicates" in result.stdout

    def test_an_empty_index_says_to_scan_first(self, tmp_path, library_root):
        path = tmp_path / CONFIG_FILENAME
        path.write_text(
            f'[library]\nroot = "{library_root.as_posix()}"\n\n'
            f'[index]\npath = "{(tmp_path / "fresh.db").as_posix()}"\n',
            encoding="utf-8",
        )

        assert "scan" in invoke(path, "dupes", "titles").stdout


class TestGenre:
    def test_check_lists_artists_missing_from_the_map(self, config_file):
        payload = json.loads(invoke(config_file, "--json", "genre", "check").stdout)

        assert "beatMARIO" in payload["unmapped_artists"]
        assert "DECO_27" not in payload["unmapped_artists"]

    def test_apply_writes_nothing_without_the_flag(self, config_file, library_root):
        untagged = library_root / "ZUN" / "Nested" / "15 Heian Alien.flac"
        assert tags.read_tags(untagged).genres == ()

        payload = json.loads(invoke(config_file, "--json", "genre", "apply").stdout)

        assert payload["applied"] is False
        assert payload["planned"]
        assert tags.read_tags(untagged).genres == ()

    def test_apply_tags_the_mapped_files(self, config_file, library_root):
        invoke(config_file, "genre", "apply", "--apply")

        assert tags.read_tags(library_root / "DECO_27" / "01 - Roki.flac").genres == ("Vocaloid",)
        assert tags.read_tags(library_root / "ZUN" / "Nested" / "15 Heian Alien.flac").genres == (
            "Touhou",
            "Game",
        )

    def test_apply_is_idempotent_without_a_rescan(self, config_file):
        # The index has to reflect what the command just wrote, or the next
        # command reports the state from before it ran.
        invoke(config_file, "genre", "apply", "--apply")

        payload = json.loads(invoke(config_file, "--json", "genre", "apply").stdout)

        assert payload["planned"] == []

    def test_the_new_genres_are_visible_to_other_commands_immediately(self, config_file):
        invoke(config_file, "genre", "apply", "--apply")

        payload = json.loads(invoke(config_file, "--json", "genre", "check").stdout)

        assert payload["genres"]["Vocaloid"] == 2
        assert payload["untagged"] == 1

    def test_a_missing_genre_map_fails_with_a_hint(self, tmp_path, library_root):
        path = tmp_path / CONFIG_FILENAME
        path.write_text(
            f'[library]\nroot = "{library_root.as_posix()}"\n\n'
            f'[genres]\nmap_path = "{(tmp_path / "absent.toml").as_posix()}"\n',
            encoding="utf-8",
        )

        result = invoke(path, "genre", "apply")

        assert result.exit_code == 1
        assert "genres.example.toml" in result.output


class TestArt:
    def test_check_reports_files_with_no_cover(self, config_file):
        missing = json.loads(invoke(config_file, "--json", "art", "check").stdout)

        assert len(missing) == 4
        assert all(row["reason"] == "no embedded cover" for row in missing)

    @respx.mock
    def test_fetch_writes_nothing_without_the_flag(self, config_file, library_root):
        _mock_itunes("DECO*27")

        payload = json.loads(invoke(config_file, "--json", "art", "fetch", "--limit", "1").stdout)

        assert payload[0]["applied"] is False
        assert tags.artwork_size(library_root / "DECO_27" / "01 - Roki.flac") == 0

    @respx.mock
    def test_fetch_embeds_a_verified_cover(self, config_file, library_root):
        _mock_itunes("DECO*27")

        invoke(config_file, "art", "fetch", "--limit", "1", "--apply")

        assert tags.artwork_size(library_root / "DECO_27" / "01 - Roki.flac") == len(PNG)

    @respx.mock
    def test_an_embedded_cover_is_visible_to_other_commands_immediately(self, config_file):
        _mock_itunes("DECO*27")
        before = len(json.loads(invoke(config_file, "--json", "art", "check").stdout))

        invoke(config_file, "art", "fetch", "--limit", "1", "--apply")

        after = json.loads(invoke(config_file, "--json", "art", "check").stdout)
        assert len(after) == before - 1

    @respx.mock
    def test_fetch_refuses_a_result_by_the_wrong_artist(self, config_file, library_root):
        _mock_itunes("A Completely Different Band")

        payload = json.loads(
            invoke(config_file, "--json", "art", "fetch", "--limit", "1", "--apply").stdout
        )

        assert payload[0]["ok"] is False
        assert tags.artwork_size(library_root / "DECO_27" / "01 - Roki.flac") == 0

    @respx.mock
    def test_verification_can_be_waived_explicitly(self, config_file, library_root):
        _mock_itunes("A Completely Different Band")

        payload = json.loads(
            invoke(
                config_file,
                "--json",
                "art",
                "fetch",
                "--limit",
                "1",
                "--apply",
                "--allow-unverified",
            ).stdout
        )

        assert payload[0]["ok"] is True

    def test_an_unknown_provider_fails_with_the_valid_names(self, tmp_path, library_root):
        path = tmp_path / CONFIG_FILENAME
        path.write_text(
            f'[library]\nroot = "{library_root.as_posix()}"\n\n'
            '[artwork]\nproviders = ["spotify"]\n',
            encoding="utf-8",
        )

        result = invoke(path, "art", "fetch")

        assert result.exit_code == 1
        assert "itunes" in result.output


class TestTags:
    def test_dump_writes_csv_to_stdout(self, config_file):
        result = invoke(config_file, "tags", "dump")

        rows = list(csv.DictReader(io.StringIO(result.stdout)))
        assert len(rows) == 4
        assert {row["folder_artist"] for row in rows} == {"DECO_27", "ZUN", "beatMARIO"}

    def test_dump_writes_to_a_file(self, config_file, tmp_path):
        destination = tmp_path / "out" / "tags.csv"

        invoke(config_file, "tags", "dump", "-o", str(destination))

        with destination.open(encoding="utf-8-sig", newline="") as handle:
            assert len(list(csv.DictReader(handle))) == 4

    def test_dump_surfaces_the_album_field_used_to_spot_wrong_matches(self, config_file):
        rows = json.loads(invoke(config_file, "--json", "tags", "dump").stdout)

        assert any(row["album"] == "Ghost" for row in rows)


def _mock_itunes(artist_name: str) -> None:
    respx.get(ITUNES_URL).mock(
        return_value=httpx.Response(
            200,
            json={
                "results": [
                    {
                        "artistName": artist_name,
                        "trackName": "Roki",
                        "artworkUrl100": "https://is1.test/a/100x100bb.jpg",
                    }
                ]
            },
        )
    )
    respx.get("https://is1.test/a/1200x1200bb.jpg").mock(
        return_value=httpx.Response(200, content=PNG)
    )
    # The fallback provider is mocked as empty so that a rejected iTunes result
    # exercises the rejection rather than reaching the network.
    respx.get(url__startswith="https://musicbrainz.org").mock(
        return_value=httpx.Response(200, json={"recordings": []})
    )
