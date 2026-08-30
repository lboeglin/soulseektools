"""Review app tests, driven through Textual's pilot harness."""

import pytest

from soulseektools.models import Candidate, ScoredCandidate, Track, TrackStatus
from soulseektools.review.store import ReviewStore
from soulseektools.tui.app import (
    EMPTY_QUEUE_MESSAGE,
    ReviewApp,
    render_candidates,
    render_summary,
)


def scored(rank: int, *, folder="Music", flags=()) -> ScoredCandidate:
    return ScoredCandidate(
        candidate=Candidate(
            username=f"peer{rank}",
            remote_path=f"@@peer{rank}\\{folder}\\03 - Roki.flac",
            size=30_000_000,
            bit_depth=16,
            sample_rate=44100,
            free_upload_slots=rank == 1,
            queue_length=rank,
        ),
        score=1000 - rank,
        title_ratio=0.99,
        flags=tuple(flags),
    )


@pytest.fixture
def store(tmp_path):
    with ReviewStore(tmp_path / "review.db") as opened:
        opened.add(
            [
                Track(artist="DECO*27", title="Roki", album="Ghost", duration_seconds=215.0),
                Track(artist="ZUN", title="Heian Alien"),
            ]
        )
        first, second = opened.tracks()
        opened.set_candidates(
            first.id, [scored(1), scored(2, folder="Live Concert", flags=("live",)), scored(3)]
        )
        opened.set_candidates(second.id, [])
        yield opened


def track_ids(store):
    return [entry.id for entry in store.tracks()]


class TestLayout:
    async def test_only_reviewable_tracks_are_listed_by_default(self, store):
        async with ReviewApp(store).run_test() as pilot:
            assert len(pilot.app.entries) == 1
            assert pilot.app.entries[0].status is TrackStatus.AWAITING_REVIEW

    async def test_pressing_a_shows_everything_including_dead_ends(self, store):
        async with ReviewApp(store).run_test() as pilot:
            await pilot.press("a")

            assert len(pilot.app.entries) == 2
            assert TrackStatus.NO_CANDIDATES in {entry.status for entry in pilot.app.entries}

    def test_the_summary_counts_the_whole_queue(self, store):
        summary = render_summary(store)

        assert "awaiting_review" in summary
        assert "no_candidates" in summary

    def test_an_empty_queue_summarises_as_empty(self, tmp_path):
        with ReviewStore(tmp_path / "empty.db") as empty:
            assert render_summary(empty) == "queue is empty"


class TestCandidateDisplay:
    @pytest.fixture
    def panel(self, store):
        return render_candidates(store.tracks(TrackStatus.AWAITING_REVIEW)[0])

    def test_every_candidates_full_remote_path_is_shown(self, panel):
        # The whole point of the panel: a filename cannot distinguish a studio
        # track from one sitting in a concert folder, and the CSV workflow this
        # replaces showed only the filename.
        assert r"@@peer1\Music\03 - Roki.flac" in panel
        assert r"@@peer2\Live Concert\03 - Roki.flac" in panel

    def test_warnings_are_shown_against_the_candidate_that_raised_them(self, panel):
        assert "warnings: live" in panel

    def test_format_and_peer_details_are_shown(self, panel):
        assert "44.1kHz/16bit" in panel
        assert "user=peer1" in panel
        assert "slot=free" in panel

    def test_the_wanted_track_is_named_with_its_album_and_length(self, panel):
        assert "DECO*27 - Roki" in panel
        assert "Ghost" in panel
        assert "215s" in panel

    def test_the_approved_candidate_is_marked(self, store):
        entry = store.tracks(TrackStatus.AWAITING_REVIEW)[0]
        store.approve(entry.id, 2)

        assert "[green]*[/][bold]2.[/]" in render_candidates(store.track(entry.id))

    def test_a_track_with_no_candidates_says_so(self, store):
        assert "No candidates" in render_candidates(store.tracks(TrackStatus.NO_CANDIDATES)[0])


class TestDecisions:
    async def test_pressing_a_number_approves_that_candidate(self, store):
        first = track_ids(store)[0]

        async with ReviewApp(store).run_test() as pilot:
            await pilot.press("2")

        entry = store.track(first)
        assert entry.status is TrackStatus.APPROVED
        assert entry.approved_rank == 2
        assert entry.approved.candidate.username == "peer2"

    async def test_pressing_r_rejects_the_track(self, store):
        first = track_ids(store)[0]

        async with ReviewApp(store).run_test() as pilot:
            await pilot.press("r")

        assert store.track(first).status is TrackStatus.REJECTED

    async def test_approving_a_candidate_that_does_not_exist_changes_nothing(self, store):
        first = track_ids(store)[0]
        store.set_candidates(first, [scored(1)])

        async with ReviewApp(store).run_test() as pilot:
            await pilot.press("3")

        assert store.track(first).status is TrackStatus.AWAITING_REVIEW

    async def test_a_decision_can_be_undone(self, store):
        first = track_ids(store)[0]

        async with ReviewApp(store).run_test() as pilot:
            await pilot.press("1")
            await pilot.press("u")

        entry = store.track(first)
        assert entry.status is TrackStatus.AWAITING_REVIEW
        assert entry.approved_rank is None
        assert len(entry.candidates) == 3

    async def test_the_list_shrinks_as_decisions_are_made(self, store):
        async with ReviewApp(store).run_test() as pilot:
            assert len(pilot.app.entries) == 1

            await pilot.press("1")

            # The approved track stays visible so a change of mind is possible.
            assert pilot.app.entries[0].status is TrackStatus.APPROVED

    async def test_an_empty_queue_selects_nothing(self, tmp_path):
        with ReviewStore(tmp_path / "empty.db") as empty:
            async with ReviewApp(empty).run_test() as pilot:
                assert pilot.app.entries == []
                assert pilot.app.selected is None

    def test_the_empty_queue_message_says_what_to_run(self):
        assert "fetch search" in EMPTY_QUEUE_MESSAGE

    async def test_pressing_q_quits(self, store):
        async with ReviewApp(store).run_test() as pilot:
            await pilot.press("q")

            assert not pilot.app.is_running
