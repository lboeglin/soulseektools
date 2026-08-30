"""Artwork lookup, with every request mocked -- these tests never touch the network."""

from pathlib import Path

import httpx
import pytest
import respx

from soulseektools.artwork import service
from soulseektools.artwork.base import ArtCandidate, select
from soulseektools.artwork.coverartarchive import CoverArtArchiveProvider
from soulseektools.artwork.itunes import SEARCH_URL as ITUNES_URL
from soulseektools.artwork.itunes import ITunesProvider
from soulseektools.config import ArtworkSettings
from soulseektools.errors import ConfigError
from soulseektools.matching import NameForms
from soulseektools.models import LibraryEntry, TrackTags

JPEG = b"\xff\xd8\xff" + b"padding" * 2000
PNG = b"\x89PNG\r\n\x1a\n" + b"padding" * 2000


def entry(artist="DECO*27", title="Roki", artwork_bytes=0) -> LibraryEntry:
    return LibraryEntry(
        path=Path(artist) / f"{title}.flac",
        size=1,
        mtime=1.0,
        folder_artist=artist,
        tags=TrackTags(artist=artist, title=title),
        artist=NameForms.of(artist),
        title=NameForms.of(title),
        artwork_bytes=artwork_bytes,
    )


def candidate(artist, title="Roki", source="itunes") -> ArtCandidate:
    return ArtCandidate(
        url=f"https://example.test/{artist}.jpg", artist=artist, title=title, source=source
    )


class TestSelect:
    def test_nothing_is_selected_from_an_empty_result_set(self):
        selection = select([], "DECO*27")

        assert not selection
        assert selection.reason == "no results"

    def test_a_matching_artist_is_selected(self):
        selection = select([candidate("DECO*27")], "DECO_27")

        assert selection
        assert selection.candidate is not None

    def test_the_wrong_artist_is_rejected_rather_than_embedded(self):
        # The defect this guard exists for: the original script embedded the
        # first result unchecked and pasted the wrong artist's cover twice.
        selection = select([candidate("Some Other Band")], "DECO*27")

        assert not selection
        assert "Some Other Band" in selection.reason
        assert "not 'DECO*27'" in selection.reason

    def test_the_best_matching_artist_wins_over_the_first_result(self):
        results = [candidate("Unrelated Artist"), candidate("DECO*27")]

        selection = select(results, "DECO*27")

        assert selection.candidate is not None
        assert selection.candidate.artist == "DECO*27"

    def test_verification_can_be_disabled(self):
        selection = select([candidate("Some Other Band")], "DECO*27", require_artist_match=False)

        assert selection
        assert "unverified" in selection.reason

    def test_the_threshold_is_configurable(self):
        results = [candidate("DECO 27 and friends")]

        assert not select(results, "DECO*27", artist_threshold=0.99)
        assert select(results, "DECO*27", artist_threshold=0.5)


class TestITunesProvider:
    @respx.mock
    def test_parses_results_and_upscales_the_thumbnail(self):
        respx.get(ITUNES_URL).mock(
            return_value=httpx.Response(
                200,
                json={
                    "results": [
                        {
                            "artistName": "DECO*27",
                            "trackName": "Roki",
                            "collectionName": "Ghost",
                            "artworkUrl100": "https://is1.test/a/100x100bb.jpg",
                        }
                    ]
                },
            )
        )

        with httpx.Client() as client:
            results = ITunesProvider().search(client, "DECO*27", "Roki")

        assert len(results) == 1
        assert results[0].url == "https://is1.test/a/1200x1200bb.jpg"
        assert results[0].album == "Ghost"

    @respx.mock
    def test_results_without_artwork_are_skipped(self):
        respx.get(ITUNES_URL).mock(
            return_value=httpx.Response(200, json={"results": [{"artistName": "X"}]})
        )

        with httpx.Client() as client:
            assert ITunesProvider().search(client, "X", "Y") == []

    @respx.mock
    def test_an_http_error_propagates_to_the_caller(self):
        respx.get(ITUNES_URL).mock(return_value=httpx.Response(503))

        with httpx.Client() as client, pytest.raises(httpx.HTTPError):
            ITunesProvider().search(client, "X", "Y")


class TestCoverArtArchiveProvider:
    @respx.mock
    def test_builds_an_archive_url_from_the_recordings_release(self):
        respx.get(url__startswith="https://musicbrainz.org/ws/2/recording").mock(
            return_value=httpx.Response(
                200,
                json={
                    "recordings": [
                        {
                            "title": "Roki",
                            "artist-credit": [{"name": "DECO*27"}],
                            "releases": [{"id": "abc-123", "title": "Ghost"}],
                        }
                    ]
                },
            )
        )

        with httpx.Client() as client:
            results = CoverArtArchiveProvider().search(client, "DECO*27", "Roki")

        assert results[0].url == "https://coverartarchive.org/release/abc-123/front-1200"
        assert results[0].artist == "DECO*27"

    @respx.mock
    def test_a_recording_with_no_release_is_skipped(self):
        respx.get(url__startswith="https://musicbrainz.org/ws/2/recording").mock(
            return_value=httpx.Response(200, json={"recordings": [{"title": "Roki"}]})
        )

        with httpx.Client() as client:
            assert CoverArtArchiveProvider().search(client, "X", "Y") == []


class TestAudit:
    def test_a_file_with_no_cover_needs_artwork(self):
        statuses = service.audit([entry(artwork_bytes=0)], ArtworkSettings())

        assert statuses[0].needs_artwork
        assert statuses[0].reason == "no embedded cover"

    def test_a_placeholder_sized_cover_counts_as_missing(self):
        statuses = service.audit([entry(artwork_bytes=800)], ArtworkSettings(min_bytes=3000))

        assert statuses[0].needs_artwork
        assert "800 bytes" in statuses[0].reason

    def test_a_real_cover_is_left_alone(self):
        statuses = service.audit([entry(artwork_bytes=50_000)], ArtworkSettings())

        assert not statuses[0].needs_artwork


class TestBuildProviders:
    def test_builds_the_configured_providers_in_order(self):
        providers = service.build_providers(
            ArtworkSettings(providers=("coverartarchive", "itunes"))
        )

        assert [provider.name for provider in providers] == ["coverartarchive", "itunes"]

    def test_an_unknown_provider_is_rejected_with_the_valid_names(self):
        with pytest.raises(ConfigError, match="itunes"):
            service.build_providers(ArtworkSettings(providers=("spotify",)))


class TestFindArtwork:
    @respx.mock
    def test_downloads_a_verified_match(self):
        respx.get(ITUNES_URL).mock(
            return_value=httpx.Response(
                200,
                json={
                    "results": [
                        {
                            "artistName": "DECO*27",
                            "trackName": "Roki",
                            "artworkUrl100": "https://is1.test/a/100x100bb.jpg",
                        }
                    ]
                },
            )
        )
        respx.get("https://is1.test/a/1200x1200bb.jpg").mock(
            return_value=httpx.Response(200, content=JPEG)
        )
        settings = ArtworkSettings(providers=("itunes",))

        result = service.find_artwork(entry(), service.build_providers(settings), settings)

        assert result.ok
        assert result.artwork is not None
        assert result.artwork.mime == "image/jpeg"

    @respx.mock
    def test_a_wrong_artist_result_is_not_downloaded(self):
        respx.get(ITUNES_URL).mock(
            return_value=httpx.Response(
                200,
                json={
                    "results": [
                        {
                            "artistName": "Completely Different",
                            "trackName": "Roki",
                            "artworkUrl100": "https://is1.test/a/100x100bb.jpg",
                        }
                    ]
                },
            )
        )
        image = respx.get("https://is1.test/a/1200x1200bb.jpg")
        settings = ArtworkSettings(providers=("itunes",))

        result = service.find_artwork(entry(), service.build_providers(settings), settings)

        assert not result.ok
        assert not image.called

    @respx.mock
    def test_the_second_provider_is_tried_when_the_first_finds_nothing(self):
        respx.get(ITUNES_URL).mock(return_value=httpx.Response(200, json={"results": []}))
        respx.get(url__startswith="https://musicbrainz.org").mock(
            return_value=httpx.Response(
                200,
                json={
                    "recordings": [
                        {
                            "title": "Roki",
                            "artist-credit": [{"name": "DECO*27"}],
                            "releases": [{"id": "abc-123"}],
                        }
                    ]
                },
            )
        )
        respx.get("https://coverartarchive.org/release/abc-123/front-1200").mock(
            return_value=httpx.Response(200, content=PNG)
        )
        settings = ArtworkSettings(providers=("itunes", "coverartarchive"))

        result = service.find_artwork(entry(), service.build_providers(settings), settings)

        assert result.ok
        assert result.selection.candidate is not None
        assert result.selection.candidate.source == "coverartarchive"

    @respx.mock
    def test_an_unreachable_provider_does_not_abort_the_search(self):
        respx.get(ITUNES_URL).mock(side_effect=httpx.ConnectError("down"))
        respx.get(url__startswith="https://musicbrainz.org").mock(
            return_value=httpx.Response(
                200,
                json={
                    "recordings": [
                        {
                            "title": "Roki",
                            "artist-credit": [{"name": "DECO*27"}],
                            "releases": [{"id": "abc-123"}],
                        }
                    ]
                },
            )
        )
        respx.get("https://coverartarchive.org/release/abc-123/front-1200").mock(
            return_value=httpx.Response(200, content=PNG)
        )
        settings = ArtworkSettings(providers=("itunes", "coverartarchive"))

        result = service.find_artwork(entry(), service.build_providers(settings), settings)

        assert result.ok

    @respx.mock
    def test_a_download_too_small_to_be_a_cover_is_rejected(self):
        respx.get(ITUNES_URL).mock(
            return_value=httpx.Response(
                200,
                json={
                    "results": [
                        {
                            "artistName": "DECO*27",
                            "trackName": "Roki",
                            "artworkUrl100": "https://is1.test/a/100x100bb.jpg",
                        }
                    ]
                },
            )
        )
        respx.get("https://is1.test/a/1200x1200bb.jpg").mock(
            return_value=httpx.Response(200, content=b"\xff\xd8\xfftiny")
        )
        settings = ArtworkSettings(providers=("itunes",))

        result = service.find_artwork(entry(), service.build_providers(settings), settings)

        assert not result.ok
        assert "only" in result.error

    @respx.mock
    def test_a_failed_download_is_reported_not_raised(self):
        respx.get(ITUNES_URL).mock(
            return_value=httpx.Response(
                200,
                json={
                    "results": [
                        {
                            "artistName": "DECO*27",
                            "trackName": "Roki",
                            "artworkUrl100": "https://is1.test/a/100x100bb.jpg",
                        }
                    ]
                },
            )
        )
        respx.get("https://is1.test/a/1200x1200bb.jpg").mock(return_value=httpx.Response(404))
        settings = ArtworkSettings(providers=("itunes",))

        result = service.find_artwork(entry(), service.build_providers(settings), settings)

        assert not result.ok
        assert result.error


class TestDetectMime:
    @pytest.mark.parametrize(
        ("data", "expected"),
        [(JPEG, "image/jpeg"), (PNG, "image/png"), (b"unknown", "image/jpeg")],
    )
    def test_type_comes_from_the_bytes_not_the_headers(self, data, expected):
        assert service.detect_mime(data) == expected


class TestEmbed:
    @respx.mock
    def test_embeds_a_verified_cover_into_a_real_file(self, tmp_path, encode_audio):
        from soulseektools.library import tags
        from soulseektools.library.index import describe

        track = encode_audio(tmp_path / "DECO_27" / "Roki.flac")
        respx.get(ITUNES_URL).mock(
            return_value=httpx.Response(
                200,
                json={
                    "results": [
                        {
                            "artistName": "DECO*27",
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
        settings = ArtworkSettings(providers=("itunes",))

        result = service.find_artwork(
            describe(track, tmp_path), service.build_providers(settings), settings
        )
        service.embed(result)

        embedded = tags.read_artwork(track)
        assert embedded is not None
        assert embedded.data == PNG
        assert embedded.mime == "image/png"

    def test_embedding_nothing_is_a_programming_error(self):
        from soulseektools.artwork.base import ArtSelection

        result = service.ArtworkResult(entry=entry(), selection=ArtSelection(None, "none"))

        with pytest.raises(ValueError, match="nothing to embed"):
            service.embed(result)
