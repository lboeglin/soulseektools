# Oracle — reference output from the Python implementation

Captured to give the Rust port (#1) something to diff against. Every file here
is output from the Python `stk` at commit `b7f2bac`, run against a real library.

**These files are the specification.** If the Rust implementation disagrees with
them, the Rust implementation is wrong until proven otherwise.

## Capture conditions

| | |
| --- | --- |
| Library | `E:\Music` — 266 audio files, 144 artist folders |
| Formats | 257 flac, 7 mp3, 1 ogg, 1 m4a |
| Character of it | Japanese-heavy: Vocaloid, Touhou, doujin, Eurobeat |
| ffmpeg / ffprobe | 8.1.2 |
| fpcalc | 1.6.1 — installed for the `dupes-content` capture, which came later |
| slskd | not running (not needed; no command here touches the network) |
| Platform | Windows 11 |

All commands were run with `PYTHONIOENCODING=utf-8` set, to work around #4.
Without it, every command below crashes when redirected.

## Files

| File | Command | Notes |
| --- | --- | --- |
| `stats.json` | `stk --json library stats` | 266 total, 265 tagged, 266 with artwork, 266 with genre |
| `verify.csv` | `stk verify --report oracle/verify.csv` | 258 lossless files: 248 OK, 9 FAKE, 1 SUSPECT |
| `verify.stdout.txt` | — | the run's console output; ends in the #4 crash, after the CSV was written |
| `dupes-titles.json` | `stk --json dupes titles` | |
| `dupes-content.json` | `stk --json dupes content` | 3 pairs, over chromaprint fingerprints |
| `art.json` | `stk --json art check` | empty — every file already has usable artwork |
| `genre.json` | `stk --json genre check` | |
| `tags.csv` | `stk tags dump` | 266 rows: path, folder_artist, artist, album, title, genres, duration |
| `romaji.csv` | see below | 434 distinct name forms: raw, normalized, romaji |

### `romaji.csv`

Not produced by a CLI command — dumped directly from the index, because it is
the reference for the single highest-risk part of the port (`text/romaji.py`,
which used pykakasi):

```python
from soulseektools.config import load_settings
from soulseektools.library.index import LibraryIndex

s = load_settings(None)
with LibraryIndex(s.index.path) as ix:
    for e in ix.entries():
        for kind, forms in (("artist", e.artist), ("title", e.title)):
            ...  # kind, forms.raw, forms.normalized, forms.romaji -- deduplicated
```

Diff **both** the `normalized` and `romaji` columns. The normalised column is
the one at risk from the `\w` difference between Python's `re` and Rust's
`regex` (see #1, "Porting gotchas") — and since every comparison in the project
flows through `normalize()`, a small divergence there shifts matching scores
globally.

## Caveats

**`dupes-content.json` pins chromaprint, not the fingerprints.** It was captured
after Chromaprint was installed, so the pairs it records came from `fpcalc`
rather than from the loudness-envelope fallback the Python used without it. The
Rust build computes chromaprint in-process through `rusty-chromaprint`, which
agrees with `fpcalc` on about 98.5% of a fingerprint's bits -- far above the 0.9
that decides a duplicate, and far below the 1.0 two reads of one file should
score. So what the gate compares is the set of pairs a library-wide run finds,
which has to hold, and not the fingerprints, which cannot.

**One `verify` row cannot be reproduced.** `TAK - PPPP - 01 PPPP.flac` is 32-bit
FLAC; symphonia accepts those and then decorrelates mid/side through an `i32`
too narrow to hold `2*mid + side`, so the Rust build refuses the file rather
than believe the noise it decodes. The oracle's verdict for it stands, and
nothing in the Rust build can check it. The other 257 rows match exactly.

**Five `verify` rows are judged differently on purpose.** The Python called
them OK; since #16 they are `UPSAMPLED`.

- `hachi\01 ドーナツホール.flac` and `Neru\01. ロストワンの号哭.flac` state 24
  bits and use 16: the low 8 bits of every sample are zero, which ffmpeg's
  `astats` confirms (`Bit depth: 16/16`).
- `Azari\01 - D+N+A.flac`, `Eve\02. Nonsense Bungaku.flac` and
  `Iyowa\22. Heat Abnormal.flac` are 96 kHz files whose content ends in a steep
  edge between 21 and 23.5 kHz with next to nothing above it: -164 dB, digital
  silence, in the first; a flat -109 dB dither floor in the last. The other
  96 kHz files carry their converter's noise out to 48 kHz.

The captured rows are left as the Python wrote them, and the gates in
`oracle_verify.rs` and `oracle_cli.rs` name these five and expect the new
verdict and detail, with every other field unchanged.

**17 `romaji.csv` rows are corrected on purpose.** Romaji is read with IPADIC
since #21, not with a kakasi, and where the two disagreed each row was checked
by hand. The `romaji` cell was rewritten where the new reading is the right
one, and nowhere else; a reading that came out wrong was fixed in
`KNOWN_READINGS` instead. The `raw` and `normalized` cells are untouched.

| Reading | Was | Now |
| --- | --- | --- |
| 25時 (2 rows) | `25toki` | `25ji` |
| 重音テト (3 rows) | `juuonteto` | `kasaneteto` |
| 稲葉曇 (2 rows) | `inabadon` | `inabakumori` |
| 藤咲かりん | `fujisakarin` | `fujisakikarin` |
| 優曇華院 | `udonhanain` | `udongein` |
| 山本 椛 | `kaba` | `momiji` |
| 鳴花ミコト | `meihanamikoto` | `meikamikoto` |
| 君はできない子 | `kunhadekinaiko` | `kimihadekinaiko` |
| 愛して 愛して 愛して | `itoshite` | `aishite` |
| 荒療治 | `araryouchi` | `araryouji` |
| 雨路少女 | `amemichishoujo` | `amejishoujo` |
| 宵崎奏 | `shousakisou` | `yoisakikanade` |
| 白上フブキ | `shirouefubuki` | `shirakamifubuki` |

**`verify.csv` has been reviewed, but not exhaustively.** The 9 FAKE verdicts
are specific and physically plausible — see the summary below — but the
classifier had never seen a real file before this run. Treat the verdicts as a
strong reference, not as ground truth, and re-check any file where the Rust
implementation disagrees rather than assuming Rust is at fault.

**Compilations are not represented.** No `Various Artists`-style folder exists in
this library, so nothing here exercises the failure described in #2 section 6.
That question is closed for *this* library, not in general.

## The `verify` verdicts

| Verdict | Count |
| --- | --- |
| OK | 248 |
| FAKE | 9 |
| SUSPECT | 1 |

The 9 FAKE files split into two clean groups:

- **Lossy transcodes** — brickwall cutoffs at 15300, 16350, 16800, 19950 and
  20400 Hz, at 28–68 dB/kHz. The 16.8 kHz file at 68 dB/kHz is about as
  unambiguous as this test gets.
- **Fake hi-res** — four 48 kHz containers whose content stops sharply at
  19800 Hz. That they all land on the *same* frequency is corroborating rather
  than suspicious: it points at one common upstream encoder.

The single SUSPECT is a 14850 Hz cutoff at 64 dB/kHz — very sharp, at no
standard bitrate. That is the verdict behaving correctly: sharp enough to
distrust, not matching a known encoder, so it asks for a human instead of
asserting.

The borderline case worth a manual look is the 20400 Hz / 28 dB/kHz file. It sits
just past the 25 dB/kHz steepness threshold and at the very top of the
256–320 kbps band, so it is the verdict most sensitive to
`verify.steep_db_per_khz`.

## Reproducing

```bash
export PYTHONIOENCODING=utf-8        # until #4 is fixed
stk library scan
stk --json library stats  > oracle/stats.json
stk verify --report oracle/verify.csv
stk --json dupes titles   > oracle/dupes-titles.json
stk --json dupes content  > oracle/dupes-content.json
stk --json art check      > oracle/art.json
stk --json genre check    > oracle/genre.json
stk tags dump             > oracle/tags.csv
```
