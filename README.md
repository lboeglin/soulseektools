# SoulseekTools

`stk` builds a music library from a streaming-service export via Soulseek, then
keeps it clean: no duplicates, cover art on everything, consistent genres, and
no FLACs that are really transcoded MP3s.

Every command that writes to your files prints its plan first and does nothing
without `--apply`, and every write is logged so `stk changes undo` can put it
back.

## Requirements

- A Rust toolchain to build it. Nothing else is needed at runtime: decoding,
  loudness and fingerprinting are compiled in.
- [slskd](https://github.com/slskd/slskd) for searching and downloading (see
  [docs/slskd-setup.md](docs/slskd-setup.md)). Library maintenance works
  without it.

FLAC, WAV, AIFF, ALAC, MP3, AAC and Vorbis are decoded. Opus tags are read and
written, but its audio isn't decoded.

## Install

```bash
git clone git@github.com:lboeglin/soulseektools.git
cd soulseektools
cargo install --path .

stk config init    # writes ./stk.toml
$EDITOR stk.toml   # set library.root
stk doctor         # check everything is reachable
```

Shell completions: `stk completions <bash|zsh|fish|powershell|elvish>`.

## Configuration

Settings are read from, first match winning: a command-line flag, an `STK_`
environment variable (`STK_LIBRARY__ROOT`), `./stk.toml` or the nearest one in
a parent directory, `~/.config/soulseektools/config.toml`
(`%LOCALAPPDATA%` on Windows), then the defaults.

Secrets never go in the file. Export `SLSKD_API_KEY`, and `DISCOGS_TOKEN` or
`LASTFM_API_KEY` if you use those wantlists. `stk config show` prints the
effective settings without secrets.

```toml
[library]
roots   = ['E:\Music', 'F:\More Music']
exclude = ["**/Audiobooks/**"]

[organise]
pattern = "{artist}/{album}/{track} - {title}"

[genres]
map_path = "genres.toml"   # start from data/genres.example.toml
```

`stk config init` writes every setting with its default.

## Getting music

One song, or a few:

```bash
stk get "DECO*27 - Roki"               # search, list results, pick one
stk get "Roki" -a "DECO*27" --lossless
stk get --from wanted.txt              # one "Artist - Title" per line
stk get --album "DECO*27 - Ghost Rule EP"   # a peer's whole folder
```

A whole export (Spotify/Exportify CSV, Apple Music `Library.xml`, M3U, YouTube
Music Takeout, Last.fm scrobbles or `lastfm:<user>`, Discogs collection):

```bash
stk library scan
stk wantlist diff "My Spotify Library.csv"   # queue what the library lacks
stk fetch search                             # search Soulseek for the queue
stk review pick                              # choose a result per track
stk fetch download                           # download, tag and verify
```

Or all of that in one go, waiting for downloads and organising what arrives:

```bash
stk sync "My Spotify Library.csv" --apply
```

Matching is romaji-aware (`千本桜` matches `Senbonzakura.flac`) and
conservative: an uncertain match counts as missing. Results are ranked on
format, bitrate, peer availability and the full remote path, so a live
recording or a cover filed under the right name ranks low. When several tracks
can come from one peer at the same quality, that peer is preferred for all of
them; `stk review albums` shows those folders explicitly.

## Keeping the library clean

```bash
stk verify                 # fake lossless, fake hi-res, upsampled MP3s
stk dupes titles           # same song under two spellings
stk dupes content          # same audio under unrelated names
stk dupes resolve --apply  # keep one copy, quarantine the rest
stk art check              # missing or broken covers
stk art fetch --apply      # embed covers whose artist matches
stk genre check            # genres in use, unmapped artists
stk genre apply --apply    # write genres from the map
stk tags dump -o tags.csv --round-trip
stk tags apply tags.csv --apply
stk tags identify --apply  # tag unknown files via AcoustID
stk organise --apply       # move files into the configured layout
stk loudness --apply       # ReplayGain 2.0 track and album gain
stk playlist genre Touhou -o touhou.m3u8
stk library trend          # how the numbers change scan to scan
```

`verify` exits non-zero when it finds a `FAKE`. It looks at where the spectrum
stops and how steeply, because a lossy encoder leaves a brickwall and a loud
master only rolls off.

The genre map is TOML, keyed by artist, with optional per-album and per-track
overrides:

```toml
"DECO*27" = "Vocaloid"
"SOUND HOLIC" = ["Touhou", "Eurobeat"]

[albums.Shihori]
"Prismatic" = "J-Pop"
```

## Undo

```bash
stk changes list
stk changes undo --last            # preview
stk changes undo --last --apply    # put the last run back
```

Undo refuses a file that has changed since it was written, unless `--force`.
`dupes resolve` never deletes. It moves copies to `quarantine/` beside the
index.

## Scripting

`stk --json <command>` prints one JSON document instead of a table. Errors go
to stderr and exit codes are the same as without `--json`. `review pick` is
interactive and refuses `--json`.

## History

This replaced a set of ad-hoc scripts and a Nicotine+ plugin, then went from
Python to Rust. The earlier trees are tagged `original-scripts` and
`python-reference`. See [docs/nicotine-quirks.md](docs/nicotine-quirks.md) for
why the plugin was dropped.

## Licence

MIT, see [LICENSE](LICENSE). The binary embeds IPADIC (NAIST/ICOT licence) and
Unihan kanji readings in `src/text/kanji_readings.tsv`
([Unicode licence](https://www.unicode.org/license.txt)).
