import pytest

from soulseektools.models import Candidate, ScoredCandidate, Track, TrackStatus
from soulseektools.review.store import ReviewStore


@pytest.fixture
def store(tmp_path):
    with ReviewStore(tmp_path / "review.db") as opened:
        yield opened


def track(artist="DECO*27", title="Roki") -> Track:
    return Track(artist=artist, title=title, album="Ghost", duration_seconds=215.0)


def scored(rank: int, *, flags=()) -> ScoredCandidate:
    return ScoredCandidate(
        candidate=Candidate(
            username=f"peer{rank}",
            remote_path=f"@@x\\Music\\{rank}\\Roki.flac",
            size=30_000_000 - rank,
            bitrate=1000,
            sample_rate=44100,
            bit_depth=16,
            free_upload_slots=rank == 1,
            queue_length=rank,
            duration_seconds=215.0,
        ),
        score=1000 - rank,
        title_ratio=0.99,
        flags=tuple(flags),
    )


class TestAdd:
    def test_tracks_are_queued_as_pending(self, store):
        assert store.add([track()]) == 1

        queued = store.tracks()
        assert len(queued) == 1
        assert queued[0].status is TrackStatus.PENDING
        assert queued[0].track.album == "Ghost"

    def test_the_same_track_is_not_queued_twice(self, store):
        store.add([track()])

        assert store.add([track()]) == 0
        assert len(store.tracks()) == 1

    def test_the_original_export_row_survives_the_round_trip(self, store):
        store.add([Track(artist="A", title="B", source_row={"Added at": "2026-01-01"})])

        assert store.tracks()[0].track.source_row == {"Added at": "2026-01-01"}


class TestCandidates:
    def test_recording_candidates_moves_a_track_to_review(self, store):
        store.add([track()])
        track_id = store.tracks()[0].id

        store.set_candidates(track_id, [scored(1), scored(2)])

        reviewed = store.track(track_id)
        assert reviewed is not None
        assert reviewed.status is TrackStatus.AWAITING_REVIEW
        assert len(reviewed.candidates) == 2

    def test_candidates_round_trip_with_their_flags(self, store):
        store.add([track()])
        track_id = store.tracks()[0].id

        store.set_candidates(track_id, [scored(1, flags=("live", "remix"))])

        stored = store.track(track_id)
        assert stored is not None
        assert stored.candidates[0].flags == ("live", "remix")
        assert stored.candidates[0].candidate.free_upload_slots is True

    def test_no_candidates_is_a_distinct_state(self, store):
        store.add([track()])
        track_id = store.tracks()[0].id

        store.set_candidates(track_id, [])

        stored = store.track(track_id)
        assert stored is not None
        assert stored.status is TrackStatus.NO_CANDIDATES
        assert "no usable" in stored.detail

    def test_a_second_search_replaces_the_first(self, store):
        store.add([track()])
        track_id = store.tracks()[0].id
        store.set_candidates(track_id, [scored(1), scored(2), scored(3)])

        store.set_candidates(track_id, [scored(1)])

        stored = store.track(track_id)
        assert stored is not None
        assert len(stored.candidates) == 1


class TestApproval:
    @pytest.fixture
    def reviewable(self, store):
        store.add([track()])
        track_id = store.tracks()[0].id
        store.set_candidates(track_id, [scored(1), scored(2), scored(3)])
        return track_id

    def test_approving_records_the_choice(self, store, reviewable):
        store.approve(reviewable, 2)

        stored = store.track(reviewable)
        assert stored is not None
        assert stored.status is TrackStatus.APPROVED
        assert stored.approved_rank == 2
        assert stored.approved is not None
        assert stored.approved.candidate.username == "peer2"

    def test_changing_the_choice_clears_the_previous_one(self, store, reviewable):
        store.approve(reviewable, 1)

        store.approve(reviewable, 3)

        stored = store.track(reviewable)
        assert stored is not None
        assert stored.approved_rank == 3

    def test_approving_a_candidate_that_does_not_exist_is_an_error(self, store, reviewable):
        with pytest.raises(KeyError):
            store.approve(reviewable, 9)

        stored = store.track(reviewable)
        assert stored is not None
        assert stored.status is TrackStatus.AWAITING_REVIEW

    def test_rejecting_records_the_reason_and_clears_any_choice(self, store, reviewable):
        store.approve(reviewable, 1)

        store.reject(reviewable, "all candidates are live recordings")

        stored = store.track(reviewable)
        assert stored is not None
        assert stored.status is TrackStatus.REJECTED
        assert stored.approved is None
        assert "live recordings" in stored.detail

    def test_a_track_can_be_returned_to_the_queue(self, store, reviewable):
        store.approve(reviewable, 1)

        store.reset(reviewable)

        stored = store.track(reviewable)
        assert stored is not None
        assert stored.status is TrackStatus.PENDING
        assert stored.candidates == ()


class TestQueries:
    def test_tracks_can_be_filtered_by_status(self, store):
        store.add([track("A", "One"), track("B", "Two")])
        store.mark(store.tracks()[0].id, TrackStatus.QUEUED)

        assert len(store.tracks(TrackStatus.PENDING)) == 1
        assert len(store.tracks(TrackStatus.QUEUED)) == 1
        assert len(store.tracks(TrackStatus.PENDING, TrackStatus.QUEUED)) == 2

    def test_the_limit_is_honoured(self, store):
        store.add([track("A", str(index)) for index in range(5)])

        assert len(store.tracks(limit=2)) == 2

    def test_counts_summarise_the_queue(self, store):
        store.add([track("A", "One"), track("B", "Two")])
        store.mark(store.tracks()[0].id, TrackStatus.DOWNLOADED)

        assert store.counts() == {TrackStatus.PENDING: 1, TrackStatus.DOWNLOADED: 1}

    def test_an_unknown_track_id_returns_nothing(self, store):
        assert store.track(999) is None

    def test_the_queue_survives_being_reopened(self, tmp_path):
        database = tmp_path / "review.db"
        with ReviewStore(database) as first:
            first.add([track()])
            first.set_candidates(first.tracks()[0].id, [scored(1)])
            first.approve(first.tracks()[0].id, 1)

        with ReviewStore(database) as second:
            stored = second.tracks()[0]
            assert stored.status is TrackStatus.APPROVED
            assert stored.approved is not None
