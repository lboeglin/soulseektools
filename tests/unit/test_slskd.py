"""slskd client tests.

The payloads here follow the shapes taken from a running slskd container's
OpenAPI document and its bundled web UI, so a change in slskd's wire format
shows up as a failure here rather than at runtime.
"""

import json
from typing import ClassVar

import httpx
import pytest
import respx

from soulseektools.config import SlskdSettings
from soulseektools.errors import SoulseekClientError
from soulseektools.models import Candidate
from soulseektools.soulseek.base import TransferState
from soulseektools.soulseek.slskd import API_KEY_HEADER, SlskdClient

BASE = "http://slskd.test:5030"
API = f"{BASE}/api/v0"


def settings(**overrides) -> SlskdSettings:
    return SlskdSettings(**{"url": BASE, **overrides})


def client(**overrides) -> SlskdClient:
    return SlskdClient(settings(**overrides))


SEARCH_RESPONSE = [
    {
        "username": "peerA",
        "hasFreeUploadSlot": True,
        "queueLength": 0,
        "uploadSpeed": 500000,
        "fileCount": 2,
        "lockedFileCount": 0,
        "files": [
            {
                "filename": "@@peerA\\Music\\Ghost\\03 - Roki.flac",
                "size": 30_000_000,
                "extension": ".flac",
                "bitRate": 0,
                "bitDepth": 16,
                "sampleRate": 44100,
                "length": 215,
                "isVariableBitRate": False,
            },
            {
                "filename": "@@peerA\\Music\\Ghost\\cover.jpg",
                "size": 40_000,
                "extension": ".jpg",
            },
        ],
    },
    {
        "username": "peerB",
        "hasFreeUploadSlot": False,
        "queueLength": 12,
        "files": [
            {
                "filename": "@@peerB\\mp3\\Roki.mp3",
                "size": 8_000_000,
                "extension": ".mp3",
                "bitRate": 320,
                "isVariableBitRate": True,
            }
        ],
    },
]


def mock_search(responses=None, *, complete=True):
    respx.post(f"{API}/searches").mock(return_value=httpx.Response(200, json={"id": "x"}))
    respx.get(url__regex=rf"{API}/searches/[^/]+$").mock(
        return_value=httpx.Response(200, json={"isComplete": complete, "responseCount": 2})
    )
    respx.get(url__regex=rf"{API}/searches/[^/]+/responses$").mock(
        return_value=httpx.Response(
            200, json=responses if responses is not None else SEARCH_RESPONSE
        )
    )
    respx.delete(url__regex=rf"{API}/searches/[^/]+$").mock(return_value=httpx.Response(204))


class TestAuthentication:
    @respx.mock
    def test_the_api_key_is_sent_as_a_header(self):
        route = respx.get(f"{API}/server").mock(
            return_value=httpx.Response(200, json={"isConnected": True, "isLoggedIn": True})
        )

        client(api_key="s3cret").is_connected()

        assert route.calls.last.request.headers[API_KEY_HEADER] == "s3cret"

    @respx.mock
    def test_no_header_is_sent_when_no_key_is_configured(self):
        route = respx.get(f"{API}/server").mock(
            return_value=httpx.Response(200, json={"isConnected": True, "isLoggedIn": True})
        )

        client().is_connected()

        assert API_KEY_HEADER not in route.calls.last.request.headers

    @respx.mock
    def test_a_rejected_key_is_reported_with_what_to_check(self):
        respx.get(f"{API}/searches").mock(return_value=httpx.Response(401))

        with pytest.raises(SoulseekClientError, match="SLSKD_API_KEY"):
            client()._get("/searches")


class TestIsConnected:
    @respx.mock
    def test_true_when_logged_in(self):
        respx.get(f"{API}/server").mock(
            return_value=httpx.Response(200, json={"isConnected": True, "isLoggedIn": True})
        )

        assert client().is_connected() is True

    @respx.mock
    def test_false_when_connected_but_not_logged_in(self):
        respx.get(f"{API}/server").mock(
            return_value=httpx.Response(200, json={"isConnected": True, "isLoggedIn": False})
        )

        assert client().is_connected() is False

    @respx.mock
    def test_false_when_the_daemon_is_unreachable(self):
        respx.get(f"{API}/server").mock(side_effect=httpx.ConnectError("refused"))

        assert client().is_connected() is False


class TestSearch:
    @respx.mock
    def test_files_from_every_peer_are_returned(self):
        mock_search()

        candidates = client().search("DECO*27 Roki")

        assert len(candidates) == 3
        assert {candidate.username for candidate in candidates} == {"peerA", "peerB"}

    @respx.mock
    def test_file_attributes_are_carried_through(self):
        mock_search()

        flac = next(c for c in client().search("x") if c.extension == "flac")

        assert flac.size == 30_000_000
        assert flac.bit_depth == 16
        assert flac.sample_rate == 44100
        assert flac.duration_seconds == pytest.approx(215.0)
        assert flac.is_vbr is False

    @respx.mock
    def test_peer_availability_is_carried_through(self):
        mock_search()

        candidates = {c.username: c for c in client().search("x")}

        assert candidates["peerA"].free_upload_slots is True
        assert candidates["peerB"].free_upload_slots is False
        assert candidates["peerB"].queue_length == 12
        assert candidates["peerB"].is_vbr is True

    @respx.mock
    def test_a_zero_bitrate_is_treated_as_unknown_not_as_zero(self):
        mock_search()

        flac = next(c for c in client().search("x") if c.extension == "flac")

        assert flac.bitrate is None

    @respx.mock
    def test_the_search_text_and_timeout_are_sent(self):
        mock_search()
        route = respx.post(f"{API}/searches")

        client().search("DECO*27 Roki", timeout_seconds=20)

        body = json.loads(route.calls.last.request.content)
        assert body["searchText"] == "DECO*27 Roki"
        assert body["searchTimeout"] == 20

    @respx.mock
    def test_a_timeout_below_the_daemon_minimum_is_raised_to_it(self):
        mock_search()
        route = respx.post(f"{API}/searches")

        client().search("x", timeout_seconds=1)

        assert json.loads(route.calls.last.request.content)["searchTimeout"] == 5

    @respx.mock
    def test_the_search_is_cleaned_up_afterwards(self):
        mock_search()
        route = respx.delete(url__regex=rf"{API}/searches/[^/]+$")

        client().search("x")

        assert route.called

    @respx.mock
    def test_a_failure_to_clean_up_does_not_lose_the_results(self):
        mock_search()
        respx.delete(url__regex=rf"{API}/searches/[^/]+$").mock(return_value=httpx.Response(500))

        assert len(client().search("x")) == 3

    @respx.mock
    def test_no_responses_yields_no_candidates(self):
        mock_search(responses=[])

        assert client().search("x") == []

    @respx.mock
    def test_a_response_with_no_files_is_skipped(self):
        mock_search(responses=[{"username": "peerC", "files": []}])

        assert client().search("x") == []

    @respx.mock
    def test_an_unreachable_daemon_is_reported_with_its_url(self):
        respx.post(f"{API}/searches").mock(side_effect=httpx.ConnectError("refused"))

        with pytest.raises(SoulseekClientError, match=BASE):
            client().search("x")


class TestEnqueue:
    FILES: ClassVar = [
        Candidate(username="peerA", remote_path="@@peerA\\Music\\Roki.flac", size=30_000_000)
    ]

    @respx.mock
    def test_a_destination_uses_the_batch_endpoint(self):
        route = respx.post(f"{API}/transfers/downloads/batches").mock(
            return_value=httpx.Response(
                200,
                json={
                    "batch": {
                        "id": "batch-1",
                        "username": "peerA",
                        "transfers": [
                            {
                                "id": "t1",
                                "username": "peerA",
                                "filename": "@@peerA\\Music\\Roki.flac",
                                "size": 30_000_000,
                                "state": "Queued, Remotely",
                            }
                        ],
                    },
                    "failures": [],
                },
            )
        )

        transfers = client().enqueue("peerA", self.FILES, destination="DECO_27")

        assert len(transfers) == 1
        assert transfers[0].state is TransferState.QUEUED
        body = json.loads(route.calls.last.request.content)
        assert body["options"]["destination"] == "DECO_27"
        assert body["files"] == [{"filename": r"@@peerA\Music\Roki.flac", "size": 30_000_000}]

    @respx.mock
    def test_a_batch_failure_is_raised_with_its_reason(self):
        respx.post(f"{API}/transfers/downloads/batches").mock(
            return_value=httpx.Response(
                200,
                json={
                    "batch": {},
                    "failures": [{"filename": "Roki.flac", "message": "peer offline"}],
                },
            )
        )

        with pytest.raises(SoulseekClientError, match="peer offline"):
            client().enqueue("peerA", self.FILES, destination="DECO_27")

    @respx.mock
    def test_without_a_destination_the_per_user_endpoint_is_used(self):
        route = respx.post(f"{API}/transfers/downloads/peerA").mock(
            return_value=httpx.Response(201)
        )
        respx.get(f"{API}/transfers/downloads").mock(return_value=httpx.Response(200, json=[]))

        client().enqueue("peerA", self.FILES)

        body = json.loads(route.calls.last.request.content)
        assert body == [{"filename": r"@@peerA\Music\Roki.flac", "size": 30_000_000}]

    @respx.mock
    def test_enqueueing_nothing_makes_no_request(self):
        route = respx.post(url__startswith=f"{API}/transfers")

        assert client().enqueue("peerA", []) == []
        assert not route.called


class TestTransfers:
    DOWNLOADS: ClassVar = [
        {
            "username": "peerA",
            "directories": [
                {
                    "directory": "@@peerA\\Music",
                    "files": [
                        {
                            "id": "t1",
                            "username": "peerA",
                            "filename": "@@peerA\\Music\\Roki.flac",
                            "size": 30_000_000,
                            "bytesTransferred": 30_000_000,
                            "state": "Completed, Succeeded",
                        },
                        {
                            "id": "t2",
                            "username": "peerA",
                            "filename": "@@peerA\\Music\\Other.flac",
                            "size": 20_000_000,
                            "bytesTransferred": 5_000_000,
                            "state": "InProgress",
                            "placeInQueue": 3,
                        },
                    ],
                }
            ],
        }
    ]

    @respx.mock
    def test_transfers_are_flattened_across_users_and_folders(self):
        respx.get(f"{API}/transfers/downloads").mock(
            return_value=httpx.Response(200, json=self.DOWNLOADS)
        )

        transfers = client().transfers()

        assert len(transfers) == 2
        assert {transfer.id for transfer in transfers} == {"t1", "t2"}

    @respx.mock
    def test_progress_is_reported(self):
        respx.get(f"{API}/transfers/downloads").mock(
            return_value=httpx.Response(200, json=self.DOWNLOADS)
        )

        transfers = {transfer.id: transfer for transfer in client().transfers()}

        assert transfers["t1"].percent_complete == pytest.approx(100.0)
        assert transfers["t2"].percent_complete == pytest.approx(25.0)
        assert transfers["t2"].place_in_queue == 3

    @pytest.mark.parametrize(
        ("raw", "expected"),
        [
            ("Completed, Succeeded", TransferState.COMPLETED),
            ("Completed, Errored", TransferState.FAILED),
            ("Completed, Cancelled", TransferState.CANCELLED),
            ("Completed, TimedOut", TransferState.FAILED),
            ("Completed, Rejected", TransferState.FAILED),
            ("InProgress", TransferState.IN_PROGRESS),
            ("Queued, Remotely", TransferState.QUEUED),
            ("Requested", TransferState.QUEUED),
            ("", TransferState.QUEUED),
        ],
    )
    @respx.mock
    def test_soulseek_state_flags_collapse_to_one_outcome(self, raw, expected):
        # slskd passes through Soulseek.NET's flag-like state strings; a
        # "Completed" that errored is a failure, not a completion.
        respx.get(f"{API}/transfers/downloads").mock(
            return_value=httpx.Response(
                200,
                json=[
                    {
                        "username": "p",
                        "directories": [{"files": [{"id": "t", "filename": "f", "state": raw}]}],
                    }
                ],
            )
        )

        assert client().transfers()[0].state is expected

    def test_finished_states_are_identified(self):
        assert TransferState.COMPLETED.finished
        assert TransferState.FAILED.finished
        assert TransferState.CANCELLED.finished
        assert not TransferState.QUEUED.finished
        assert not TransferState.IN_PROGRESS.finished

    @respx.mock
    def test_no_downloads_yields_nothing(self):
        respx.get(f"{API}/transfers/downloads").mock(return_value=httpx.Response(200, json=[]))

        assert client().transfers() == []
