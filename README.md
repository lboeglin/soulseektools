# SoulseekTools

Scripts + a Nicotine+ plugin for building out a music library from a Spotify
export via Soulseek, and keeping that library clean afterward (no
duplicates, full cover art, consistent genre tags, no fake/transcoded FLACs).

Built ad hoc during one long session — paths are currently hardcoded to
that session's setup (`E:\Music` as the library, `C:\Users\Jment\Downloads\`
for the CSVs). Treat this as a working prototype, not a polished tool: if
you pick this back up, the first job is pulling those paths into a config
file or CLI args instead of editing the scripts by hand.

## The workflow, end to end

1. **`dedup_library.py`** — compares a Spotify library CSV export against
   what's already in `E:\Music` and writes `missing.csv` (tracks you don't
   have yet). Matching is romaji/kanji-aware (via `pykakasi`) since Spotify
   titles are often in kanji while ripped files are named in romaji — plain
   string matching alone misses those. Deliberately conservative: an
   uncertain match stays in "missing" rather than getting silently dropped,
   since a duplicate download is cheaper than losing a track.

2. **`nicotine-plugin/soulseek_fetch/`** — the actual download engine. A
   Nicotine+ plugin (not a standalone script — Nicotine+ has to be running
   for it to do anything). Copy `soulseek_fetch/` into your Nicotine+
   plugins folder (Windows: `%APPDATA%\nicotine\plugins\`) and enable it in
   Preferences → Plugins.
   - Reads `missing.csv`, searches Soulseek for each track, ranks results
     (prefers FLAC, then bitrate, then uploader queue/free-slot), and
     writes the top 3 candidates per track to `soulseek_review.csv` —
     **nothing downloads automatically**.
   - You (or an approve script) fill in the `approve` column with `1`/`2`/`3`
     to pick a candidate. The plugin polls that file every 30s while
     running and queues approved downloads.
   - On each finished download it runs `check_fake_flac.py` automatically
     and logs the verdict.
   - See "Nicotine+ quirks" below before relying on this for a large batch.

3. **`check_fake_flac.py`** — spectral analysis to catch FLACs that are
   actually a transcoded lossy source (MP3/AAC) re-encoded as FLAC. Looks
   for a *hard, steep* frequency cutoff (the signature of a lossy encoder)
   vs. a *gradual* roll-off (normal mastering) — plain "is there content
   above 16kHz" is not enough, loud modern masters roll off early too and
   will false-positive. Validated against a real synthetic transcode during
   development. Usage: `--file <path>` for one file (prints JSON, used by
   the plugin), or `<folder>` to scan recursively with `--report out.csv`
   and optional `--spectrograms` for visual confirmation on flagged files.

4. **Library maintenance**, run periodically or after a batch of downloads:
   - **`find_duplicates.py`** — fuzzy title+romaji matching across the
     *whole* library (not just within a folder), so it catches the same
     song filed under two different artist-folder spellings (e.g.
     `Inabakumori` vs `稲葉曇`), not just exact re-downloads.
   - **`find_duplicates_by_content.py`** — a second, independent check:
     groups files by duration + file size fingerprint to catch duplicates
     that don't even share a similar title. Verify any hit by actually
     diffing decoded audio (see `check_art_quality.py`-style spot checks in
     session history) before trusting it — coincidental length/size matches
     between unrelated songs do happen.
   - **`check_missing_art.py`** / **`embed_missing_art.py`** — find files
     with no embedded cover, look it up (iTunes Search API first, falls
     back to manual VocaDB/YouTube-thumbnail lookups for stuff iTunes
     doesn't carry), embed it. **Don't trust the first API result blindly**
     — iTunes search match quality is inconsistent; verify the returned
     `artistName` actually matches before embedding, or you'll paste the
     wrong artist's art onto the file (happened twice in testing).
   - **`check_art_quality.py`** — flags embedded art under ~3KB, i.e.
     present but broken/placeholder.
   - **`check_genres.py`** / **`apply_genres.py`** — `apply_genres.py`
     hardcodes an artist-folder → genre map (Vocaloid / J-Pop / Touhou /
     Happy Hardcore / etc.) built for *this specific library's* artist
     roster. Re-tag `GENRE_MAP` for a different library rather than reusing
     it as-is.
   - **`audit_all_tags.py`** — dumps artist/album/title for every file.
     Mainly useful for spotting a whole *class* of mismatch at once — e.g.
     grepping the output for "Symphony"/"Orchestra"/"Live" surfaced eight
     tracks that were orchestral concert covers instead of studio originals,
     all sharing the root cause (the ranker only scores the filename, not
     the album/folder context, so it can't tell a concert recording from a
     studio one when both have the right title).
   - **`audit_art_matches.py`** — spot-check helper for verifying a batch of
     iTunes art matches actually have the right `artistName` before trusting
     them.

## Nicotine+ quirks worth knowing before relying on this again

- **Nicotine+ persists its download queue across restarts** (in
  `downloads.json`). Killing the process does *not* cancel an in-flight or
  queued transfer the way you'd expect — on the next launch it resumes
  everything that wasn't marked "Finished," including stale duplicate
  requests from an earlier candidate choice. This caused repeated duplicate
  downloads during development. If you restart after switching a
  candidate, check `downloads.json` for old non-Finished entries pointing
  at the same destination file and remove them, or you'll get both.
- **Log output written via `nohup ... > file.log` lags behind actual
  activity** by anywhere from seconds to a couple minutes (stdout
  buffering). Check the real file state on disk, not just the log tail, if
  something looks stalled.
- **Files can get transiently locked** ("Permission denied") while
  Nicotine+ is actively uploading them to another Soulseek user — this
  library is shared, so that's expected. Don't force anything; retry later.
- **The Soulseek search exclusion syntax (`-word`) is unreliable** for
  filtering out unwanted results (tried to exclude "remix"/"live"/etc. from
  results — consistently returned zero matches rather than filtering).
  Better to search plainly and review/reject candidates manually.
- **No Soulseek privileges** (this account has none) means downloads queue
  behind privileged users on each peer — wait time per file is unpredictable
  and can't be forced. Switching to a different uploader (candidate 2/3) is
  usually faster than waiting out a stalled candidate 1.

## Known limitation in the ranking logic

The candidate scorer in the Nicotine+ plugin only ever looks at a result's
*filename*, never the remote folder path it lives in. That's the root cause
behind most of the wrong-match bugs found during use: a file named exactly
right can still be a live concert recording, a different artist's remix, or
literally a different song's medley track, and none of that is visible from
the filename alone. If you extend this, feeding the full remote path into
the scoring/red-flag check (folder names often say "Live", "Symphony",
"Orchestra", a different circle/label name, etc.) would catch a lot of this
before download instead of requiring a manual re-listen afterward.
