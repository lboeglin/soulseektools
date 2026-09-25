# SoulseekTools

`stk` builds a music library from a streaming-service export via Soulseek, then
keeps it clean: duplicates, missing cover art, genres, and FLACs that are really
transcoded MP3s.

Nothing touches your files without `--apply`, and `stk changes undo` reverts
whatever `--apply` wrote.

## Install

Needs a Rust toolchain, and [slskd](https://github.com/slskd/slskd) for
searching and downloading ([setup](docs/slskd-setup.md)).

```bash
cargo install --path .
stk config init    # writes ./stk.toml; set library.root
stk doctor         # checks the setup
```

Secrets come from the environment: `SLSKD_API_KEY`, and `DISCOGS_TOKEN` or
`LASTFM_API_KEY` for those wantlists. `stk completions <shell>` prints shell
completions.

## Use

```bash
stk get "DECO*27 - Roki"               # one song
stk get --album "DECO*27 - Ghost Rule" # a peer's whole folder
stk sync "My Spotify Library.csv" --apply
```

`sync` scans the library, queues what the export has and the library lacks,
searches, asks you to pick, downloads and files what arrives. Exports can be
Spotify/Exportify CSV, Apple Music `Library.xml`, M3U, YouTube Music Takeout,
Last.fm (`lastfm:<user>` or an export) or a Discogs collection. The steps also
run on their own: `library scan`, `wantlist diff`, `fetch search`,
`review pick`, `fetch download`.

To keep the library clean:

| Command | Does |
|---|---|
| `verify` | finds fake lossless, fake hi-res and upsampled MP3s |
| `dupes titles` / `content` / `resolve` | finds duplicates, quarantines extras |
| `art check` / `fetch` | finds and embeds missing covers |
| `genre check` / `apply` | tags genres from a map ([example](data/genres.example.toml)) |
| `tags dump` / `apply` / `identify` | edits tags as CSV, or identifies files via AcoustID |
| `organise` | moves files into `{artist}/{album}/{track} - {title}` |
| `loudness` | writes ReplayGain tags |
| `playlist` | writes M3U8 playlists |
| `library stats` / `trend` | shows how the library changes over time |

`stk <command> --help` has the details, and `stk --json <command>` prints JSON.

## Licence

MIT, see [LICENSE](LICENSE). The binary embeds IPADIC (NAIST/ICOT licence) and
Unihan kanji readings ([Unicode licence](https://www.unicode.org/license.txt)).
