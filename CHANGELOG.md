# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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
- `stk review list`, `pick`, `albums` and `approve-album`.
- `stk sync`, which runs scan, diff, search, pick, download and organise in one
  command.
- Downloads are tagged from their review row (`fetch.tags`) and verified on
  arrival.
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
  `genre` and `artist`.
- `stk library scan`, `stats` and `trend`, over several roots with exclusions.
- `stk changes list` and `undo` for everything `--apply` writes.
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
