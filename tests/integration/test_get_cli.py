"""The single-track flow: ask for one song by name, pick a result, download it."""

import json
import os

import pytest
from typer.testing import CliRunner

from soulseektools.cli import client as client_module
from soulseektools.cli.get_cmd import split_query
from soulseektools.cli.main import app
from soulseektools.config import CONFIG_FILENAME
from soulseektools.models import Candidate, TrackStatus
from soulseektools.review.store import ReviewStore
from tests.unit.test_fetch import FakeClient

runner = CliRunner()


@pytest.fixture(autouse=True)
def isolated_environment(monkeypatch, tmp_path):
    for name in [n for n in os.environ if n.startswith("STK_") or n == "SLSKD_API_KEY"]:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.chdir(tmp_path)


@pytest.fixture
def config_file(tmp_path):
    path = tmp_path / CONFIG_FILENAME
    path.write_text(
        f'[library]\nroot = "{tmp_path.as_posix()}"\n\n'
        f'[review]\npath = "{(tmp_path / "review.db").as_posix()}"\n',
        encoding="utf-8",
    )
    return path


def candidate(path, **overrides) -> Candidate:
    defaults = {
        "username": "peerA",
        "remote_path": path,
        "size": 30_000_000,
        "bit_depth": 16,
        "sample_rate": 44100,
        "free_upload_slots": True,
    }
    return Candidate(**{**defaults, **overrides})


LOSSLESS = candidate("@@peerA\\Music\\Ghost\\03 - Roki.flac")
LOSSY = candidate(
    "@@peerB\\mp3\\Roki.mp3",
    username="peerB",
    bit_depth=None,
    sample_rate=None,
    bitrate=192,
)
LIVE = candidate("@@peerC\\Live at Budokan\\03 - Roki.flac", username="peerC")


@pytest.fixture
def backend(monkeypatch):
    """Install a fake Soulseek backend that the command will pick up."""

    def install(results):
        fake = FakeClient(results)
        monkeypatch.setattr(client_module, "open_client", lambda _ctx: fake)
        monkeypatch.setattr("soulseektools.cli.get_cmd.open_client", lambda _ctx: fake)
        monkeypatch.setattr("soulseektools.cli.fetch_cmd.open_client", lambda _ctx: fake)
        return fake

    return install


def invoke(config_file, *args, stdin=""):
    return runner.invoke(
        app, ["--config", str(config_file), *args], input=stdin, catch_exceptions=False
    )


class TestSplitQuery:
    @pytest.mark.parametrize(
        ("query", "expected"),
        [
            ("DECO*27 - Roki", ("DECO*27", "Roki")),
            ("Roki", ("", "Roki")),
            ("  spaced  -  out  ", ("spaced", "out")),
            ("a - b - c", ("a", "b - c")),
        ],
    )
    def test_splits_on_a_plain_hyphen(self, query, expected):
        assert split_query(query) == expected

    def test_splits_on_a_typographic_dash(self):
        # What you get pasting a name out of a browser.
        assert split_query("YOASOBI – Idol") == ("YOASOBI", "Idol")  # noqa: RUF001
        assert split_query("YOASOBI — Idol") == ("YOASOBI", "Idol")


class TestGet:
    def test_candidates_are_offered_for_a_one_line_query(self, config_file, backend):
        backend([LOSSLESS, LOSSY])

        result = invoke(config_file, "--json", "get", "DECO*27 - Roki")

        payload = json.loads(result.stdout)
        assert payload["query"] == "DECO*27 - Roki"
        assert len(payload["candidates"]) == 2

    def test_the_artist_can_be_given_separately(self, config_file, backend):
        fake = backend([LOSSLESS])

        invoke(config_file, "--json", "get", "Roki", "--artist", "DECO*27")

        assert fake.searches == ["DECO*27 Roki"]

    def test_a_title_alone_is_searchable(self, config_file, backend):
        fake = backend([LOSSLESS])

        invoke(config_file, "--json", "get", "Roki")

        assert fake.searches == ["Roki"]

    def test_an_empty_query_fails_with_an_example(self, config_file, backend):
        backend([])

        result = invoke(config_file, "get", "   ")

        assert result.exit_code == 1
        assert "stk get" in result.output

    def test_nothing_found_exits_non_zero(self, config_file, backend):
        backend([])

        result = invoke(config_file, "get", "DECO*27 - Roki")

        assert result.exit_code == 1
        assert "Nothing usable found" in result.output

    def test_the_chosen_candidate_is_queued(self, config_file, backend, tmp_path):
        fake = backend([LOSSLESS, LOSSY])

        result = invoke(config_file, "get", "DECO*27 - Roki", stdin="1\n")

        assert result.exit_code == 0
        assert fake.enqueued == [("peerA", LOSSLESS.remote_path, "DECO_27")]

    def test_a_different_candidate_can_be_chosen(self, config_file, backend):
        fake = backend([LOSSLESS, LOSSY])

        invoke(config_file, "get", "DECO*27 - Roki", stdin="2\n")

        assert fake.enqueued[0] == ("peerB", LOSSY.remote_path, "DECO_27")

    def test_declining_downloads_nothing(self, config_file, backend):
        fake = backend([LOSSLESS])

        result = invoke(config_file, "get", "DECO*27 - Roki", stdin="n\n")

        assert fake.enqueued == []
        assert "Nothing downloaded" in result.stdout

    def test_an_out_of_range_choice_downloads_nothing(self, config_file, backend):
        fake = backend([LOSSLESS])

        invoke(config_file, "get", "DECO*27 - Roki", stdin="9\n")

        assert fake.enqueued == []

    def test_yes_takes_the_best_without_asking(self, config_file, backend):
        fake = backend([LIVE, LOSSLESS])

        result = invoke(config_file, "get", "DECO*27 - Roki", "--yes")

        assert result.exit_code == 0
        # The studio copy outranks the one in a "Live at Budokan" folder.
        assert fake.enqueued[0][1] == LOSSLESS.remote_path

    def test_queue_only_approves_without_downloading(self, config_file, backend, tmp_path):
        fake = backend([LOSSLESS])

        result = invoke(config_file, "get", "DECO*27 - Roki", "--yes", "--queue-only")

        assert fake.enqueued == []
        assert "fetch download" in result.stdout
        with ReviewStore(tmp_path / "review.db") as store:
            assert store.tracks()[0].status is TrackStatus.APPROVED

    def test_the_track_is_recorded_in_the_review_queue(self, config_file, backend, tmp_path):
        backend([LOSSLESS])

        invoke(config_file, "get", "DECO*27 - Roki", "--yes")

        with ReviewStore(tmp_path / "review.db") as store:
            entry = store.tracks()[0]
            assert entry.track.artist == "DECO*27"
            assert entry.status is TrackStatus.QUEUED

    def test_asking_twice_does_not_duplicate_the_track(self, config_file, backend, tmp_path):
        backend([LOSSLESS])

        invoke(config_file, "get", "DECO*27 - Roki", "--yes")
        invoke(config_file, "get", "DECO*27 - Roki", "--yes")

        with ReviewStore(tmp_path / "review.db") as store:
            assert len(store.tracks()) == 1

    def test_the_number_of_candidates_offered_can_be_capped(self, config_file, backend):
        backend([candidate(f"@@peer{i}\\Music\\03 - Roki.flac") for i in range(10)])

        payload = json.loads(invoke(config_file, "--json", "get", "Roki", "-n", "2").stdout)

        assert len(payload["candidates"]) == 2

    def test_warnings_are_shown_against_a_suspicious_result(self, config_file, backend):
        backend([LIVE])

        result = invoke(config_file, "get", "DECO*27 - Roki", stdin="n\n")

        assert "warnings" in result.stdout
        assert "Live at Budokan" in result.stdout


class TestQualityLimits:
    def test_lossless_only_rejects_a_lossy_result(self, config_file, backend):
        backend([LOSSY])

        result = invoke(config_file, "get", "DECO*27 - Roki", "--lossless")

        assert result.exit_code == 1
        assert "quality limits" in result.output

    def test_lossless_only_keeps_the_lossless_result(self, config_file, backend):
        backend([LOSSLESS, LOSSY])

        payload = json.loads(invoke(config_file, "--json", "get", "Roki", "--lossless").stdout)

        assert len(payload["candidates"]) == 1
        assert payload["candidates"][0]["remote_path"].endswith(".flac")

    def test_a_bitrate_floor_rejects_a_lower_bitrate_result(self, config_file, backend):
        backend([LOSSY])

        result = invoke(config_file, "get", "Roki", "--min-bitrate", "320")

        assert result.exit_code == 1

    def test_a_bitrate_floor_never_rejects_lossless(self, config_file, backend):
        backend([LOSSLESS])

        payload = json.loads(
            invoke(config_file, "--json", "get", "Roki", "--min-bitrate", "320").stdout
        )

        assert len(payload["candidates"]) == 1

    def test_the_floor_can_be_set_in_the_config_instead(self, tmp_path, backend):
        path = tmp_path / CONFIG_FILENAME
        path.write_text(
            f'[library]\nroot = "{tmp_path.as_posix()}"\n\n'
            f'[review]\npath = "{(tmp_path / "review.db").as_posix()}"\n\n'
            "[ranking]\nrequire_lossless = true\n",
            encoding="utf-8",
        )
        backend([LOSSY])

        assert invoke(path, "get", "Roki").exit_code == 1
