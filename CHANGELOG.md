# Changelog

Notable changes to this project. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org/).

## [Unreleased]

The project became an installable package with a single `stk` command, replacing
a set of ad-hoc scripts and a Nicotine+ plugin, and was then rewritten in Rust.

### Added

- `stk tags apply`, the inverse of `tags dump`: edit the dump in a spreadsheet
  and write it back. Artist, album artist, album, title and track number could
  not be written at all before, so the dump could only show what was wrong.
  `tags dump --round-trip` adds the two columns a row needs to be written back
  exactly; the plain dump keeps the columns it always had. A column the sheet
  does not carry is never written, and an empty cell leaves the tag alone unless
  `--clear-blanks` says otherwise -- a half-filled sheet is a commoner cause of
  an empty cell than an intent to erase. Each field is logged separately, so an
  undo never writes wider than the row it is undoing (#3).
- `stk organise`, which moves files into the layout their tags describe --
  `{artist}/{album}/{track} - {title}` by default, and configurable. A
  placeholder with nothing to fill it takes the punctuation next to it away, so
  a file with no track number is `Title.flac` and one with no album loses the
  directory rather than gaining an `Unknown Album`. Nothing is overwritten and
  nothing is auto-suffixed: an occupied destination and two files landing on one
  name are both found while planning. The index follows each file, carrying its
  fingerprint across, and emptied folders are removed behind the move unless
  they still hold something. `--from` tidies the flat layout a fetch leaves in
  the download directory (#3).
- `stk playlist export|genre|artist`, writing M3U8. A library is built from an
  ordered export and the order then survived nowhere; `export` writes back the
  tracks the library holds in the export's own row order (#3).
- `stk library trend`. Every `library scan` now records a measurement -- files,
  tagged, artwork, genre, lossless, flagged -- and `trend` shows the series with
  what moved, so maintenance can be seen to be winning or losing. A scan that
  found what the last one found records nothing. `library.db` v5 adds the
  `snapshots` table, created in place like every version before it (#3).
- Per-album and per-track genre overrides. `[albums."Artist"]` and
  `[tracks."Artist"]` sit above the artist table and the most specific entry
  wins, so an artist who crosses genres can finally be expressed (#3).
- `stk wantlist diff --new` reports only what the review queue has never seen.
  A plain diff re-reports every track it has already searched for and failed on,
  which is what made it unusable as a recurring sync (#3).
- Searches are scored as a batch: where several tracks can come from one peer at
  essentially the same quality, that peer is preferred for all of them -- one
  queue position instead of eight, one rip, one tagging convention. The window
  is narrower than any difference of codec, bitrate tier, sample rate or
  content, so it can never buy a lossy file or a live recording, and a promoted
  result is flagged in the review list. `uploader.prefer_one = false` turns it
  off (#3).
- Rust implementation of the whole CLI, under `rs/`. Ported step by step against
  `oracle/` -- output captured from the Python build against a real 266-file
  library -- and every capture reproduces: the 434 name forms on both their
  normalised and romanised columns, all 258 rows of the verify report, and the
  tag dump, stats, genre, artwork, title-duplicate and content-duplicate
  documents. Startup drops from about three quarters of a second to single-digit
  milliseconds.
- `stk get` takes several tracks, and a list from a file or from stdin, where it
  previously took exactly one query.
- `library.db` v4: `files` gains `codec`, `bitrate`, `sample_rate`, `bit_depth`,
  `verdict` and `verdict_at`; a `changes` table records every `--apply` write,
  grouped by the run that made it. Columns are added in place, so a database an
  earlier build wrote opens where it stands.
- `stk changes list` and `stk changes undo`, so a bad genre map entry or the
  wrong artist's cover across an album can be taken back — per file, or a whole
  `--apply` run at a time. Replaced covers are kept beside the index and shared
  between files that carried the same one. Undo refuses a file edited since it
  was logged unless `--force` says otherwise, and logs itself like any other
  write.
- `stk verify` records each verdict in the index, and `--changed` then skips
  files that still carry one. It is the slowest command here, and on a second
  run that is most of the library.
- `review.db` v2: `tracks` gains `queued_at` and `attempts`. A failed transfer
  promotes the next candidate rather than ending the track, and `stk fetch
  download` gives up on one queued past `slskd.stall_after_seconds`, cancels it
  in slskd and queues the next candidate in its place.
- `library.roots` and `library.exclude`, for a library spread across more than
  one drive and for keeping audiobooks and sample packs out of the index.
- Concurrency where the time actually goes: the verify fan-out over files, and
  the slskd searches, which ran one at a time and were mostly spent waiting.
- `stk` CLI covering the whole workflow: `config`, `doctor`, `library`,
  `wantlist`, `fetch`, `review`, `verify`, `dupes`, `art`, `genre`, `tags`,
  `changes`.
- `stk review list/approve/reject`, replacing hand-editing an `approve` column
  in a CSV. Shows each candidate's full remote path and its warnings. The
  Textual app this began as did not survive the port.
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
- `stk get "Artist - Title"` for fetching a single track by name, with
  `--lossless`, `--min-bitrate`, `--yes` and `--queue-only`. The batch flow
  needs an export; this needs a song title.
- Request pacing for artwork providers, at each service's published limit --
  one second for MusicBrainz, three for iTunes. The limiter lives on the
  provider, so a slow service does not slow a fast one.
- `ranking.require_lossless` and `ranking.min_bitrate` quality floors.
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

### Removed

- The Python implementation, once the Rust build reproduced every reference
  capture. It remains reachable at the `python-reference` tag:
  `git checkout python-reference -- py/`.
- The Textual review app, which went with it. `stk review list --candidates`,
  `stk review approve` and `stk review reject` cover the same workflow.

### Fixed

- `sanitize_folder_name` did not strip backslashes, so an artist name containing
  one produced a nested directory and the finished download was then looked for
  somewhere it had never been written (#5).
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
- `genre apply --apply` and `art fetch --apply` left the cached index holding
  what was true before they ran, so a following command reported the state from
  before the edit.
- Long paths in command output are folded rather than truncated, so the paths
  the user acts on can be copied.
- `ranking.prefer_lossless` was declared in the configuration but never read,
  so turning it off did nothing.
- `stk get`, `stk verify` and `stk doctor` were registered as command groups, so
  their arguments were parsed as subcommand names.
- The User-Agent sent to artwork providers no longer advertises a project URL
  that does not exist. MusicBrainz asks callers to identify themselves and
  throttles those that do not, so a dead URL was worse than none; set
  `artwork.contact` to a real URL or email to be identified.
