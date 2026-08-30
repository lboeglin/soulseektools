"""
Scan the whole E:\\Music library for duplicate tracks, including cases where
the same song exists under totally different filenames/artist-folder
spellings (e.g. a pre-existing "Inabakumori" folder vs a newly-downloaded
"稲葉曇" folder for the same producer). Compares every file against every
other file using the same romaji-aware fuzzy matching used for the CSV
dedup -- plain filename grouping misses these cross-naming duplicates.
"""
import csv
import difflib
import os
import re
import sys
import unicodedata

import pykakasi

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

MUSIC_DIR = r"E:\Music"
AUDIO_EXTS = (".flac", ".mp3", ".m4a", ".wav", ".ogg")

ARTIST_THRESHOLD = 0.55
ARTIST_THRESHOLD_WEAK = 0.3
TITLE_THRESHOLD = 0.62
TITLE_STRONG = 0.85

TRACK_NUM_RE = re.compile(r"^\s*\(?\d+[\.\-\)]?\d*\s*[-.\)]?\s*")
FEAT_RE = re.compile(r"\(feat\.[^)]*\)|feat\.[^,(\[]*", re.IGNORECASE)
PAREN_RE = re.compile(r"[\(\[][^\)\]]*[\)\]]")
PUNCT_RE = re.compile(r"[^\w\s]", re.UNICODE)

_kks = pykakasi.kakasi()
_romaji_cache = {}


def normalize(text):
    text = unicodedata.normalize("NFKC", text)
    text = FEAT_RE.sub(" ", text)
    text = PAREN_RE.sub(" ", text)
    text = PUNCT_RE.sub(" ", text)
    return re.sub(r"\s+", " ", text).strip().lower()


def romaji(normalized_text):
    if normalized_text not in _romaji_cache:
        result = _kks.convert(normalized_text)
        _romaji_cache[normalized_text] = normalize("".join(r["hepburn"] for r in result))
    return _romaji_cache[normalized_text]


def strip_artist_prefix(filename_no_ext, artist_folder):
    name = TRACK_NUM_RE.sub("", filename_no_ext)
    artist_norm = normalize(artist_folder)
    name_norm = normalize(name)
    if name_norm.startswith(artist_norm + " "):
        name = name[len(artist_folder):].lstrip(" -")
    return name


def ratio(a, b):
    if not a or not b:
        return 0.0
    return difflib.SequenceMatcher(None, a, b).ratio()


def best_ratio(norm_a, romaji_a, norm_b, romaji_b):
    score = max(ratio(norm_a, norm_b), ratio(romaji_a, norm_b), ratio(norm_a, romaji_b))
    for x, y in ((norm_a, norm_b), (romaji_a, norm_b), (norm_a, romaji_b)):
        if x and y and (x in y or y in x):
            score = max(score, 0.85)
    return score


def load_library():
    entries = []
    for artist_folder in os.listdir(MUSIC_DIR):
        artist_path = os.path.join(MUSIC_DIR, artist_folder)
        if not os.path.isdir(artist_path):
            continue
        for fname in os.listdir(artist_path):
            base, ext = os.path.splitext(fname)
            if ext.lower() not in AUDIO_EXTS:
                continue
            title_guess = strip_artist_prefix(base, artist_folder)
            artist_norm = normalize(artist_folder)
            title_norm = normalize(title_guess)
            entries.append({
                "artist_folder": artist_folder,
                "path": os.path.join(artist_path, fname),
                "artist_norm": artist_norm,
                "title_norm": title_norm,
                "artist_romaji": romaji(artist_norm),
                "title_romaji": romaji(title_norm),
            })
    return entries


def main():
    entries = load_library()
    print(f"Loaded {len(entries)} files\n")

    seen_pairs = set()
    matches = []

    for i in range(len(entries)):
        for j in range(i + 1, len(entries)):
            a, b = entries[i], entries[j]
            if a["path"] == b["path"]:
                continue

            a_score = best_ratio(a["artist_norm"], a["artist_romaji"], b["artist_norm"], b["artist_romaji"])
            t_score = best_ratio(a["title_norm"], a["title_romaji"], b["title_norm"], b["title_romaji"])

            if t_score >= TITLE_STRONG and a_score >= ARTIST_THRESHOLD_WEAK:
                pass
            elif a_score >= ARTIST_THRESHOLD and t_score >= TITLE_THRESHOLD:
                pass
            else:
                continue

            score = a_score * 0.4 + t_score * 0.6
            matches.append((score, a["path"], b["path"]))

    matches.sort(key=lambda m: -m[0])
    print(f"Found {len(matches)} likely duplicate pair(s):\n")
    for score, p1, p2 in matches:
        size1 = os.path.getsize(p1) / 1_000_000
        size2 = os.path.getsize(p2) / 1_000_000
        print(f"[{score:.2f}]")
        print(f"    {size1:6.1f} MB  {p1}")
        print(f"    {size2:6.1f} MB  {p2}")
        print()


if __name__ == "__main__":
    main()
