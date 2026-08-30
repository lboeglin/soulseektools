# Changelog

Notable changes to this project. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org/).

## [Unreleased]

The project became an installable package with a single `stk` command, replacing
a set of ad-hoc scripts and a Nicotine+ plugin.

### Added

- `stk` CLI covering the whole workflow: `config`, `doctor`, `library`,
  `wantlist`, `fetch`, `review`, `verify`, `dupes`, `art`, `genre`, `tags`.
- Terminal review app (`stk review`), replacing hand-editing an `approve` column
  in a CSV. Shows each candidate's full remote path and its warnings.
- Layered configuration — CLI flag, `STK_` environment variable, `stk.toml`,
  per-user config, defaults — replacing paths hardcoded into every script. The
  slskd API key is read from the environment only and never written to a file.
- slskd backend, replacing the Nicotine+ plugin. Every endpoint and response
  shape was pinned against a running container's OpenAPI document.
- SQLite library index, keyed by path and invalidated by size and mtime, so
  commands stop rescanning the whole library. Durations come from container
  headers instead of an ffprobe subprocess per file.
- SQLite review queue with an explicit state per track, plus `stk review import`
  for an existing `soulseek_review.csv`.
- Acoustic fingerprinting for content duplicates: Chromaprint when `fpcalc` is
  installed, otherwise a loudness-envelope curve needing only ffmpeg.
- MusicBrainz and Cover Art Archive as a second artwork provider.
- `--apply` gating on every command that writes to audio files.
- Test suite of ~500 tests with no network access, run on Linux and Windows
  across Python 3.11–3.13.

### Changed

- **Search results are scored on their full remote path**, not just the
  filename. The original README named filename-only scoring as the root cause of
  most wrong downloads: a file named exactly right can still be a live recording
  or an orchestral cover, and only its folder says so.
- **Cover art is rejected unless the artist matches.** Embedding the first search
  result unchecked put the wrong artist's cover on two files during the original
  session.
- **Content duplicates are decided by audio, not by duration and size.** Duration
  is now only a prefilter; matching on duration and size alone produced
  coincidental matches between unrelated tracks of similar length.
- The duplicate scan compares only entries sharing an n-gram instead of every
  pair against every other.
- The library scan is recursive; the previous two-level walk silently skipped
  anything filed under `Artist/Album/track.flac`.
- The 143-artist genre map moved from Python source into an editable TOML file.
- Container handling was unified into one module. Six divergent copies of
  mutagen dispatch disagreed about which formats they supported and about
  whether a missing ID3 header was an error or an empty tag.
- rapidfuzz replaced difflib for name matching.

### Fixed

- A silent or inaudible file normalised to its own peak read as 0 dB in every
  band and was confidently reported as genuinely lossless. It is now reported as
  unanalysable, as is any file whose content stops below every encoder cutoff
  and so carries no evidence either way.
- The known-bitrate cutoff bands were narrow enough that measured LAME stereo
  brickwalls at 16650 Hz and 18750 Hz fell between them, downgrading a real
  transcode to `SUSPECT`. The bands are now contiguous and derived from
  measurement.
- `normalize()` did not fold underscore, so `DECO_27` and `DECO*27` did not
  compare equal.
- Stripping an artist prefix sliced by the folder name's length, cutting titles
  short whenever the two spellings differed in length (`ryo (supercell) - Melt`).
- Name matching never compared romaji against romaji, so two kanji spellings
  sharing a reading did not match.
- Bracketed suffixes were stripped before scanning a path for warning terms, so
  `Roki (Live).flac` lost the marker that identified it.
- A `Completed, Errored` transfer state is read as a failure rather than a
  completion.
- Long paths in command output are folded rather than truncated, so the paths
  the user acts on can be copied.
