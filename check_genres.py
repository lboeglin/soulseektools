import os
import sys
from collections import Counter

from mutagen.flac import FLAC
from mutagen.mp3 import MP3
from mutagen.mp4 import MP4

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

MUSIC_DIR = r"E:\Music"


def get_genre(path):
    ext = os.path.splitext(path)[1].lower()
    try:
        if ext == ".flac":
            g = FLAC(path).get("genre")
            return g[0] if g else None
        if ext == ".mp3":
            audio = MP3(path)
            if audio.tags and "TCON" in audio.tags:
                return str(audio.tags["TCON"])
            return None
        if ext in (".m4a", ".mp4"):
            g = MP4(path).get("\xa9gen")
            return g[0] if g else None
    except Exception as e:
        return f"ERROR:{e}"
    return None


counts = Counter()
missing = []
for artist in os.listdir(MUSIC_DIR):
    artist_path = os.path.join(MUSIC_DIR, artist)
    if not os.path.isdir(artist_path):
        continue
    for fname in os.listdir(artist_path):
        ext = os.path.splitext(fname)[1].lower()
        if ext not in (".flac", ".mp3", ".ogg", ".m4a", ".mp4"):
            continue
        path = os.path.join(artist_path, fname)
        genre = get_genre(path)
        if genre:
            counts[genre] += 1
        else:
            missing.append((artist, fname))

print("Existing genre tag values found:")
for genre, count in counts.most_common():
    print(f"  {count:4d}  {genre}")

print(f"\nFiles with NO genre tag: {len(missing)}")
