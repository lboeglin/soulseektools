# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [1.1.1] - 2026-10-26

### Added

- One-line installers for Windows (`stk-installer.ps1`) and Linux and macOS
  (`stk-installer.sh`), which put `stk` in `~/.local/bin` and on the PATH.
- `stk self-update` upgrades a copy the installer put in place;
  `--check` only says whether a newer release is out.
- Each release archive comes with a `.sha256` checksum.

### Changed

- Release archives no longer carry the version in their name:
  `stk-x86_64-pc-windows-msvc.zip`, and `.tar.xz` instead of `.tar.gz` on
  Linux and macOS.

## [1.1.0] - 2026-10-26

### Added

- A `year` column in `tags dump --round-trip`. `tags apply` writes it as
  given (`2000`, `2000-09` or `2000-09-09`), refuses any other form with
  the sheet's line, and `changes undo` puts the previous year back.
- `tags identify`, `art fetch` and `sync` fill in a missing year from the
  release they matched, never over one already there. `art fetch` only does
  so for a verified cover; `artwork.fill_year = false` turns that off.
- `stk art set <file|folder> <image|url>` embeds a cover you choose, in one
  file or every file directly in a folder, and `changes undo` puts the old
  one back.

### Changed

- `library.db` and `review.db` use SQLite's WAL journal, so a command that
  only reads never waits on another run's writes. SQLite keeps `-wal` and
  `-shm` files next to each.

### Fixed

- Two `get` running at once tagged, verified, retried and filed each other's
  downloads, and one could move a file the other had not checked yet. Each
  `get` now handles only the downloads it queued, says how many others are in
  progress, and a failed transfer is retried once whichever run sees it.
- The first scan after an upgrade read every file while holding the index's
  write lock, so another run could fail with `database is locked`.
- `get --lossless` took an AAC `.m4a` as lossless. An `.m4a` now counts as
  lossless only when the peer reports a bit depth.
- A download `verify` called FAKE or UPSAMPLED was still filed into the
  library, with a cover. It goes to the Recycle Bin now, and the next
  candidate is downloaded in its place.
- The download summary said OK for a track that then failed with nothing
  left to try.
- `rustls`, `rustls-webpki`, `webpki-roots` and `webpki-root-certs` updated
  for RUSTSEC-2026-0346 to -0349 and -0381 to -0383.
- `verify` held an AAC `.m4a` to lossless, so an honest 128 kbps one came out
  FAKE. Only an ALAC one is checked now, and `library stats` and
  `dupes resolve --keep lossless` no longer count an AAC one as lossless.
- Undoing a cover change put a PNG back labelled as a JPEG.
- `organise` cut a closing bracket off a title or album it filed by
  (`Crazy on Emotion (Extended`). Only a separator stranded by an empty
  placeholder is trimmed now, never a tag's own characters.
- A file with no title tag lost a number its name starts with (`32ki, …`
  became `ki, …`), and one crediting more than its folder's artist lost that
  artist (`, Hatsune Miku - …`). Titles already guessed this way are
  re-guessed once when the index is upgraded.
- A library already organised with these names moves back to the full ones on
  the next `organise --apply`.

## [1.0.2] - 2026-10-01

### Fixed

- A failed or stalled download was retried with the next candidate even when
  it was flagged, which filed the wrong version. Retries now skip flagged
  candidates, and a track with only flagged ones left goes back to review.
- Stopping slskd while `sync` or `get` waited aborted the run, leaving what had
  arrived unfiled. The wait now ends there and files what arrived.

## [1.0.1] - 2026-09-30

### Fixed

- A track queued by `stk get` and downloaded later by `sync` or `fetch
  download` had its tags replaced with the typed name. A typed name now only
  fills what the file lacks, whichever command downloads it.

## [1.0.0] - 2026-09-30

The project went from ad-hoc scripts and a Nicotine+ plugin to one `stk`
command, first in Python and then rewritten in Rust.

### Added

- `stk get` for one track, several, a list or stdin, and `get --album` for a
  peer's whole folder.
- Wantlists from Spotify/Exportify CSV, Apple Music `Library.xml`, M3U, YouTube
  Music Takeout, Last.fm exports and profiles (`lastfm:<user>`), and Discogs
  collections.
- `stk wantlist diff`, with `--new` for what the queue has never seen. A row
  whose ISRC a library file also carries matches it outright, whatever script
  either title is written in.
- `stk fetch search` and `download` through slskd. Results are ranked on the
  full remote path and scored as a batch that prefers one peer. A stalled peer
  hands its whole group on. Searches start at least
  `slskd.search_interval_seconds` apart, so a large sync doesn't flood the
  server.
- Candidates are flagged when they lack a version the wanted title names
  (`missing:reloaded`) or add one (`extra:eurobeat mix`).
- `stk review list`, `pick`, `albums` and `approve-album`. A search keeps ten
  candidates per track, and `--candidates N` on `get`, `sync` and `review`
  picks how many to show.
- `stk sync`, which runs scan, diff, search, pick, download and organise in one
  command. `--yes` takes the best candidate unless it is flagged, which
  `--force` overrides.
- Downloads are tagged from their review row (`fetch.tags`), verified on
  arrival, and given a cover when they come without one.
- `stk verify` for transcodes, upsampled MP3s and fake hi-res. Verdicts are
  stored so `--changed` can skip files already checked.
- `stk dupes titles`, `content` and `resolve`. `resolve` quarantines copies
  instead of deleting them.
- `stk art check` and `fetch` (iTunes, MusicBrainz and Cover Art Archive),
  rejecting covers whose artist doesn't match.
- `stk genre check` and `apply` from a TOML map, with per-album and per-track
  overrides.
- `stk tags dump`, `apply` and `identify` (AcoustID).
- `stk organise`, `stk loudness` (ReplayGain 2.0), and `stk playlist export`,
  `genre` and `artist`. `organise` reuses an existing artist folder whose name
  reads the same (`稲葉曇` into `Inabakumori`), or `organise.artist_aliases`.
- `stk library scan`, `stats` and `trend`, over several roots with exclusions.
- `stk changes list` and `undo` for everything `--apply` writes.
- `stk doctor`, which checks the configuration, the library roots and, when
  an API key is set, whether slskd is logged in and what it shares.
- `--json` on every command, and `stk completions`.
- Layered configuration: flag, environment variable, `stk.toml`, user config,
  defaults.

### Changed

- Rewritten in Rust as a single binary that needs nothing installed. ffmpeg,
  ffprobe and fpcalc are no longer used.
- Romaji is read with IPADIC through lindera instead of kakasi, so the binary
  is MIT.

### Removed

- The Python implementation, the Textual review app and the Nicotine+ plugin.

[Unreleased]: https://github.com/lboeglin/soulseektools/compare/v1.1.1...HEAD
[1.1.1]: https://github.com/lboeglin/soulseektools/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/lboeglin/soulseektools/compare/v1.0.2...v1.1.0
[1.0.2]: https://github.com/lboeglin/soulseektools/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/lboeglin/soulseektools/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/lboeglin/soulseektools/releases/tag/v1.0.0
