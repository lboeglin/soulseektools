import json
import sys
import urllib.parse
import urllib.request

from mutagen.flac import FLAC, Picture

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

FIXES = [
    (r"E:\Music\beatMARIO\15. ビートまりお - ナイト・オブ・ナイツ.flac", "beatMARIO Night of Knights", "beatmario"),
    (r"E:\Music\ぐちり\06 Schadenfreude.m4a", "Guchiry Schadenfreude", "guchiry"),
]


def find_art(query, expected_artist_lower):
    q = urllib.parse.quote(query)
    url = f"https://itunes.apple.com/search?term={q}&media=music&limit=5"
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read())
    for r in data.get("results", []):
        if r.get("artistName", "").lower() == expected_artist_lower:
            return r["artworkUrl100"].replace("100x100bb", "1200x1200bb"), r.get("trackName")
    return None, None


def download(url):
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        return resp.read()


for path, query, expected_artist in FIXES:
    art_url, track_name = find_art(query, expected_artist)
    if not art_url:
        print(f"STILL NOT FOUND: {path}")
        continue
    image_bytes = download(art_url)
    ext = path.rsplit(".", 1)[-1].lower()
    if ext == "flac":
        audio = FLAC(path)
        pic = Picture()
        pic.type = 3
        pic.mime = "image/jpeg"
        pic.data = image_bytes
        audio.clear_pictures()
        audio.add_picture(pic)
        audio.save()
    elif ext == "m4a":
        from mutagen.mp4 import MP4, MP4Cover
        audio = MP4(path)
        audio["covr"] = [MP4Cover(image_bytes, imageformat=MP4Cover.FORMAT_JPEG)]
        audio.save()
    print(f"FIXED: {path} -> matched '{track_name}' ({len(image_bytes)} bytes)")
