# SoulseekTools

Build a music library from a streaming-service export via Soulseek, and keep it
clean afterwards: no duplicates, full cover art, consistent genre tags, and no
FLACs that are really transcoded MP3s.

One `stk` command-line tool, driven by a config file. Nothing is hardcoded to
one machine, every command that writes to your files shows its plan first, and
nothing downloads without you choosing it.

```
$ stk library scan
Indexed 1284 file(s) under E:\Music: 1284 added, 0 updated, 0 unchanged, 0 removed

$ stk wantlist diff "My Spotify Library.csv"
147 of 1431 track(s) missing, 1284 owned
Queued 147 new track(s) for searching

$ stk fetch search
Searched 147 track(s); 139 have candidates to review. Run stk review pick to choose.

$ stk review pick     # choose a candidate per track, one keystroke each
$ stk fetch download  # queue the approved picks, verify what arrives
```

Or the whole loop, waiting for the downloads and organising what arrives:

```
$ stk sync "My Spotify Library.csv" --apply
```

Or, for a single song:

```
$ stk get "DECO*27 - Roki"
```

## Requirements

- A Rust toolchain, to build it. Nothing is needed to run the result: the
  decoder, the loudness meter and the acoustic fingerprint are all compiled in.
- [slskd](https://github.com/slskd/slskd) — only for searching and downloading.
  See [docs/slskd-setup.md](docs/slskd-setup.md). Every library-maintenance
  command works without it.

FLAC, WAV, AIFF, ALAC, MP3, AAC and Vorbis are read as audio. Opus is read and
written as tags but not decoded, so `stk loudness`, `stk dupes content` and
`stk tags identify` pass over it. Monkey's Audio and WavPack are not handled at all.

## Install

This is not published to crates.io. Install it from a clone:

```bash
git clone git@github.com:lboeglin/soulseektools.git
cd soulseektools

cargo install --path .   # puts `stk` on your PATH
# or, to work on it instead:
cargo build              # then run target/debug/stk
```

Then set it up:

```bash
stk config init          # writes ./stk.toml
$EDITOR stk.toml         # set library.root
stk doctor               # check everything is reachable
```

`stk doctor` reports what is present and what is missing, and exits non-zero if
anything required is unavailable.

`cargo install --path .` builds from the clone, not from a registry.
Re-run it after pulling changes; `cargo uninstall stk` removes it.

## Configuration

Settings resolve in this order, first match winning:

1. a command-line flag (`--library`, `--config`)
2. a `STK_`-prefixed environment variable (`STK_LIBRARY__ROOT`)
3. `./stk.toml`, or the nearest one in a parent directory
4. `~/.config/soulseektools/config.toml` (`%LOCALAPPDATA%` on Windows)
5. the built-in defaults

Secrets are the exception and are never read from the file: export
`SLSKD_API_KEY` instead, and `DISCOGS_TOKEN` or `LASTFM_API_KEY` if you read a
wantlist from Discogs or Last.fm. `stk config show` prints the effective settings with
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
| `review.album_minimum_tracks` | How many tracks a peer's folder must hold to count as an album, in `review albums` and `get --album`. |
| `slskd.stall_after_seconds` | How long a download may sit queued before the next candidate is tried. |
| `fetch.tags` | What a finished download is tagged with: `replace` (default), `fill` or `keep`. |
| `uploader.prefer_one` | Take a batch from one peer where the per-track choice is close. On by default. |
| `uploader.score_window` | How close "close" is. Wider than availability, narrower than any quality gap. |
| `uploader.minimum_coverage` | How many of the batch's tracks a peer must hold to be preferred. |
| `verify.steep_db_per_khz` | How sharp a frequency cutoff must be to look artificial. |
| `loudness.reference_lufs` | What a scanned file is levelled to. -18 LUFS is ReplayGain 2.0. |
| `loudness.true_peak` | Catch inter-sample peaks as well as the loudest sample. On by default. |
| `artwork.require_artist_match` | Refuse cover art belonging to a different artist. |
| `artwork.contact` | A URL or email identifying you to MusicBrainz, AcoustID and Discogs. Optional. |
| `identify.min_score` | How sure AcoustID must be before a match is proposed. Default 0.85. |
| `genres.map_path` | Your artist-to-genre map. Start from `data/genres.example.toml`. |
| `organise.pattern` | Where a file belongs, from its tags. Default `{artist}/{album}/{track} - {title}`. |
| `organise.track_number_width` | How many digits a track number is padded to. Default 2. |
| `organise.destination` | Consolidate everything here. Unset means each file stays under its own root. |
| `dupes.resolve_min_score` | How sure a duplicate pair must be for `dupes resolve` to act on it. Default 0.95. |
| `wantlist.discogs_cache` | Where the Discogs tracklists already fetched are kept. |

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

## Getting a whole album

An album is not twelve tracks you happen to want together. Soulseek's real unit
is the folder, and `--album` asks for one by name:

```bash
stk get --album "DECO*27 - Ghost Rule EP"    # search, show the folders, take one
stk get --album -a "DECO*27" "Ghost Rule EP" # or give the artist separately
stk get --album "Ghost Rule EP" --lossless   # the quality limits apply per folder
stk get --album "Ghost Rule EP" --yes        # take the best-scored folder
```

```
$ stk get --album "DECO*27 - Ghost Rule EP"
Searching for DECO*27 Ghost Rule EP ...
#  Peer   Album                   Files  Quality        Slot  Queue
1  bulk   DECO27 - Ghost Rule EP  4      4 flac         free  0
2  peerC  Ghost Rule [2017]       4      3 flac, 1 mp3  busy  7

1. @@bulk\Music\DECO27 - Ghost Rule EP
   01 - Roki.flac [flac, 44.1kHz/16bit] user=bulk queue=0 slot=free
   02 - Ghost Rule.flac [flac, 44.1kHz/16bit] user=bulk queue=0 slot=free
   03 - Aimai Elegy.flac [flac, 44.1kHz/16bit] user=bulk queue=0 slot=free
   04 - Hibana.flac [flac, 44.1kHz/16bit] user=bulk queue=0 slot=free
   + 2 other file(s), not queued

Take which? [1-2, or n for none] (1):
```

Every file is named before you choose, because the folder is taken whole: what
six columns say about it is not enough to decide on four transfers. The `.cue`,
the `.log` and the scan are counted so the listing describes the folder as it
is, and left where they are -- the album was asked for, not the uploader's
paperwork.

The same search and the same reject and penalise terms apply, read across the
whole folder rather than one file at a time: one `(Karaoke)` track inside it
says something about all of them. What cannot come along is the name match,
because an album asked for by name brings no tracklist to hold the filenames
against. `--lossless` and `--min-bitrate` therefore apply per folder: one MP3
among the FLACs is a warning on the listing rather than a file quietly dropped
out of a folder you would have received whole.

Each audio file becomes its own review row, approved with that peer's file, so
`stk fetch download` queues them as it queues anything else. The titles are read
off the filenames -- there is nothing else to read them off -- so the queue marks
them as guesses, and a download keeps the peer's own title wherever it has one.
`review.album_minimum_tracks` sets how many audio files make a folder worth
offering, and one already downloaded or already on its way is left alone.

If one file never arrives, it fails on its own: there is no second candidate for
it, because the folder was the point. `stk get "DECO*27 - Roki"` is how you go
and find that one somewhere else.

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

**What an export can be.** Anything that lists the songs you want. The format
is recognised from the file itself; `--format` overrides the guess, and every
command that reads an export (`wantlist diff`, `playlist export`, `sync`)
takes the same flags.

- **A CSV** from Exportify, TuneMyMusic, or your own hand, with any common
  spelling of the artist, title, album and duration columns.
- **A Last.fm export**, with a header (`artist`, `album`, `track`) or in the
  headerless `artist,album,title,date` shape `lastfm-to-csv` writes. A
  scrobble log lists every play, so read whole it is every song ever heard
  once: `--min-plays 3` keeps the tracks played at least three times, once
  each, and `--min-plays 1` only collapses the repeats. It works on any
  export.
- **An Apple Music or iTunes `Library.xml`** (File > Library > Export
  Library). Podcasts, films, TV, music videos and audiobooks in it are
  skipped.
- **An M3U or M3U8 playlist**, from another machine or another player. An
  entry whose file is here is described by its tags; otherwise by the
  `#EXTINF` line a player would show, and failing that by its filename,
  which is a guess.
- **A YouTube Music Takeout** (`music library songs.csv`). A list of video
  titles with no artist column is also read, each title split on ` - ` as
  `stk get` splits a query; the split is a guess, so a download's own title
  tag is trusted over it.
- **A Discogs collection export** (Collection > Export). It lists releases,
  not songs, so each release's tracklist is looked up through the Discogs
  API and every song on it is wanted -- credited to its own artist on a
  compilation, and named after its work when it is one movement of a
  concerto. Discogs allows 25 lookups a minute, or 60 with a personal access
  token exported as `DISCOGS_TOKEN`, so a large collection takes a while the
  first time; every tracklist fetched is kept in `wantlist.discogs_cache`,
  and later runs only look up the releases added since.

  ```bash
  stk wantlist diff "me-collection-20260924.csv"
  ```

- **A Last.fm profile**, named as `lastfm:<user>` where the file would go.
  Its loved tracks are the wantlist; `--min-plays 20` adds every track the
  user has played at least twenty times. Last.fm answers nothing without an
  API key, public profile or not: create one at
  <https://www.last.fm/api/account/create> and export it as
  `LASTFM_API_KEY`.

  ```bash
  stk wantlist diff lastfm:rj --min-plays 20
  ```

Re-run the same diff against a grown export and it reports everything missing
all over again, including the tracks a previous run searched for and gave up on
— they are, after all, still missing. `--new` reports only the tracks the review
queue has never seen, which is what makes the diff usable as a recurring sync
rather than a one-shot. It changes what you are told, not what is queued.

**2. Search.**

```bash
stk fetch search
```

Searches run concurrently, and the results are scored as a batch rather than one
track at a time. Where several tracks could come from the same peer at
essentially the same quality, that peer is preferred for all of them: eight
tracks from one uploader is one queue position, one rip and one tagging
convention, where eight uploaders is eight of each, any of which can stall on
its own. The trade is only made when the per-track choice is genuinely close —
the window is narrower than any difference of codec, bitrate tier, sample rate
or content, so it can never buy a lossy file or a live recording — and a
promoted result is flagged `preferred-uploader` in the review list, so the
reordering is never silent. `uploader.prefer_one = false` turns it off.

If that peer then goes quiet, its tracks move on together. Once more than one
of them has stalled, or slskd reports the peer offline, every track still
queued with them is cancelled and the same preference is run again over what
is left, so a second peer covering the batch takes it as a unit rather than
each track falling to a different uploader. The download report says so in one
line — `bulk stalled; 7 track(s) moved to other; 1 fell back individually`. A
peer that stalls on one file and delivers the rest is left to the ordinary
per-track retry.

Each result is scored on format, bitrate, uploader availability, how well the
name matches — and on its **full remote path**. That last part matters: a file
named exactly right is still, often enough, a live recording or an orchestral
cover, and only the folder it sits in says so.

**3. Review.**

```bash
stk review pick
```

Walks the tracks awaiting review, one question each. Every candidate shows its
format, uploader, queue position, any warnings raised, and its full remote path
on the line beneath — which is what reveals a live recording or an orchestral
cover behind a filename that looks exactly right.

```
$ stk review pick

1/139  DECO*27 - Roki
 1. 01 Roki.flac [flac, 44.1kHz/16bit] user=bulk queue=0 slot=free flags=preferred-uploader
    @@bulk\Music\DECO*27 - Ghost Rule EP\01 Roki.flac
 2. Roki.mp3 [mp3, 320kbps] user=peerC queue=4 slot=busy
    @@peerC\shared\Vocaloid\Roki.mp3
 3. Roki (Live at Budokan).mp3 [mp3, 256kbps] user=livefan queue=1 slot=free
    @@livefan\bootlegs\Roki (Live at Budokan).mp3
Take which? [1-3, r reject, s skip, u undo, q quit] (1):
```

Enter takes the top pick, `1`–`n` takes that one, `r` rejects the track, `s`
passes over it, `u` takes back the decision just made and offers that track
again, `q` stops. Nothing is downloaded here: a decision changes the review
queue, and `stk fetch download` is what acts on it.

It is a prompt loop rather than a full-screen app on purpose. Every command has
`--json` and a meaningful exit code, so the whole tool is scriptable; a
full-screen app would reimplement each command's presentation and give the
project a second place where output is decided. So `stk review list
--candidates`, `stk review approve <id> <rank>` and `stk review reject <id>`
remain the scriptable form, and `pick` is the same decisions asked for out loud.
It refuses to run under `--json`, rather than blocking a pipeline on a keystroke.

A `soulseek_review.csv` left over from the Nicotine+ plugin this tool replaced
carries over, tracks and approvals both:

```bash
stk review import soulseek_review.csv
```

Soulseek's real unit is the folder, though, and `stk review albums` says which
of them one peer could supply several waiting tracks from:

```
$ stk review albums --tracks
#  Peer   Album                    Tracks  Quality
1  bulk   Ghost Rule EP            4       4 flac
2  peerC  DECO*27 - Complete Best  3       2 flac, 1 mp3

$ stk review approve-album 1
Approved 4 track(s) from bulk's "Ghost Rule EP"
```

One peer holding four of them is one queue position, one rip and one tagging
convention. An offer only ever covers tracks already waiting in the queue — the
folder's other contents, a `.cue` or a scan among them, were never searched for
and are never fetched — so approving one is four ordinary approvals made at
once, and each track still falls back to its own next candidate if that file
does not arrive. That is the difference from `stk get --album`, which starts
from an album name rather than from the queue: there the folder *is* what was
asked for, so all of its audio is taken and each file has only the one
candidate. The offer is shown before it is taken, and `--tracks` names
every track and candidate in it. `review.album_minimum_tracks` sets how many
tracks a folder must hold to be worth offering.

This is the same idea as the batch preference in step 2, made explicit: that one
reorders candidates during the search, at the scale of the whole batch; this is
a decision you make, at the scale of one album.

**4. Download.**

```bash
stk fetch download
```

Approved picks are queued through slskd into a folder named for the artist. Each
completed download is tagged from the row it was fetched for, and each lossless
one is run through the transcode check, with the verdict recorded against the
track.

A file from Soulseek arrives with whatever its uploader left on it -- nothing,
`Track 01` under `Unknown Artist`, or a URL in the album field and a `ripped by`
comment -- and the row it was fetched for already says what it is. `fetch.tags`
decides how the two meet:

- **`replace`** (default): artist, title and album are written from the row, and
  the rest of the peer's block -- comment, URL, lyrics -- is dropped. Track
  number, disc number, album artist, genre, loudness tags and the embedded cover
  are kept from the peer, being what the row cannot say or what another command
  owns; a track number the export carries is written over the peer's.
- **`fill`**: only fields the peer left empty are written, for when the peer's
  tagging is trusted over the export's.
- **`keep`**: the file stays as it arrived.

A blank in the row never blanks the file, so a track fetched with `stk get` and
no album keeps whatever album the peer gave it. Every write is logged, and
`stk changes undo --last --apply` puts the peer's block back whole.

The export's album is the streaming service's: sometimes a compilation or a
"(Deluxe Edition)" that the peer's rip is not. `replace` writes it anyway -- the
point is that the file says what was asked for -- and `stk tags apply` remains
the way to correct it.

**Or all of it at once.**

Once the library exists, keeping it in step with a growing export is the same
loop every time: scan, diff, search, pick, download, organise what arrived.
`stk sync` runs it as one command:

```bash
stk sync "My Spotify Library.csv"           # scan, diff, search, pick; show the rest
stk sync "My Spotify Library.csv" --apply   # and download, organise and scan again
stk sync "My Spotify Library.csv" --apply --yes --limit 20
```

It scans the library, queues what the export has that the library does not,
searches for everything waiting to be searched -- leftovers from earlier runs
included -- and walks the review queue with the same prompt as `stk review
pick`. With `--apply` it then queues the approved picks and **waits**: it keeps
collecting what finishes and retrying what stalls with the next candidate (a
stalled preferred peer's whole group at once), until
nothing it sent is still on its way or `--timeout` passes (twice
`slskd.stall_after_seconds` by default, long enough for a stalled transfer to be
retried once). What is still downloading then is not abandoned; the next sync,
or `stk fetch download --collect`, records it. Whatever arrived is organised out
of `slskd.download_directory` into the library, as `stk organise --from` would,
and the library is scanned again so `stk library trend` records the result.

Without `--apply` nothing on disk changes: the scan, the diff, the search and
the pick only write to the index and the review queue, as those commands do on
their own, and the download and the organise are described instead of done.

`--yes` takes the best candidate for every track rather than asking, and is
required under `--json`, which prints one document with a section per step.
`--limit` caps how many tracks are searched and downloaded, so a first sync
against a large export does not queue hundreds of searches at once. The exit
code is non-zero when a search, a download or a move failed, or a download
turned out to be a transcode.

## Keeping the library clean

```bash
stk verify                 # find FLACs that are really transcoded MP3s, and fake hi-res
stk dupes titles           # the same song under two spellings of an artist
stk dupes content          # the same audio under unrelated names
stk dupes resolve --apply  # keep one copy of each, quarantine the rest
stk art check              # files with no cover, or a broken one
stk art fetch --apply      # look art up and embed the verified matches
stk genre check            # genres in use, and artists your map does not cover
stk genre apply --apply    # write the mapped genres
stk tags dump              # every file's artist/album/title/genre as CSV
stk tags apply sheet.csv   # write back an edited one
stk tags identify --apply  # tag untagged files from their audio, via AcoustID
stk organise --apply       # move files into the layout their tags describe
stk loudness --apply       # measure loudness and write the ReplayGain tags
stk playlist genre Touhou  # an m3u8 of everything carrying a genre
stk changes list           # what those --apply runs actually wrote
stk library trend          # whether any of it is winning
```

**Nothing is written without `--apply`.** Run the command, read the plan, then
run it again with the flag.

### Is the maintenance winning?

`stk library stats` is a snapshot. Every `stk library scan` also records one, so
`stk library trend` can show the same numbers as a series:

```
Measured    Files      Tagged     Artwork     Genre      Lossless  Verified    Flagged  Loudness
2026-08-14  2140       1902       1655        2103       890       1204        12       -
2026-09-01  2211 (+71) 1988 (+86) 1790 (+135) 2154 (+51) 921 (+31) 2199 (+995) 8 (-4)   2211
```

A scan that found exactly what the last one found records nothing, so scanning
four times on a quiet afternoon leaves one point rather than four. `Verified`
and `Flagged` both come from `stk verify` -- what it cleared, and what it
doubted, `FAKE`, `SUSPECT` and `UPSAMPLED` together -- and stay empty until you
have run it. A column a measurement predates reads `-` rather than zero: that
scan did not count it, which is not the same as having counted none.

### Organising files

Downloads land as `<download dir>/<Artist>/<whatever the peer called it>`, which
is often `01 track.flac` or worse. `stk organise` moves them into the layout
their tags describe:

```bash
stk organise                                    # see the plan
stk organise --apply                            # move them
stk organise --pattern "{artist}/{title}"       # try a layout for one run
stk organise --from ~/Downloads/slskd --apply   # tidy what a fetch left behind
```

The default pattern is `{artist}/{album}/{track} - {title}`; the placeholders
are `{artist}`, `{album_artist}`, `{album}`, `{title}` and `{track}`. Directories
are separated with `/` on every platform, and the extension is never part of the
pattern -- it comes from the file, since a rename must not lie about the
container.

A placeholder with nothing to fill it takes the punctuation next to it away too,
so a file with no track number becomes `Title.flac` rather than ` - Title.flac`,
and one with no album loses the album directory rather than gaining an
`Unknown Album`. A file with nothing at all to name it after is left where it is.

Nothing is ever overwritten. A destination that already holds a different file
is reported and skipped, as are two files that would land on the same name --
both are found while planning, before anything moves. Emptied folders are
removed behind the move, but only if they are genuinely empty: one still holding
a `cover.jpg` is left alone. Every move is logged, so
`stk changes undo --last --apply` puts them all back.

### Fixing tags in bulk

`stk tags dump` shows what every file claims to be. `stk tags apply` writes an
edited copy of it back, which is the point of dumping in the first place:

```bash
stk tags dump --round-trip -o tags.csv   # edit this in a spreadsheet
stk tags apply tags.csv                  # see the plan
stk tags apply tags.csv --apply          # write it
```

`--round-trip` adds the `album_artist` and `track_number` columns, so a row can
be written back exactly; the plain dump keeps the columns it has always had.
The sheet is keyed on its `path` column and read by header name, so a
hand-written `path,genres` file with two columns is a perfectly good input.

Two rules decide what a cell means:

- **A column the sheet does not have is never written.** That is what makes the
  two-column file safe.
- **An empty cell leaves the tag alone.** The usual reason a cell is empty is a
  half-filled sheet, not a decision to erase something. Pass `--clear-blanks`
  when you do mean to erase, and the plan will show you every tag it would
  remove before anything is written.

Rows naming a file the index does not hold are reported and skipped, never
guessed at. Everything written is logged, so `stk changes undo --last --apply`
takes the whole run back.

### Identifying untagged files

A dump can show you `01 track.flac` under `Unknown Artist` and nothing more,
and a spreadsheet cannot say what it is. `stk tags identify` asks the audio:
the same acoustic fingerprint `dupes content` computes is sent to
[AcoustID](https://acoustid.org), which answers with the MusicBrainz recording
it belongs to and every release that recording is on.

```bash
stk tags identify                     # look up every file missing an artist or a title
stk tags identify ~/Music/slskd       # or just one folder
stk tags identify --all --limit 20    # look tagged files up too, twenty at a time
stk tags identify --apply             # write what was found
```

```
Score  File           Track                           Release                             Why
0.98   01 track.flac  DECO*27 - ゴーストルール          GHOST (2016)                        one of 5 releases dated 2016-09-28, taken by id
0.99   02 track.flac  Anamanaguchi feat. 初音ミク - Miku  HATSUNE MIKU EXPO 2016 E.P. (2016)  the earliest of 10 releases (2016-03-09)
--     03 track.flac  no confident match (best 0.62)
```

Artist, title, album, album artist and track number are written; genres are
left to `stk genre apply`, and the embedded cover is left alone. A match is
only proposed when AcoustID's score clears `identify.min_score`; below it the
file is reported as no confident match and nothing is written, since a
wrongly tagged file is worse than an untagged one -- `stk organise` will file
it under the lie.

A recording is usually on several releases -- the album, a single, a decade of
compilations -- and one has to be chosen, because that is what the album and
track-number tags say. The first of these rules that separates two releases
decides, and the `Why` column names it:

1. a release whose title matches the file's album tag;
2. a release whose title matches the folder the file is in;
3. a recording credited to the artist the file is filed under -- one
   fingerprint can be linked to a cover as well as the original;
4. the earliest release, undated ones last;
5. the original over a compilation, when both came out the same day;
6. the release id, so a tie is the same tie on every run.

On a compilation the recording's credit becomes the artist and the release's
becomes the album artist, so a track reads `Dave Rodgers` on `Various Artists`
rather than the other way round. A solo release leaves the album artist alone.
Names are compared the way everything else here compares them, so a
`DECO_27` folder matches a `DECO*27` credit.

With `--all`, a file that already names an artist is looked up too and its
tags proposed over; where the recording is credited to someone other than the
file's artist the plan says so, because a print linked only to a cover is
usually the cover. Read the plan.

This is slow the first time: a fingerprint decodes two minutes of every file,
in parallel, the way `stk verify` does. The prints are cached in the index, so
a library that has run `stk dupes content` pays nothing, and a run of
`identify` makes the next `dupes content` free in turn. Lookups are paced to
AcoustID's published limit. Every write is logged under one `tags identify`
run, so `stk changes undo --last --apply` takes it back.

The release id is shown under `--json`, so a cover can be fetched for the
exact release later; `stk art fetch` still searches by name for now.

### Playlists

The library is built from an ordered export and the order then survives
nowhere: the index is keyed by path and reads back sorted by path.

```bash
stk playlist export "My Spotify Library.csv" -o library.m3u8
stk playlist genre Eurobeat -o eurobeat.m3u8
stk playlist artist "DECO*27" -o deco.m3u8
```

`export` writes the tracks the library actually holds, in the export's own row
order. The export can be any wantlist, a playlist written elsewhere included,
so a list from another machine comes back pointing at this one's files. `genre` and `artist` match on the normalised name, so `DECO*27` also
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

### Level-matching playback

Albums are mastered at wildly different levels, so a shuffled library jumps in
volume between tracks and you spend the evening on the volume knob. ReplayGain
fixes that without touching the audio: `stk loudness` measures how loud each
file actually is and writes a gain figure into its tags, which the player
applies at playback. It is metadata, so it is reversible and costs nothing in
quality.

```bash
stk loudness                       # see the plan
stk loudness --apply               # write the tags
stk loudness ~/Music/ZUN --apply   # or just one folder, or one file
stk loudness --force --apply       # measure files that already carry a gain
stk loudness --track-only --apply  # skip the album figures
```

Two figures are written, and they are not interchangeable:

- **track gain** flattens every file to the same loudness, which is what you
  want on shuffle;
- **album gain** applies one figure across a record, so a quiet interlude stays
  quiet relative to the track after it.

Players choose between them per context, so writing only one would be half a
job. An album is whatever the tags say it is — `(album artist or artist,
album)`, which is exactly how a player groups when it applies album gain. **A
file with no album tag gets a track gain only** and is left without the other:
falling back to the folder would give a folder of unrelated singles one shared
figure, which is the wrong answer for that folder.

**An existing scan is never overwritten.** A file already carrying a gain — from
foobar2000, `rsgain`, `loudgain` or an earlier run — is left alone, and
`--force` is how you say otherwise. The decision is made per record rather than
per file, so an album half of which is already scanned is measured entire: an
album gain derived from half a record is not an album gain.

For the same reason, a run covering only part of a record writes that part a
track gain and no album gain, and says so: `stk loudness "Album/03 - Track.flac"`
cannot know how loud the record is. Point it at the album, or at the library.

The measurement is EBU R 128 / ITU-R BS.1770 against a −18 LUFS reference —
ReplayGain 2.0, which is what every current scanner writes. A true peak is
measured alongside it, so a player applying positive gain has something to
check itself against. A record is measured as one programme rather than
averaged from its tracks, because the gate deciding which of its moments count
is relative to the whole record. Opus keeps its gain in the container header
rather than in a tag, so those files get `R128_TRACK_GAIN` as well as the usual
four keys; both say the same thing.

Nothing in `stk` plays audio, so these tags are written purely for other
players — `stk` never reads them back to make a decision. What it does read is
whether a file carries one at all: `stk library stats` and `stk library trend`
count the files nothing has level-matched, which is how you notice that last
week's fetch dropped forty un-scanned files into the library.

Every write is logged, so `stk changes undo --last --apply` takes a whole run
back, and a file that had no gain before has none again.

### Finding fake lossless and upsampled files

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
stk verify --lossy                      # MP3s too, checked against their bitrate
```

Verdicts are `OK`, `SUSPECT` (sharp cutoff at no standard bitrate — look at it
yourself), `FAKE`, `UPSAMPLED` and `ERROR`.

`FAKE` is a transcode: a lossy file re-encoded as lossless, audio thrown away and
a lossless container claiming otherwise. `UPSAMPLED` is a file stretched into a
bigger container than its audio needs. Nothing was lost -- the audio is intact --
but the file is several times larger than it has to be, and the peer's claim
about it was false. Three things earn it:

- **An MP3 whose cutoff is below its bitrate.** A 128 kbps file re-encoded at 320
  keeps the 16 kHz wall of the 128 under a 320 kbps header. The wall has to sit a
  band or more below where the header's bitrate would put it. A VBR header is an
  average, so it is held to three quarters of its figure before it is compared.
- **A hi-res file cut off at a lower rate's limit.** A 44.1 kHz master resampled
  to 96 kHz has nothing past 22.05 kHz, the most 44.1 kHz audio can hold; 48 kHz
  has nothing past 24 kHz. A sharp resampler walls the content off just below
  that limit. ffmpeg's default one lets it fade a little past it and then drops
  it to silence, so the check looks for the edge at a much deeper level too. As
  with a transcode, only a steep edge counts: a genuine 96 kHz recording of
  material with nothing above 22 kHz rolls off into its converter's noise, which
  carries on to 48 kHz. When an edge could come from either rate -- 23 kHz is as
  far past 22.05 as a soft resampler reaches and as far short of 24 as a sharp
  one stops -- the verdict names both.

  Only a hi-res claim, above 48 kHz, is judged this way. A 48 kHz file shows too
  little spectrum past 22.05 kHz to tell a resampler from the steep 20 kHz
  low-pass many masters end on, so a 44.1 kHz master delivered at 48 kHz passes.
  48 kHz is a rate records are made at, and resampling to it costs a tenth of the
  space, not four times.
- **A 24-bit file using 16 bits.** A 16-bit master padded to 24 leaves the low 8
  bits of every sample zero, and the samples say so directly.

A hi-res file whose wall sits at a lossy encoder's cutoff is still `FAKE`: the
source was lossy, which is the graver finding.

A library sweep covers the lossless files unless `--lossy` adds the MP3s; a file
or folder named on the command line is checked whatever it holds. The command
exits non-zero if any file is `FAKE`, and not for `UPSAMPLED`, whose audio is
fine where it is. The check `stk fetch` runs on each download covers MP3s as well
as lossless files, and there `UPSAMPLED` counts as a failure like `FAKE`: a
peer who lied about one file is worth knowing about before taking the next.

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

Neither changes anything. They report pairs, largest copy first, for you to
judge.

`stk dupes resolve` acts on them. It runs both passes, merges the pairs into
groups -- three copies of a track come back as three pairs -- and keeps one copy
per group:

```bash
stk dupes resolve                  # see the plan: the keeper, and why each other copy goes
stk dupes resolve --apply          # move the others into quarantine
stk dupes resolve --keep tagged    # keep the best-tagged copy instead
```

`--keep` is `lossless` by default: a lossless container, then the largest. The
FLAC and its own MP3 that `dupes content` finds are exactly the pair where the
lossy copy can be the larger one, so `largest` is there but not the default.
`tagged` keeps the copy with the most of artist, album, title and cover filled
in, and `oldest` the one whose modification time is earliest.

A pair is a judgement, not a fact, so a group is resolved only if every pair in
it scores at least `dupes.resolve_min_score` (0.95 by default). A weaker group is
listed and left alone.

Nothing is deleted. The copies not kept are moved to
`<index dir>/quarantine/<run>/`, keeping their path below the library root, the
index forgets them, and folders the move emptied are removed if they are
genuinely empty. Every move is logged, so `stk changes undo --last --apply` puts
them all back. Empty the quarantine by hand once you are sure.

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

## History

This replaced a set of ad-hoc scripts and a Nicotine+ plugin, and was then
ported from Python to Rust against reference output captured from the Python
build -- see [`oracle/`](oracle/README.md) and
[issue #1](https://github.com/lboeglin/soulseektools/issues/1). Both earlier
trees stay reachable by tag:

```bash
git checkout original-scripts -- .        # the scripts and the plugin
git checkout python-reference -- py/      # the Python implementation
```

Why the plugin was dropped, and the Nicotine+ behaviour that motivated it, is
recorded in [docs/nicotine-quirks.md](docs/nicotine-quirks.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

MIT. See [LICENSE](LICENSE).

The source is MIT, but a built `stk` links
[kakasi](https://github.com/Theta-Dev/kakasi), which is GPL-3.0, so a
distributed binary is GPL-3.0 as a whole. Replacing it is
[#21](https://github.com/lboeglin/soulseektools/issues/21).
