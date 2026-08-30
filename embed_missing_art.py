"""
For files missing embedded cover art: look up artwork via the iTunes Search
API (fast, no auth, direct high-res image URLs) and embed it. Prints which
ones it couldn't find, for manual/web-search follow-up.
"""
import json
import os
import sys
import urllib.parse
import urllib.request

from mutagen.flac import FLAC, Picture
from mutagen.mp3 import MP3
from mutagen.id3 import APIC
from mutagen.mp4 import MP4

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

TARGETS = [
    (r"E:\Music\32ki\32ki, Hatsune Miku, 重音テト - メズマライザー (feat. 初音ミク&重音テト).flac", "32ki", "Mesmerizer"),
    (r"E:\Music\ALICESOFT\29 Rebirth the edge.flac", "ALICESOFT", "Rebirth the edge"),
    (r"E:\Music\beatMARIO\15. ビートまりお - ナイト・オブ・ナイツ.flac", "beatMARIO", "Night of Knights"),
    (r"E:\Music\COOL&CREATE\(04) [ビートまりお，myu314] レイセン・オカルティア [東方深秘録 Re：Arrange].flac", "COOL&CREATE", "Reisen Occultia"),
    (r"E:\Music\DECO_27\(06) [DECO-27] 弱虫モンブラン　(sasakure.UK brokensweets Remix).flac", "DECO*27", "Yowamushi Montblanc"),
    (r"E:\Music\Garrett Williamson\01 The Girl Who Glitched (Ai.Mi's Theme from ''Omega Strikers'').flac", "Garrett Williamson", "The Girl Who Glitched"),
    (r"E:\Music\Iyowa\22. Heat Abnormal.flac", "Iyowa", "Heat Abnormal"),
    (r"E:\Music\Mikito P\03 - ロキ.flac", "Mikito P", "Roki"),
    (r"E:\Music\otetsu\01.飴と鎖.flac", "otetsu", "Ame to Kusari"),
    (r"E:\Music\Reol\4 - LUVORATORRRRRY!.flac", "Reol", "LUVORATORRRRRY!"),
    (r"E:\Music\ぐちり\06 Schadenfreude.m4a", "ぐちり", "Schadenfreude"),
    (r"E:\Music\ショコラ(CV_八木侑紀)\01 - Shiny Happy Days [16-44kHz].flac", "Chocolat", "Shiny Happy Days"),
    (r"E:\Music\上海アリス幻樂団\15 平安のエイリアン.flac", "ZUN", "Heian Alien"),
    (r"E:\Music\宵崎奏\01 - アイデンティティ.mp3", "Kanade Yoisaki", "Identity"),
    (r"E:\Music\香椎モイミ\01. キャットラビング.flac", "香椎モイミ", "Cat Loving"),
]


def itunes_lookup(artist, title):
    query = urllib.parse.quote(f"{artist} {title}")
    url = f"https://itunes.apple.com/search?term={query}&media=music&limit=3"
    try:
        with urllib.request.urlopen(url, timeout=10) as resp:
            data = json.loads(resp.read())
    except Exception as e:
        return None, str(e)

    if not data.get("results"):
        return None, "no results"

    result = data["results"][0]
    art_url = result.get("artworkUrl100", "")
    if not art_url:
        return None, "no artwork field"

    hi_res = art_url.replace("100x100bb", "1200x1200bb")
    return hi_res, result.get("trackName", "")


def download_image(url):
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        return resp.read()


def embed(path, image_bytes, mime="image/jpeg"):
    ext = os.path.splitext(path)[1].lower()
    if ext == ".flac":
        audio = FLAC(path)
        pic = Picture()
        pic.type = 3
        pic.mime = mime
        pic.data = image_bytes
        audio.clear_pictures()
        audio.add_picture(pic)
        audio.save()
    elif ext == ".mp3":
        audio = MP3(path)
        if audio.tags is None:
            audio.add_tags()
        audio.tags.add(APIC(encoding=3, mime=mime, type=3, desc="Cover", data=image_bytes))
        audio.save()
    elif ext in (".m4a", ".mp4"):
        from mutagen.mp4 import MP4Cover
        audio = MP4(path)
        fmt = MP4Cover.FORMAT_JPEG if "jpeg" in mime else MP4Cover.FORMAT_PNG
        audio["covr"] = [MP4Cover(image_bytes, imageformat=fmt)]
        audio.save()
    else:
        raise ValueError(f"unsupported ext {ext}")


def main():
    found = []
    not_found = []

    for path, artist, title in TARGETS:
        print(f"{artist} - {title} ...", end=" ")
        art_url, info = itunes_lookup(artist, title)
        if not art_url:
            print(f"NOT FOUND ({info})")
            not_found.append((path, artist, title))
            continue

        try:
            image_bytes = download_image(art_url)
            embed(path, image_bytes)
            print(f"OK -- embedded ({len(image_bytes)} bytes) matched '{info}'")
            found.append(path)
        except Exception as e:
            print(f"FAILED: {e}")
            not_found.append((path, artist, title))

    print(f"\n{len(found)} embedded, {len(not_found)} need manual lookup:")
    for path, artist, title in not_found:
        print(f"  {artist} - {title}")
        print(f"    {path}")


if __name__ == "__main__":
    main()
