"""Batch orchestration, driven by a fake backend.

No slskd, no network and no Soulseek account: the flows are exercised against
the SoulseekClient protocol, which is what the protocol exists for.
"""

import subprocess
from pathlib import Path

import pytest

from soulseektools import fetch
from soulseektools.config import Settings
from soulseektools.errors import SoulseekClientError
from soulseektools.models import Candidate, Track, TrackStatus, Verdict
from soulseektools.review.store import ReviewStore
from soulseektools.soulseek.base import Transfer, TransferState
from tests.conftest import FFMPEG


class FakeClient:
    """A SoulseekClient that answers from canned data and records what it was asked."""

    def __init__(self, results=None, transfers=None, *, search_error=None, enqueue_error=None):
        self.results = results or []
        self._transfers = transfers or []
        self.search_error = search_error
        self.enqueue_error = enqueue_error
        self.searches: list[str] = []
        self.enqueued: list[tuple[str, str, str]] = []

    def is_connected(self) -> bool:
        return True

    def search(self, text, *, timeout_seconds=15, response_limit=200):
        self.searches.append(text)
        if self.search_error:
            raise SoulseekClientError(self.search_error)
        return list(self.results)

    def enqueue(self, username, files, *, destination=""):
        if self.enqueue_error:
            raise SoulseekClientError(self.enqueue_error)
        self.enqueued.extend((username, file.remote_path, destination) for file in files)
        return []

    def transfers(self):
        return list(self._transfers)


def _reencode(source, destination, codec, *extra):
    """Re-encode one file into another container, for building a fake lossless file."""
    subprocess.run(
        [
            FFMPEG,
            "-v",
            "error",
            "-y",
            "-i",
            str(source),
            "-c:a",
            codec,
            *extra,
            "-ac",
            "2",
            str(destination),
        ],
        check=True,
        capture_output=True,
    )
    return destination


@pytest.fixture
def store(tmp_path):
    with ReviewStore(tmp_path / "review.db") as opened:
        yield opened


@pytest.fixture
def settings(tmp_path):
    return Settings(slskd={"url": "http://x", "download_directory": tmp_path / "downloads"})


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


def transfer(path, state=TransferState.COMPLETED, **overrides) -> Transfer:
    defaults = {
        "id": "t1",
        "username": "peerA",
        "remote_path": path,
        "state": state,
        "size": 30_000_000,
        "bytes_transferred": 30_000_000,
    }
    return Transfer(**{**defaults, **overrides})


class TestSearchPending:
    def test_candidates_are_ranked_and_stored(self, store, settings):
        store.add([Track(artist="DECO*27", title="Roki")])
        client = FakeClient(
            [
                candidate("@@peerA\\Live Concert\\03 - Roki.flac"),
                candidate("@@peerA\\Music\\Ghost\\03 - Roki.flac"),
            ]
        )

        outcomes = fetch.search_pending(store, client, settings)

        assert outcomes[0].found == 2
        entry = store.tracks()[0]
        assert entry.status is TrackStatus.AWAITING_REVIEW
        assert "Live Concert" not in entry.candidates[0].candidate.remote_path

    def test_the_search_text_is_normalised(self, store, settings):
        store.add([Track(artist="DECO*27", title="ロキ")])
        client = FakeClient()

        fetch.search_pending(store, client, settings)

        assert client.searches == ["deco 27 ロキ"]

    def test_only_pending_tracks_are_searched(self, store, settings):
        store.add([Track(artist="A", title="One"), Track(artist="B", title="Two")])
        store.mark(store.tracks()[0].id, TrackStatus.REJECTED)
        client = FakeClient()

        fetch.search_pending(store, client, settings)

        assert len(client.searches) == 1

    def test_a_track_with_no_results_is_marked_rather_than_left_pending(self, store, settings):
        store.add([Track(artist="A", title="One")])

        fetch.search_pending(store, FakeClient([]), settings)

        assert store.tracks()[0].status is TrackStatus.NO_CANDIDATES

    def test_the_number_of_candidates_kept_is_configurable(self, store, settings):
        store.add([Track(artist="DECO*27", title="Roki")])
        client = FakeClient([candidate(f"@@peer{i}\\Music\\03 - Roki.flac") for i in range(10)])
        settings = settings.model_copy(
            update={"review": settings.review.model_copy(update={"candidates_per_track": 2})}
        )

        fetch.search_pending(store, client, settings)

        assert len(store.tracks()[0].candidates) == 2

    def test_a_backend_failure_marks_the_track_and_continues(self, store, settings):
        store.add([Track(artist="A", title="One"), Track(artist="B", title="Two")])
        client = FakeClient(search_error="slskd unreachable")

        outcomes = fetch.search_pending(store, client, settings)

        assert len(outcomes) == 2
        assert all(entry.status is TrackStatus.FAILED for entry in store.tracks())
        assert "unreachable" in outcomes[0].error

    def test_progress_is_reported_per_track(self, store, settings):
        store.add([Track(artist="A", title="One"), Track(artist="B", title="Two")])
        seen = []

        fetch.search_pending(store, FakeClient(), settings, on_track=seen.append)

        assert len(seen) == 2

    def test_the_number_searched_can_be_capped(self, store, settings):
        store.add([Track(artist="A", title=str(i)) for i in range(5)])
        client = FakeClient()

        fetch.search_pending(store, client, settings, limit=2)

        assert len(client.searches) == 2


class TestEnqueueApproved:
    @pytest.fixture
    def approved(self, store, settings):
        store.add([Track(artist="DECO*27", title="Roki")])
        fetch.search_pending(store, FakeClient([candidate("@@peerA\\Music\\Roki.flac")]), settings)
        store.approve(store.tracks()[0].id, 1)
        return store.tracks()[0].id

    def test_the_approved_candidate_is_queued(self, store, approved):
        client = FakeClient()

        outcomes = fetch.enqueue_approved(store, client)

        assert outcomes[0].queued is True
        assert client.enqueued == [("peerA", "@@peerA\\Music\\Roki.flac", "DECO_27")]

    def test_the_destination_folder_is_safe_for_the_filesystem(self, store, approved):
        client = FakeClient()

        fetch.enqueue_approved(store, client)

        assert client.enqueued[0][2] == "DECO_27"

    def test_the_track_moves_to_queued(self, store, approved):
        fetch.enqueue_approved(store, FakeClient())

        assert store.track(approved).status is TrackStatus.QUEUED

    def test_nothing_unapproved_is_queued(self, store, settings):
        store.add([Track(artist="A", title="One")])
        client = FakeClient()

        assert fetch.enqueue_approved(store, client) == []
        assert client.enqueued == []

    def test_a_backend_failure_marks_the_track(self, store, approved):
        client = FakeClient(enqueue_error="peer offline")

        outcomes = fetch.enqueue_approved(store, client)

        assert outcomes[0].queued is False
        assert store.track(approved).status is TrackStatus.FAILED
        assert "peer offline" in store.track(approved).detail


class TestCollect:
    @pytest.fixture
    def queued(self, store, settings):
        store.add([Track(artist="DECO*27", title="Roki")])
        fetch.search_pending(store, FakeClient([candidate("@@peerA\\Music\\Roki.flac")]), settings)
        track_id = store.tracks()[0].id
        store.approve(track_id, 1)
        fetch.enqueue_approved(store, FakeClient())
        return track_id

    def test_nothing_is_recorded_while_a_transfer_is_still_running(self, store, settings, queued):
        client = FakeClient(
            transfers=[transfer("@@peerA\\Music\\Roki.flac", TransferState.IN_PROGRESS)]
        )

        assert fetch.collect(store, client, settings) == []
        assert store.track(queued).status is TrackStatus.QUEUED

    def test_a_completed_transfer_is_recorded(self, store, settings, queued):
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.flac")])

        outcomes = fetch.collect(store, client, settings, verify=False)

        assert len(outcomes) == 1
        assert store.track(queued).status is TrackStatus.DOWNLOADED

    def test_a_failed_transfer_is_recorded_as_a_failure(self, store, settings, queued):
        client = FakeClient(
            transfers=[
                transfer("@@peerA\\Music\\Roki.flac", TransferState.FAILED, error="peer gone")
            ]
        )

        outcomes = fetch.collect(store, client, settings)

        assert outcomes[0].state is TransferState.FAILED
        assert store.track(queued).status is TrackStatus.FAILED
        assert "peer gone" in store.track(queued).detail

    def test_a_transfer_for_another_track_is_ignored(self, store, settings, queued):
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Something Else.flac")])

        assert fetch.collect(store, client, settings) == []

    def test_the_local_path_is_rebuilt_from_the_destination(self, store, settings, queued):
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.flac")])

        outcome = fetch.collect(store, client, settings, verify=False)[0]

        assert outcome.local_path == settings.slskd.download_directory / "DECO_27" / "Roki.flac"

    def test_no_local_path_is_guessed_without_a_configured_download_directory(self, store, queued):
        bare = Settings()
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.flac")])

        assert fetch.collect(store, client, bare)[0].local_path is None

    def test_a_downloaded_lossless_file_is_verified(self, store, settings, queued, encode_audio):
        destination = settings.slskd.download_directory / "DECO_27" / "Roki.flac"
        encode_audio(destination, seconds=6.0, noise=True)
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.flac")])

        outcome = fetch.collect(store, client, settings)[0]

        assert outcome.verdict is not None
        assert outcome.verdict.verdict is Verdict.OK
        assert store.track(queued).detail == "downloaded, verified lossless"

    def test_a_downloaded_transcode_is_recorded_against_the_track(
        self, store, settings, queued, encode_audio, tmp_path
    ):
        # The check that mattered most in practice: a file that arrives as FLAC
        # but was really an MP3 is flagged the moment it lands.
        source = encode_audio(tmp_path / "source.flac", seconds=6.0, noise=True)
        lossy = _reencode(source, tmp_path / "source.mp3", "libmp3lame", "-b:a", "128k")
        destination = settings.slskd.download_directory / "DECO_27" / "Roki.flac"
        destination.parent.mkdir(parents=True, exist_ok=True)
        _reencode(lossy, destination, "flac")
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.flac")])

        outcome = fetch.collect(store, client, settings)[0]

        assert outcome.verdict is not None
        assert outcome.verdict.verdict is Verdict.FAKE
        assert "FAKE" in store.track(queued).detail

    def test_a_missing_file_is_recorded_without_a_verdict(self, store, settings, queued):
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.flac")])

        outcome = fetch.collect(store, client, settings)[0]

        assert outcome.verdict is None
        assert store.track(queued).status is TrackStatus.DOWNLOADED

    def test_verification_can_be_skipped(self, store, settings, queued, encode_audio):
        encode_audio(settings.slskd.download_directory / "DECO_27" / "Roki.flac")
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.flac")])

        assert fetch.collect(store, client, settings, verify=False)[0].verdict is None

    def test_a_lossy_download_is_not_put_through_the_lossless_check(
        self, store, settings, encode_audio
    ):
        store.add([Track(artist="DECO*27", title="Roki")])
        fetch.search_pending(
            store,
            FakeClient([candidate("@@peerA\\Music\\Roki.mp3", bit_depth=None, bitrate=320)]),
            settings,
        )
        store.approve(store.tracks()[0].id, 1)
        fetch.enqueue_approved(store, FakeClient())
        encode_audio(settings.slskd.download_directory / "DECO_27" / "Roki.mp3")
        client = FakeClient(transfers=[transfer("@@peerA\\Music\\Roki.mp3")])

        assert fetch.collect(store, client, settings)[0].verdict is None

    def test_no_transfers_at_all_does_nothing(self, store, settings, queued):
        assert fetch.collect(store, FakeClient(), settings) == []


def test_the_fake_satisfies_the_client_protocol():
    from soulseektools.soulseek.base import SoulseekClient

    def accepts(_client: SoulseekClient) -> None:
        return None

    accepts(FakeClient())


def test_downloaded_path_keeps_only_the_filename(tmp_path):
    settings = Settings(slskd={"download_directory": tmp_path})

    path = fetch.downloaded_path(
        settings, "DECO*27", transfer("@@peer\\deep\\nested\\03 - Roki.flac")
    )

    assert path == tmp_path / "DECO_27" / "03 - Roki.flac"
    assert Path(path).name == "03 - Roki.flac"
