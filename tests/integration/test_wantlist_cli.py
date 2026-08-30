import csv
import json
import os

import pytest
from typer.testing import CliRunner

from soulseektools.cli.main import app
from soulseektools.config import CONFIG_FILENAME

runner = CliRunner()

LEGACY_REVIEW_HEADER = (
    "query,artist,title,status,"
    "cand1_score,cand1_desc,cand1_user,cand1_path,cand1_size,cand1_bitrate,cand1_samplerate,"
    "cand1_bitdepth,cand1_vbr,"
    "cand2_score,cand2_desc,cand2_user,cand2_path,cand2_size,cand2_bitrate,cand2_samplerate,"
    "cand2_bitdepth,cand2_vbr,"
    "cand3_score,cand3_desc,cand3_user,cand3_path,cand3_size,cand3_bitrate,cand3_samplerate,"
    "cand3_bitdepth,cand3_vbr,approve"
)


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
        f'[index]\npath = "{(tmp_path / "index.db").as_posix()}"\n\n'
        f'[review]\npath = "{(tmp_path / "review.db").as_posix()}"\n',
        encoding="utf-8",
    )
    runner.invoke(app, ["--config", str(path), "library", "scan"], catch_exceptions=False)
    return path


@pytest.fixture
def export(tmp_path):
    path = tmp_path / "export.csv"
    path.write_text(
        "Track name,Artist name,Album name,Duration (ms)\n"
        "Roki,DECO*27,Ghost,215000\n"
        "Lagtrain,Inabakumori,Anti Reality,232000\n"
        "Night of Knights,beatMARIO,Toho,180000\n",
        encoding="utf-8-sig",
    )
    return path


def invoke(config_file, *args):
    return runner.invoke(app, ["--config", str(config_file), *args], catch_exceptions=False)


class TestWantlistDiff:
    def test_owned_tracks_are_separated_from_missing_ones(self, config_file, export):
        payload = json.loads(invoke(config_file, "--json", "wantlist", "diff", str(export)).stdout)

        assert payload["total"] == 3
        owned = {row["title"] for row in payload["owned"]}
        missing = {row["title"] for row in payload["missing"]}
        assert "Roki" in owned
        assert "Lagtrain" in missing

    def test_missing_tracks_are_queued_for_searching(self, config_file, export):
        payload = json.loads(invoke(config_file, "--json", "wantlist", "diff", str(export)).stdout)

        assert payload["queued"] == len(payload["missing"])
        queue = json.loads(invoke(config_file, "--json", "review", "list").stdout)
        assert {row["title"] for row in queue["tracks"]} == {
            row["title"] for row in payload["missing"]
        }

    def test_queueing_can_be_declined(self, config_file, export):
        payload = json.loads(
            invoke(config_file, "--json", "wantlist", "diff", str(export), "--no-queue").stdout
        )

        assert payload["queued"] == 0

    def test_running_twice_does_not_queue_the_same_track_again(self, config_file, export):
        invoke(config_file, "wantlist", "diff", str(export))

        payload = json.loads(invoke(config_file, "--json", "wantlist", "diff", str(export)).stdout)

        assert payload["queued"] == 0

    def test_the_missing_list_can_be_written_as_csv(self, config_file, export, tmp_path):
        destination = tmp_path / "out" / "missing.csv"

        invoke(config_file, "wantlist", "diff", str(export), "--missing", str(destination))

        with destination.open(encoding="utf-8-sig", newline="") as handle:
            rows = list(csv.DictReader(handle))
        assert "Lagtrain" in {row["Track name"] for row in rows}

    def test_the_owned_list_records_the_match_and_its_score(self, config_file, export, tmp_path):
        destination = tmp_path / "owned.csv"

        invoke(config_file, "wantlist", "diff", str(export), "--owned", str(destination))

        with destination.open(encoding="utf-8-sig", newline="") as handle:
            rows = list(csv.DictReader(handle))
        assert all(row["matched_path"] for row in rows)
        assert all(0.0 <= float(row["score"]) <= 1.0 for row in rows)

    def test_a_file_without_the_needed_columns_fails_clearly(self, config_file, tmp_path):
        bad = tmp_path / "bad.csv"
        bad.write_text("Foo,Bar\n1,2\n", encoding="utf-8")

        result = invoke(config_file, "wantlist", "diff", str(bad))

        assert result.exit_code == 1
        assert "artist and a title" in result.output

    def test_an_empty_index_is_called_out(self, tmp_path, library_root, export):
        path = tmp_path / CONFIG_FILENAME
        path.write_text(
            f'[library]\nroot = "{library_root.as_posix()}"\n\n'
            f'[index]\npath = "{(tmp_path / "empty.db").as_posix()}"\n\n'
            f'[review]\npath = "{(tmp_path / "r.db").as_posix()}"\n',
            encoding="utf-8",
        )

        result = invoke(path, "wantlist", "diff", str(export))

        assert "library scan" in result.stdout


class TestReviewCommands:
    @pytest.fixture
    def queued(self, config_file, export):
        invoke(config_file, "wantlist", "diff", str(export))
        return config_file

    def test_list_shows_the_queue(self, queued):
        payload = json.loads(invoke(queued, "--json", "review", "list").stdout)

        assert payload["tracks"]
        assert payload["counts"]["pending"] == len(payload["tracks"])

    def test_list_can_filter_by_status(self, queued):
        payload = json.loads(
            invoke(queued, "--json", "review", "list", "--status", "approved").stdout
        )

        assert payload["tracks"] == []

    def test_an_empty_queue_says_so(self, config_file):
        assert "Nothing in the review queue" in invoke(config_file, "review", "list").stdout

    def test_rejecting_records_the_reason(self, queued):
        track_id = json.loads(invoke(queued, "--json", "review", "list").stdout)["tracks"][0]["id"]

        invoke(queued, "review", "reject", str(track_id), "--reason", "live only")

        entry = json.loads(invoke(queued, "--json", "review", "list").stdout)["tracks"][0]
        assert entry["status"] == "rejected"
        assert entry["detail"] == "live only"

    def test_rejecting_an_unknown_track_fails(self, queued):
        result = invoke(queued, "review", "reject", "999")

        assert result.exit_code == 1
        assert "no track 999" in result.output

    def test_approving_a_track_with_no_candidates_fails(self, queued):
        track_id = json.loads(invoke(queued, "--json", "review", "list").stdout)["tracks"][0]["id"]

        result = invoke(queued, "review", "approve", str(track_id), "1")

        assert result.exit_code == 1

    def test_resetting_returns_a_track_to_the_queue(self, queued):
        track_id = json.loads(invoke(queued, "--json", "review", "list").stdout)["tracks"][0]["id"]
        invoke(queued, "review", "reject", str(track_id))

        invoke(queued, "review", "reset", str(track_id))

        entry = json.loads(invoke(queued, "--json", "review", "list").stdout)["tracks"][0]
        assert entry["status"] == "pending"


class TestLegacyImport:
    @pytest.fixture
    def legacy_csv(self, tmp_path):
        path = tmp_path / "soulseek_review.csv"
        path.write_text(
            LEGACY_REVIEW_HEADER
            + "\n"
            + "DECO*27 - Roki,DECO*27,Roki,,"
            + "9000,desc1,peerA,@@a\\Music\\Roki.flac,30000000,1000,44100,16,0,"
            + "8000,desc2,peerB,@@b\\Music\\Roki.mp3,9000000,320,44100,,1,"
            + ",,,,,,,,,"
            + "2\n"
            + "ZUN - Heian Alien,ZUN,Heian Alien,,"
            + ",,,,,,,,,"
            + ",,,,,,,,,"
            + ",,,,,,,,,"
            + "\n",
            encoding="utf-8-sig",
        )
        return path

    def test_tracks_and_approvals_carry_over(self, config_file, legacy_csv):
        result = invoke(config_file, "review", "import", str(legacy_csv))

        assert "2 new track(s)" in result.stdout
        assert "1 approval(s)" in result.stdout

    def test_the_approved_candidate_is_the_one_the_csv_named(self, config_file, legacy_csv):
        invoke(config_file, "review", "import", str(legacy_csv))

        tracks = json.loads(invoke(config_file, "--json", "review", "list").stdout)["tracks"]
        approved = next(entry for entry in tracks if entry["status"] == "approved")
        assert approved["approved_rank"] == 2
        assert approved["candidates"][1]["username"] == "peerB"

    def test_a_row_with_no_candidates_is_marked_as_such(self, config_file, legacy_csv):
        invoke(config_file, "review", "import", str(legacy_csv))

        tracks = json.loads(invoke(config_file, "--json", "review", "list").stdout)["tracks"]
        assert any(entry["status"] == "no_candidates" for entry in tracks)

    def test_importing_a_missing_file_fails_clearly(self, config_file, tmp_path):
        result = invoke(config_file, "review", "import", str(tmp_path / "absent.csv"))

        assert result.exit_code == 1
        assert "no such file" in result.output
