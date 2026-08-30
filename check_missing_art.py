"""
Scan E:\\Music for audio files with no embedded cover art.
"""
import os
import sys

from mutagen.flac import FLAC
from mutagen.mp3 import MP3
from mutagen.oggvorbis import OggVorbis
from mutagen.mp4 import MP4

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

MUSIC_DIR = r"E:\Music"


def has_art(path):
    ext = os.path.splitext(path)[1].lower()
    try:
        if ext == ".flac":
            return len(FLAC(path).pictures) > 0
        if ext == ".mp3":
            tags = MP3(path).tags
            if tags is None:
                return False
            return any(k.startswith("APIC") for k in tags.keys())
        if ext == ".ogg":
            f = OggVorbis(path)
            return "metadata_block_picture" in f or "coverart" in f
        if ext in (".m4a", ".mp4"):
            f = MP4(path)
            return "covr" in f
    except Exception as e:
        return f"ERROR: {e}"
    return None  # unsupported format, skip


def main():
    missing = []
    unsupported = []
    total = 0

    for artist in os.listdir(MUSIC_DIR):
        artist_path = os.path.join(MUSIC_DIR, artist)
        if not os.path.isdir(artist_path):
            continue
        for fname in os.listdir(artist_path):
            ext = os.path.splitext(fname)[1].lower()
            if ext not in (".flac", ".mp3", ".ogg", ".m4a", ".mp4"):
                continue
            total += 1
            path = os.path.join(artist_path, fname)
            result = has_art(path)
            if result is None:
                unsupported.append(path)
            elif result is False:
                missing.append(path)
            elif isinstance(result, str):
                missing.append(path)
                print(f"  {result}: {path}")

    print(f"\nScanned {total} files")
    print(f"Missing cover art: {len(missing)}")
    for p in missing:
        print(f"  {p}")

    if unsupported:
        print(f"\nUnsupported/skipped: {len(unsupported)}")


if __name__ == "__main__":
    main()
