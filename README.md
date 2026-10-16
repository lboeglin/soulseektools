# SoulseekTools

[![CI](https://github.com/lboeglin/soulseektools/actions/workflows/ci.yml/badge.svg)](https://github.com/lboeglin/soulseektools/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lboeglin/soulseektools)](https://github.com/lboeglin/soulseektools/releases)
[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue)](LICENSE)

`stk` keeps a music library clean: it finds FLACs that are really transcoded
MP3s, duplicates, missing cover art and genres, fixes tags, files everything
into one layout and writes ReplayGain. It can also fetch what the library lacks
through Soulseek.

Nothing touches your files without `--apply`, and `stk changes undo` reverts
whatever `--apply` wrote.

## Install

Download the archive for your platform from
[Releases](https://github.com/lboeglin/soulseektools/releases) and put `stk` on
your PATH. The binaries are unsigned, so macOS and Windows warn on first run.
To build from source instead, with a Rust toolchain:
`cargo install --git https://github.com/lboeglin/soulseektools`.

```bash
stk config init    # writes ./stk.toml; set library.root
stk doctor         # checks the setup
```

Secrets come from the environment: `SLSKD_API_KEY`, and `DISCOGS_TOKEN` or
`LASTFM_API_KEY` for those wantlists. `stk completions <shell>` prints shell
completions.

## Use

| Command | Does |
|---|---|
| `verify` | finds fake lossless, fake hi-res and upsampled MP3s |
| `dupes titles` / `content` / `resolve` | finds duplicates, quarantines extras |
| `art check` / `fetch` / `set` | finds and embeds missing covers, or one you choose |
| `genre check` / `apply` | tags genres from a map ([example](data/genres.example.toml)) |
| `tags dump` / `apply` / `identify` | edits tags and the year as CSV, or identifies files via AcoustID |
| `organise` | moves files into `{artist}/{album}/{track} - {title}` |
| `loudness` | writes ReplayGain tags |
| `playlist` | writes M3U8 playlists |
| `library stats` / `trend` | shows how the library changes over time |
| `changes list` / `undo` | lists and reverts what `--apply` wrote |

`stk <command> --help` has the details, and `stk --json <command>` prints JSON.

## Fetching from Soulseek

Searching and downloading go through [slskd](https://github.com/slskd/slskd),
which only needs to run while you do ([setup](docs/slskd-setup.md)).

```bash
stk get "DECO*27 - Roki"               # one song
stk get --album "DECO*27 - Ghost Rule" # a peer's whole folder
stk sync "My Library.csv" --limit 50 --apply
```

`sync` compares a list with the library, searches for what is missing, asks you
to pick, downloads and files what arrives. Lists can be Spotify/Exportify CSV,
Apple Music `Library.xml`, M3U, YouTube Music Takeout, Last.fm
(`lastfm:<user>` or an export) or a Discogs collection. The steps also run on
their own: `library scan`, `wantlist diff`, `fetch search`, `review pick`,
`fetch download`.

Candidates that may be the wrong version are flagged, and `--yes` leaves those
for you to pick. Downloads join the artist folder you already have (`稲葉曇`
into `Inabakumori`; `organise.artist_aliases` for other spellings).

Download only what you have the right to: free releases, Creative Commons
music, music you already own. You answer for what you download. See
[being a good peer](docs/slskd-setup.md#being-a-good-peer) for sharing and
for pacing large syncs.

## Licence

MIT, see [LICENSE](LICENSE). The binary embeds IPADIC (NAIST/ICOT licence) and
Unihan kanji readings ([Unicode licence](https://www.unicode.org/license.txt)).
