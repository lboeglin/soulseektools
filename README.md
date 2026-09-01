# SoulseekTools

Build a music library from a streaming-service export via Soulseek, and keep it
clean afterwards: no duplicates, full cover art, consistent genre tags, and no
FLACs that are really transcoded MP3s.

One `stk` command-line tool with a terminal review app, driven by a config file.
Nothing is hardcoded to one machine, every command that writes to your files
shows its plan first, and nothing downloads without you choosing it.

```
$ stk library scan
Indexed 1284 file(s) under E:\Music: 1284 added, 0 updated, 0 unchanged, 0 removed

$ stk wantlist diff "My Spotify Library.csv"
147 of 1431 track(s) missing, 1284 owned
Queued 147 new track(s) for searching

$ stk fetch search
Searched 147 track(s); 139 have candidates to review. Run stk review to choose.

$ stk review          # pick a candidate per track, one keystroke each
$ stk fetch download  # queue the approved picks, verify what arrives
```

Or, for a single song:

```
$ stk get "DECO*27 - Roki"
```

## Requirements

- A Rust toolchain, to build it. Nothing is needed to run the result.
- [ffmpeg](https://ffmpeg.org/) and ffprobe on `PATH` — required for the
  transcode check and for content-based duplicate detection
- [slskd](https://github.com/slskd/slskd) — only for searching and downloading.
  See [docs/slskd-setup.md](docs/slskd-setup.md). Every library-maintenance
  command works without it.
- [Chromaprint](https://acoustid.org/chromaprint) (`fpcalc`) — optional. Without
  it, content duplicates fall back to a loudness-envelope fingerprint that needs
  only ffmpeg.

## Install

This is not published to PyPI. Install it from a clone:

```bash
git clone git@github.com:lboeglin/soulseektools.git
cd soulseektools

cargo install --path rs/crates/stk   # puts `stk` on your PATH
# or, to work on it instead:
cargo build --manifest-path rs/Cargo.toml   # then run target/debug/stk
```

Then set it up:

```bash
stk config init          # writes ./stk.toml
$EDITOR stk.toml         # set library.root
stk doctor               # check everything is reachable
```

`stk doctor` reports what is present and what is missing, and exits non-zero if
anything required is unavailable.

`cargo install --path rs/crates/stk` builds from the clone, not from a registry.
Re-run it after pulling changes; `cargo uninstall stk` removes it.

## Configuration

Settings resolve in this order, first match winning:

1. a command-line flag (`--library`, `--config`)
2. a `STK_`-prefixed environment variable (`STK_LIBRARY__ROOT`)
3. `./stk.toml`, or the nearest one in a parent directory
4. `~/.config/soulseektools/config.toml` (`%LOCALAPPDATA%` on Windows)
5. the built-in defaults

Secrets are the exception and are never read from the file: export
`SLSKD_API_KEY` instead. `stk config show` prints the effective settings with
secrets omitted, so the output is safe to paste.

Music on two drives, with the folders you would rather not index left out:

```toml
[library]
roots   = ['E:\Music', 'F:\More Music']
exclude = ["**/Audiobooks/**", "**/Samples/**"]
```

A pattern is matched against the path below its own root. `root = '...'` still
works and means a library with one.

The settings worth knowing about:

| Key | What it does |
| --- | --- |
| `library.root` | Where your music lives. Scanned recursively. |
| `library.roots` | Several of them, for a library spread over more than one drive. |
| `library.exclude` | Globs to skip, so audiobooks and sample packs stay out of the index. |
| `matching.*` | How similar two names must be to count as the same track. |
| `ranking.reject_terms` | Terms that disqualify a search result outright. |
| `ranking.penalise_terms` | Terms that push a result down the list. |
| `ranking.prefer_lossless` | Rank lossless above lossy. On by default. |
| `ranking.require_lossless` | Discard lossy results entirely. Off by default. |
| `ranking.min_bitrate` | Discard lossy results below this. Lossless is never discarded. |
| `slskd.stall_after_seconds` | How long a download may sit queued before the next candidate is tried. |
| `verify.steep_db_per_khz` | How sharp a frequency cutoff must be to look artificial. |
| `artwork.require_artist_match` | Refuse cover art belonging to a different artist. |
| `artwork.contact` | A URL or email identifying you to MusicBrainz. Optional. |
| `genres.map_path` | Your artist-to-genre map. Start from `data/genres.example.toml`. |

## Getting tracks by name

You do not need a CSV to grab a song, or a handful of them:

```bash
stk get "DECO*27 - Roki"                     # search, show what turned up, pick one
stk get "Roki" --artist "DECO*27"            # or give the artist separately
stk get "Roki" --lossless                    # only accept lossless
stk get "Roki" --min-bitrate 320             # or set a floor for lossy results
stk get "Roki" --yes                         # take the best result without asking

stk get "DECO*27 - Roki" "ZUN - Heian Alien" # several at once
stk get -a "DECO*27" Roki "Ghost Rule"       # one artist across the lot
stk get --from wanted.txt                    # a list, one "Artist - Title" per line
... | stk get --from -                       # or from stdin
```

One track is interactive: it ranks the results exactly as the batch flow does,
prints them with their full remote paths and warnings, and asks which to take.

Several tracks, or any list, is a batch. Those go into the review queue and are
searched there, because a list is already a decision about what you want --
there is nothing to diff it against. Nothing downloads until you choose, in
`stk review` or with `--yes`.

A list may carry blank lines and `#` comments, so it can be kept by hand.

`--queue-only` approves a candidate without downloading it, so you can gather a
few and run `stk fetch download` once.

## Building a library from an export

**1. Work out what is missing.**

```bash
stk library scan
stk wantlist diff "My Spotify Library.csv" --owned owned.csv
stk wantlist diff "My Spotify Library.csv" --new   # only what is newly missing
```

Matching is romaji-aware: a Spotify entry for `千本桜` matches a file named
`Senbonzakura.flac`, which plain string similarity cannot see. It is also
deliberately conservative — an uncertain match is reported as *missing* rather
than owned, because a duplicate download costs a few megabytes and a track
dropped by mistake is simply gone. `--owned` writes what it did match, with
scores, so you can check the decisions rather than trust them.

Missing tracks are added to the review queue.

Re-run the same diff against a grown export and it reports everything missing
all over again, including the tracks a previous run searched for and gave up on
— they are, after all, still missing. `--new` reports only the tracks the review
queue has never seen, which is what makes the diff usable as a recurring sync
rather than a one-shot. It changes what you are told, not what is queued.

**2. Search.**

```bash
stk fetch search
```

Each result is scored on format, bitrate, uploader availability, how well the
name matches — and on its **full remote path**. That last part matters: a file
named exactly right is still, often enough, a live recording or an orchestral
cover, and only the folder it sits in says so.

**3. Review.**

```bash
stk review
```

A terminal app listing each track with its candidates. Every candidate shows its
format, uploader, queue position, full remote path and any warnings raised.
Press `1`, `2` or `3` to take a candidate, `r` to reject the track, `u` to undo,
`a` to show everything, `q` to quit.

Prefer not to use the app? `stk review list --candidates`, `stk review approve
<id> <rank>` and `stk review reject <id>` do the same thing.

**4. Download.**

```bash
stk fetch download
```

Approved picks are queued through slskd into a folder named for the artist. Each
completed lossless download is run through the transcode check automatically and
the verdict recorded against the track.

## Keeping the library clean

```bash
stk verify                 # find FLACs that are really transcoded MP3s
stk dupes titles           # the same song under two spellings of an artist
stk dupes content          # the same audio under unrelated names
stk art check              # files with no cover, or a broken one
stk art fetch --apply      # look art up and embed the verified matches
stk genre check            # genres in use, and artists your map does not cover
stk genre apply --apply    # write the mapped genres
stk tags dump              # every file's artist/album/title/genre as CSV
stk playlist genre Touhou  # an m3u8 of everything carrying a genre
stk changes list           # what those --apply runs actually wrote
```

**Nothing is written without `--apply`.** Run the command, read the plan, then
run it again with the flag.

### Playlists

The library is built from an ordered export and the order then survives
nowhere: the index is keyed by path and reads back sorted by path.

```bash
stk playlist export "My Spotify Library.csv" -o library.m3u8
stk playlist genre Eurobeat -o eurobeat.m3u8
stk playlist artist "DECO*27" -o deco.m3u8
```

`export` writes the tracks the library actually holds, in the export's own row
order. `genre` and `artist` match on the normalised name, so `DECO*27` also
finds the `DECO_27` folder. With no `-o` the playlist goes to stdout.

Paths are absolute by default and written with forward slashes, which every
player accepts. `--relative` writes them relative to the playlist file instead,
so the playlist can be moved along with the music.

### Undoing what `--apply` wrote

Every command that writes to your files records what it replaced, so a bad
entry in the genre map, or the wrong artist's cover across an album, is not a
one-way trip:

```bash
stk changes list                       # newest first, with the run each belongs to
stk changes list --command "art fetch"
stk changes undo --last                # what putting the last run back would do
stk changes undo --last --apply        # do it
stk changes undo 42 --apply            # or just the one file
```

A run is one `--apply` invocation, so `--last` takes back the whole command
rather than its last file. Undo refuses a file that has changed since it was
logged rather than overwriting a tag you have since fixed by hand — `--force`
overrules that. Replaced covers are kept beside the index under `changes/`, and
files that carried the same cover share one copy of it.

### Finding fake lossless files

`stk verify` looks at where a file's audio actually stops in the frequency
spectrum, and at *how abruptly*. A lossy encoder leaves a brickwall — tens of dB
lost over a few hundred hertz, at one of a handful of bitrate-specific
frequencies. A loud master rolls off gradually over several kilohertz. Only the
first is a transcode.

Checking for "content above 16 kHz" alone does not work: heavily limited modern
masters roll off early and would be flagged wholesale.

```bash
stk verify --report verify.csv --spectrograms --only-flagged
stk verify --changed                    # only files with no current verdict
```

Verdicts are `OK`, `SUSPECT` (sharp cutoff at no standard bitrate — look at it
yourself), `FAKE` and `ERROR`. The command exits non-zero if any file is `FAKE`.

Every run records its verdicts in the index, and `--changed` then skips the
files that still carry one. This is the slowest command here, and on a second
run that is most of the library. It is opt-in rather than the default because
the report carries detail a stored verdict cannot reproduce, so a plain
`stk verify` still covers everything. A file that has changed comes back either
way, and so does an `ERROR`, which says the audio could not be read rather than
anything about the audio.

### Finding duplicates

Two passes, because they fail in different directions:

- `dupes titles` catches the same song filed under two spellings of an artist
  (`Inabakumori` and `稲葉曇`), but is blind to untagged files with meaningless
  names.
- `dupes content` catches those, by comparing the audio itself. Duration is only
  a prefilter; the fingerprint decides. Grouping by duration and file size alone
  produces coincidental matches between unrelated tracks of similar length.

Neither deletes anything. They report pairs, largest copy first, for you to
judge.

### Cover art

`stk art fetch` looks a track up through iTunes, then MusicBrainz and the Cover
Art Archive, and **rejects any result whose artist does not match the file's**.
Search match quality outside the mainstream is poor, and the wrong cover is
worse than no cover. `--allow-unverified` waives the check if you really want it.

MusicBrainz asks callers to identify themselves and throttles those that do not.
Set `artwork.contact` to a URL or email address if you use it heavily; nothing
is sent that you did not put there.

### Genres

The artist-to-genre map is a TOML file, not code:

```toml
"DECO*27" = "Vocaloid"
ZUN = "Touhou"
"SOUND HOLIC" = ["Touhou", "Eurobeat"]
```

Artists match on their normalised name, so a `DECO*27` entry also covers the
`DECO_27` folder it is filed under. `data/genres.example.toml` is a starting
point; it describes one specific library's roster and is meant to be edited.

An artist who crosses genres can be pinned down further:

```toml
[artists]
Shihori = "Touhou"

[albums.Shihori]
"Prismatic" = "J-Pop"

[tracks.Shihori]
"Sakura Sunrise" = "Anime"
```

The most specific entry wins -- track, then album, then artist -- and all three
match on the normalised name. Both override sections are keyed by artist first
rather than by a combined `"Artist/Album"` string, because an artist name may
itself contain a slash. `stk genre check` still reports on artists alone: one
album's worth of exception is not something to say about a roster.

## Migrating from the old scripts

This replaces a set of ad-hoc scripts and a Nicotine+ plugin. If you have a
`soulseek_review.csv` from the plugin, its tracks and approvals carry over:

```bash
stk review import soulseek_review.csv
```

The original scripts are kept under [`legacy/`](legacy/README.md) so their
output can be compared against the new commands. They are excluded from lint,
type checks and coverage.

Delete the directory once you have cross-checked — `git rm -r legacy`. They stay
retrievable from the `original-scripts` tag:

```bash
git show original-scripts:check_fake_flac.py
git checkout original-scripts -- .        # restore the whole original tree
```

Why the plugin was dropped, and the Nicotine+ behaviour that motivated it, is
recorded in [docs/nicotine-quirks.md](docs/nicotine-quirks.md).

## Written in Rust

The tool was originally Python and was ported in phases against reference output
captured from that build -- see
[issue #1](https://github.com/lboeglin/soulseektools/issues/1). Every capture
reproduces, so the two agreed on all 266 files of a real library before the
Python was removed.

That implementation is still reachable, tagged `python-reference`:

```bash
git checkout python-reference -- py/     # restores the whole tree
```

It is worth knowing it exists for one reason: the review TUI has not been
rewritten. `stk review list --candidates`, `stk review approve` and
`stk review reject` cover the same workflow non-interactively.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

MIT. See [LICENSE](LICENSE).
