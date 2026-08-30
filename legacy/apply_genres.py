"""
Overwrite genre tags across the whole library with a consistent taxonomy,
assigned per artist folder (this library's genres are consistent within
an artist's catalog).
"""
import os
import sys

from mutagen.flac import FLAC
from mutagen.mp3 import MP3
from mutagen.id3 import TCON
from mutagen.mp4 import MP4

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

MUSIC_DIR = r"E:\Music"

GENRE_MAP = {
    "25時、ナイトコードで。": "Vocaloid",
    "32ki": "Vocaloid",
    "9Lana": "Vocaloid",
    "Aiobahn +81": "Electronic",
    "Leo_need": "Vocaloid",
    "ALICESOFT": "Video Game Soundtrack",
    "Ace": "Eurobeat",
    "Ado": "J-Pop",
    "Chris Stanton": "Eurobeat",
    "D.Essex": "Eurobeat",
    "Dave Mc Loud": "Eurobeat",
    "Dave Rodgers": "Eurobeat",
    "Dave Simon": "Eurobeat",
    "Dejo Bon Bratt Sinclaire": "Eurobeat",
    "Delta Queens": "Eurobeat",
    "Domino": "Eurobeat",
    "Dr. Love": "Eurobeat",
    "Ducky Chix Bratt Sinclaire": "Eurobeat",
    "Elisa": "Eurobeat",
    "Fastway": "Eurobeat",
    "GO2": "Eurobeat",
    "Hotblade": "Eurobeat",
    "JOJAN": "Eurobeat",
    "Jager": "Eurobeat",
    "Jilly": "Eurobeat",
    "Kevin & Cherry": "Eurobeat",
    "Leslie Parrish": "Eurobeat",
    "Lou Grant": "Eurobeat",
    "Manuel": "Eurobeat",
    "Marko Polo": "Eurobeat",
    "Max Coveri": "Eurobeat",
    "Mega Nrg Man": "Eurobeat",
    "Move": "Eurobeat",
    "Mr.Groove": "Eurobeat",
    "Nando Bratt Sinclaire": "Eurobeat",
    "Nathalie": "Eurobeat",
    "Norma Sheffield": "Eurobeat",
    "Nuage": "Eurobeat",
    "Overload": "Eurobeat",
    "Priscilla": "Eurobeat",
    "Sara": "Eurobeat",
    "Speedman": "Eurobeat",
    "The Snake": "Eurobeat",
    "Victoria": "Eurobeat",
    "Wain L": "Eurobeat",
    "Za-Za": "Eurobeat",
    "Edo Boys": "Eurobeat",
    "Symbol": "Eurobeat",
    "Vicky Vale": "Eurobeat",
    "m.o.v.e": "Eurobeat",
    "After the Rain": "Vocaloid",
    "Anamanaguchi": "Electronic",
    "Azari": "Vocaloid",
    "COOL&CREATE": "Touhou",
    "Chinozo": "Vocaloid",
    "Crusher-P": "Vocaloid",
    "DECO_27": "Vocaloid",
    "Duca": "Anime",
    "Eve": "J-Pop",
    "Fahmi": "Vocaloid",
    "Garrett Williamson": "Video Game Soundtrack",
    "Giga": "Vocaloid",
    "HiiragiKirai": "Vocaloid",
    "Hoshimachi Suisei": "VTuber",
    "IOSYS": "Touhou",
    "Inabakumori": "Vocaloid",
    "Iyowa": "Vocaloid",
    "James Landino": "Electronic",
    "Jin": "Vocaloid",
    "KOTOKO": "Anime",
    "Kairiki Bear": "Vocaloid",
    "Kanaria": "Vocaloid",
    "Ken Blast": "Eurobeat",
    "Kikuo": "Vocaloid",
    "Koa": "Vocaloid",
    "Kuroneko": "Vocaloid",
    "Mako & Sayuki": "Eurobeat",
    "Marcin Przybyłowicz": "Video Game Soundtrack",
    "Maretu": "Vocaloid",
    "Mikito P": "Vocaloid",
    "NEEDY GIRL OVERDOSE": "Video Game Soundtrack",
    "Nanahoshi Orchestra": "J-Rock",
    "Nashimoto Ui": "Vocaloid",
    "Nayutalien": "Vocaloid",
    "Neru": "Vocaloid",
    "Niko": "Eurobeat",
    "OMGkawaiiAngel": "Happy Hardcore",
    "PinocchioP": "Vocaloid",
    "Ray": "Vocaloid",
    "Reol": "Vocaloid",
    "Ryokuoushoku Shakai": "J-Rock",
    "S3RL": "Happy Hardcore",
    "S3RL & Sara": "Happy Hardcore",
    "SAWTOWNE": "Vocaloid",
    "SLAVE.V-V-R": "Vocaloid",
    "Sasuke Haraguchi": "Vocaloid",
    "Siinamota": "J-Rock",
    "Suzumu": "Vocaloid",
    "Syudou": "Vocaloid",
    "TAK": "Vocaloid",
    "THREEE": "Vocaloid",
    "TOMATO GUMMY": "Vocaloid",
    "TUYU": "Vocaloid",
    "Wonderful★opportunity!": "Vocaloid",
    "YOASOBI": "J-Pop",
    "Yoshida Yasei": "Vocaloid",
    "Yuikonnu & Ayaponzu-": "Vocaloid",
    "Yuuyu": "Vocaloid",
    "ZUN": "Touhou",
    "aki _3": "Vocaloid",
    "beatMARIO": "Touhou",
    "decaying": "Vocaloid",
    "hachi": "Vocaloid",
    "iroha(sasaki)": "Vocaloid",
    "jon-YAKITORY": "Vocaloid",
    "kemu": "Vocaloid",
    "livetune": "Vocaloid",
    "natori": "Vocaloid",
    "otetsu": "Vocaloid",
    "ryo (supercell)": "Vocaloid",
    "samfree": "Vocaloid",
    "wowaka": "Vocaloid",
    "ぐちり": "Vocaloid",
    "さたぱんP": "Vocaloid",
    "ゆこぴ": "Vocaloid",
    "ショコラ(CV_八木侑紀)": "Anime",
    "ファントムシータ": "Vocaloid",
    "マサラダ": "Vocaloid",
    "上海アリス幻樂団": "Touhou",
    "可不": "Vocaloid",
    "宝鐘マリン": "VTuber",
    "宵崎奏": "Anime",
    "柊キライ": "Vocaloid",
    "柊マグネタイト": "Vocaloid",
    "森羅万象": "Vocaloid",
    "白上フブキ": "VTuber",
    "稲葉曇": "Vocaloid",
    "香椎モイミ": "Vocaloid",
    "Mamemi": ["Touhou", "Eurobeat"],
    "SOUND HOLIC": ["Touhou", "Eurobeat"],
    "Shihori": ["Touhou", "Eurobeat"],
    "Ayako Saso": "Video Game Soundtrack",
    "Omaru Polka": "VTuber",
}


def set_genre(path, genre):
    genres = [genre] if isinstance(genre, str) else list(genre)

    ext = os.path.splitext(path)[1].lower()
    if ext == ".flac":
        audio = FLAC(path)
        audio["genre"] = genres
        audio.save()
    elif ext == ".mp3":
        audio = MP3(path)
        if audio.tags is None:
            audio.add_tags()
        audio.tags.setall("TCON", [TCON(encoding=3, text=genres)])
        audio.save(v2_version=4)
    elif ext in (".m4a", ".mp4"):
        audio = MP4(path)
        audio["\xa9gen"] = genres
        audio.save()
    elif ext == ".ogg":
        from mutagen.oggvorbis import OggVorbis
        audio = OggVorbis(path)
        audio["genre"] = genres
        audio.save()
    else:
        raise ValueError(f"unsupported ext {ext}")


def main():
    tagged = 0
    unmapped = []
    failed = []

    for artist in sorted(os.listdir(MUSIC_DIR)):
        artist_path = os.path.join(MUSIC_DIR, artist)
        if not os.path.isdir(artist_path) or artist.startswith("."):
            continue

        genre = GENRE_MAP.get(artist)
        if genre is None:
            unmapped.append(artist)
            continue

        for fname in os.listdir(artist_path):
            ext = os.path.splitext(fname)[1].lower()
            if ext not in (".flac", ".mp3", ".ogg", ".m4a", ".mp4"):
                continue
            path = os.path.join(artist_path, fname)
            try:
                set_genre(path, genre)
                tagged += 1
            except Exception as e:
                failed.append((path, str(e)))

    print(f"Tagged {tagged} files across {len(GENRE_MAP)} mapped artists")
    if unmapped:
        print(f"\nUnmapped artist folders ({len(unmapped)}):")
        for a in unmapped:
            print(f"  {a}")
    if failed:
        print(f"\nFailed ({len(failed)}):")
        for p, e in failed:
            print(f"  {p}: {e}")


if __name__ == "__main__":
    main()
