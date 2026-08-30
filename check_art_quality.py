import os
import sys
from mutagen.flac import FLAC
from mutagen.mp3 import MP3
from mutagen.mp4 import MP4

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
MUSIC_DIR = r"E:\Music"
MIN_BYTES = 3000  # anything smaller is almost certainly a broken/placeholder image


def get_art_size(path):
    ext = os.path.splitext(path)[1].lower()
    try:
        if ext == ".flac":
            f = FLAC(path)
            return len(f.pictures[0].data) if f.pictures else 0
        if ext == ".mp3":
            f = MP3(path)
            if not f.tags:
                return 0
            for tag in f.tags.values():
                if tag.FrameID.startswith("APIC"):
                    return len(tag.data)
            return 0
        if ext in (".m4a", ".mp4"):
            f = MP4(path)
            covr = f.get("covr")
            return len(bytes(covr[0])) if covr else 0
    except Exception:
        return -1
    return -1


small = []
for artist in os.listdir(MUSIC_DIR):
    artist_path = os.path.join(MUSIC_DIR, artist)
    if not os.path.isdir(artist_path):
        continue
    for fname in os.listdir(artist_path):
        ext = os.path.splitext(fname)[1].lower()
        if ext not in (".flac", ".mp3", ".m4a", ".mp4"):
            continue
        path = os.path.join(artist_path, fname)
        size = get_art_size(path)
        if size < MIN_BYTES:
            small.append((path, size))

print(f"Suspiciously small/broken art: {len(small)}")
for path, size in small:
    print(f"  {size} bytes: {path}")
