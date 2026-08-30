"""
Compare the Spotify CSV export against the existing E:\\Music library and
produce missing.csv containing only tracks that don't appear to be owned yet.

Matching is deliberately conservative: a track is only marked "owned" (and
dropped) when we're fairly confident it's the same song. Anything uncertain
is left in the output list, since a duplicate download is a much smaller
problem than silently skipping a track you don't actually have.

Handles kanji/kana titles by also comparing their romaji transliteration
against romaji library filenames (e.g. CSV "千本桜" vs file "Senbonzakura.flac")
-- plain string similarity can't see those are the same song otherwise.
Artist-name kanji readings are less reliable (aliases often use non-standard
readings), so a very strong title match can pass with a weaker artist match.
"""
import csv
import difflib
import os
import re
import sys
import unicodedata

import pykakasi

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

CSV_PATH = r"C:\Users\Jment\Downloads\My Spotify Library_.csv"
MUSIC_DIR = r"E:\Music"
OUT_PATH = r"C:\Users\Jment\Downloads\missing.csv"

ARTIST_THRESHOLD = 0.6
ARTIST_THRESHOLD_WEAK = 0.35  # allowed when title match is very strong
TITLE_THRESHOLD = 0.62
TITLE_STRONG = 0.85

TRACK_NUM_RE = re.compile(r"^\s*\d+[\.\-]?\d*\s*[-.]?\s*")
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
    text = re.sub(r"\s+", " ", text).strip().lower()
    return text


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
    """Max similarity across original and romaji forms, plus substring
    containment bonus (handles truncated/extended titles)."""
    score = max(ratio(norm_a, norm_b), ratio(romaji_a, norm_b), ratio(norm_a, romaji_b))
    for x, y in ((norm_a, norm_b), (romaji_a, norm_b), (norm_a, romaji_b)):
        if x and y and (x in y or y in x):
            score = max(score, 0.85)
    return score


def load_library(music_dir):
    library = []
    for artist_folder in os.listdir(music_dir):
        artist_path = os.path.join(music_dir, artist_folder)
        if not os.path.isdir(artist_path):
            continue
        for fname in os.listdir(artist_path):
            base, ext = os.path.splitext(fname)
            if ext.lower() not in (".flac", ".mp3", ".m4a", ".wav", ".ogg"):
                continue
            title_guess = strip_artist_prefix(base, artist_folder)
            artist_norm = normalize(artist_folder)
            title_norm = normalize(title_guess)
            library.append({
                "artist_folder": artist_folder,
                "title_guess": title_guess,
                "artist_norm": artist_norm,
                "title_norm": title_norm,
                "artist_romaji": romaji(artist_norm),
                "title_romaji": romaji(title_norm),
                "path": os.path.join(artist_path, fname),
            })
    return library


def best_match(csv_artist, csv_title, library):
    a_norm = normalize(csv_artist)
    t_norm = normalize(csv_title)
    a_romaji = romaji(a_norm)
    t_romaji = romaji(t_norm)

    best = None
    for entry in library:
        a_score = best_ratio(a_norm, a_romaji, entry["artist_norm"], entry["artist_romaji"])
        t_score = best_ratio(t_norm, t_romaji, entry["title_norm"], entry["title_romaji"])

        if t_score >= TITLE_STRONG and a_score >= ARTIST_THRESHOLD_WEAK:
            pass  # strong title match overrides a weak artist match
        elif a_score >= ARTIST_THRESHOLD and t_score >= TITLE_THRESHOLD:
            pass
        else:
            continue

        score = a_score * 0.4 + t_score * 0.6
        if best is None or score > best[0]:
            best = (score, entry["path"])
    return best


def main():
    library = load_library(MUSIC_DIR)
    print(f"Loaded {len(library)} existing tracks from {MUSIC_DIR}")

    with open(CSV_PATH, encoding="utf-8-sig", newline="") as f:
        reader = csv.DictReader(f)
        rows = list(reader)
    print(f"Loaded {len(rows)} tracks from CSV")

    missing = []
    owned = []
    for row in rows:
        title = row["Track name"]
        artist = row["Artist name"]
        match = best_match(artist, title, library)
        if match:
            owned.append((artist, title, match[1], round(match[0], 2)))
        else:
            missing.append(row)

    print(f"\nAlready owned (matched): {len(owned)}")
    for artist, title, path, score in sorted(owned, key=lambda x: x[3]):
        print(f"  [{score}] {artist} - {title}  ->  {path}")

    print(f"\nMissing (to download): {len(missing)}")

    fieldnames = reader.fieldnames
    with open(OUT_PATH, "w", encoding="utf-8-sig", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(missing)

    print(f"\nWrote {len(missing)} tracks to {OUT_PATH}")


if __name__ == "__main__":
    main()
