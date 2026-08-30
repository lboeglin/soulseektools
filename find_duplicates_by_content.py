"""
Catch duplicate audio content that title-matching would miss: group files by
duration (rounded) + file size (rounded to nearest 0.5MB) as a fingerprint.
Two files with near-identical duration AND near-identical size are almost
certainly the same recording, regardless of what they're named/tagged.
"""
import os
import subprocess
import sys
from collections import defaultdict

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
MUSIC_DIR = r"E:\Music"
AUDIO_EXTS = (".flac", ".mp3", ".m4a", ".wav", ".ogg")


def get_duration(path):
    try:
        out = subprocess.run(
            ["ffprobe", "-v", "quiet", "-show_entries", "format=duration",
             "-of", "csv=p=0", path],
            capture_output=True, text=True, timeout=15,
        )
        return float(out.stdout.strip())
    except Exception:
        return None


def main():
    files = []
    for artist in os.listdir(MUSIC_DIR):
        artist_path = os.path.join(MUSIC_DIR, artist)
        if not os.path.isdir(artist_path):
            continue
        for fname in os.listdir(artist_path):
            if os.path.splitext(fname)[1].lower() not in AUDIO_EXTS:
                continue
            files.append(os.path.join(artist_path, fname))

    print(f"Checking {len(files)} files for content-duplicate fingerprint...")
    groups = defaultdict(list)
    for i, path in enumerate(files, 1):
        dur = get_duration(path)
        if dur is None:
            continue
        size_mb = os.path.getsize(path) / 1_000_000
        # bucket duration to nearest 0.5s, size to nearest 0.3MB to allow for minor encoding variance
        key = (round(dur * 2) / 2, round(size_mb / 0.3))
        groups[key].append((path, dur, size_mb))

    dupes = {k: v for k, v in groups.items() if len(v) > 1}
    print(f"\nFound {len(dupes)} content-fingerprint match group(s):\n")
    for key, items in dupes.items():
        print(f"duration~{key[0]}s:")
        for path, dur, size_mb in items:
            print(f"    {dur:7.2f}s  {size_mb:6.2f}MB  {path}")
        print()


if __name__ == "__main__":
    main()
