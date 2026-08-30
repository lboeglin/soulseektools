import pytest

from soulseektools.errors import UnsupportedFormatError
from soulseektools.library import tags
from soulseektools.models import Artwork

TAGGABLE = [".flac", ".mp3", ".m4a", ".ogg", ".opus"]

PNG_PIXEL = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4"
    "890000000a49444154789c6360000002000100ffff03000006000557bfabd400"
    "00000049454e44ae426082"
)


@pytest.fixture
def artwork() -> Artwork:
    return Artwork(data=PNG_PIXEL * 40, mime="image/png")


@pytest.mark.parametrize("suffix", TAGGABLE)
def test_read_tags_returns_the_same_fields_for_every_container(suffix, tmp_path, encode_audio):
    path = encode_audio(
        tmp_path / f"track{suffix}",
        metadata={
            "artist": "DECO*27",
            "album": "Ghost",
            "title": "Roki",
            "genre": "Vocaloid",
            "track": "3",
        },
    )

    read = tags.read_tags(path)

    assert read.artist == "DECO*27"
    assert read.album == "Ghost"
    assert read.title == "Roki"
    assert read.genres == ("Vocaloid",)
    assert read.track_number == 3


@pytest.mark.parametrize("suffix", [*TAGGABLE, ".wav"])
def test_read_tags_treats_an_untagged_file_as_empty_rather_than_an_error(
    suffix, tmp_path, encode_audio
):
    path = encode_audio(tmp_path / f"bare{suffix}")

    assert tags.read_tags(path).artist == ""


@pytest.mark.parametrize("suffix", [*TAGGABLE, ".wav"])
def test_read_duration_comes_from_the_container_header(suffix, tmp_path, encode_audio):
    path = encode_audio(tmp_path / f"track{suffix}", seconds=2.0)

    duration = tags.read_duration(path)

    assert duration is not None
    assert duration == pytest.approx(2.0, abs=0.15)


@pytest.mark.parametrize("suffix", TAGGABLE)
def test_write_genres_round_trips(suffix, tmp_path, encode_audio):
    path = encode_audio(tmp_path / f"track{suffix}")

    tags.write_genres(path, ["Touhou", "Eurobeat"])

    assert tags.read_tags(path).genres == ("Touhou", "Eurobeat")


@pytest.mark.parametrize("suffix", TAGGABLE)
def test_write_genres_replaces_rather_than_appends(suffix, tmp_path, encode_audio):
    path = encode_audio(tmp_path / f"track{suffix}", metadata={"genre": "Pop"})

    tags.write_genres(path, ["Vocaloid"])

    assert tags.read_tags(path).genres == ("Vocaloid",)


@pytest.mark.parametrize("suffix", TAGGABLE)
def test_artwork_round_trips(suffix, tmp_path, encode_audio, artwork):
    path = encode_audio(tmp_path / f"track{suffix}")
    assert tags.read_artwork(path) is None

    tags.write_artwork(path, artwork)

    embedded = tags.read_artwork(path)
    assert embedded is not None
    assert embedded.data == artwork.data
    assert embedded.mime == artwork.mime


@pytest.mark.parametrize("suffix", TAGGABLE)
def test_write_artwork_replaces_the_existing_cover(suffix, tmp_path, encode_audio, artwork):
    path = encode_audio(tmp_path / f"track{suffix}")
    tags.write_artwork(path, artwork)

    replacement = Artwork(data=PNG_PIXEL * 80, mime="image/png")
    tags.write_artwork(path, replacement)

    embedded = tags.read_artwork(path)
    assert embedded is not None
    assert len(embedded) == len(replacement)


@pytest.mark.parametrize("suffix", TAGGABLE)
def test_artwork_size_reports_zero_when_there_is_no_cover(suffix, tmp_path, encode_audio):
    path = encode_audio(tmp_path / f"track{suffix}")

    assert tags.artwork_size(path) == 0


@pytest.mark.parametrize("suffix", TAGGABLE)
def test_artwork_size_matches_the_embedded_bytes(suffix, tmp_path, encode_audio, artwork):
    path = encode_audio(tmp_path / f"track{suffix}")
    tags.write_artwork(path, artwork)

    assert tags.artwork_size(path) == len(artwork)


def test_artwork_size_is_zero_for_a_container_that_cannot_hold_a_cover(tmp_path, encode_audio):
    # The original scripts raised or crashed here; callers only ever want a number.
    path = encode_audio(tmp_path / "track.wav")

    assert tags.artwork_size(path) == 0


def test_reading_artwork_from_an_unsupported_container_is_explicit(tmp_path, encode_audio):
    path = encode_audio(tmp_path / "track.wav")

    with pytest.raises(UnsupportedFormatError):
        tags.read_artwork(path)


def test_reading_a_non_audio_file_is_explicit(tmp_path):
    path = tmp_path / "notes.txt"
    path.write_text("not audio", encoding="utf-8")

    with pytest.raises(UnsupportedFormatError):
        tags.read_tags(path)


def test_every_taggable_extension_is_covered_by_these_tests():
    assert set(TAGGABLE) == set(tags.TAGGABLE_EXTENSIONS) - {".mp4"}
