import os
import sys
from mutagen.flac import FLAC
from mutagen.mp3 import MP3
from mutagen.mp4 import MP4
from mutagen.oggvorbis import OggVorbis

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
MUSIC_DIR = r"E:\Music"


def get_tags(path):
    ext = os.path.splitext(path)[1].lower()
    try:
        if ext == ".flac":
            f = FLAC(path)
            return f.get("album", [""])[0], f.get("artist", [""])[0], f.get("title", [""])[0]
        if ext == ".mp3":
            f = MP3(path)
            if not f.tags:
                return "", "", ""
            album = str(f.tags.get("TALB", ""))
            artist = str(f.tags.get("TPE1", ""))
            title = str(f.tags.get("TIT2", ""))
            return album, artist, title
        if ext in (".m4a", ".mp4"):
            f = MP4(path)
            return (f.get("\xa9alb", [""])[0], f.get("\xa9ART", [""])[0], f.get("\xa9nam", [""])[0])
        if ext == ".ogg":
            f = OggVorbis(path)
            return f.get("album", [""])[0], f.get("artist", [""])[0], f.get("title", [""])[0]
    except Exception as e:
        return f"ERROR:{e}", "", ""
    return "", "", ""


rows = []
for artist_folder in sorted(os.listdir(MUSIC_DIR)):
    artist_path = os.path.join(MUSIC_DIR, artist_folder)
    if not os.path.isdir(artist_path):
        continue
    for fname in os.listdir(artist_path):
        ext = os.path.splitext(fname)[1].lower()
        if ext not in (".flac", ".mp3", ".ogg", ".m4a", ".mp4"):
            continue
        path = os.path.join(artist_path, fname)
        album, artist_tag, title_tag = get_tags(path)
        rows.append((artist_folder, fname, album, artist_tag, title_tag))

for artist_folder, fname, album, artist_tag, title_tag in rows:
    print(f"{artist_folder} | {fname} | ALBUM={album} | ARTIST={artist_tag} | TITLE={title_tag}")
