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
written as tags but not decoded, so `stk loudness` and `stk dupes content` pass
over it. Monkey's Audio and WavPack are not handled at all.

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
| `review.album_minimum_tracks` | How many waiting tracks a peer's folder must hold to be offered as an album. |
| `slskd.stall_after_seconds` | How long a download may sit queued before the next candidate is tried. |
| `fetch.tags` | What a finished download is tagged with: `replace` (default), `fill` or `keep`. |
| `uploader.prefer_one` | Take a batch from one peer where the per-track choice is close. On by default. |
| `uploader.score_window` | How close "close" is. Wider than availability, narrower than any quality gap. |
| `uploader.minimum_coverage` | How many of the batch's tracks a peer must hold to be preferred. |
| `verify.steep_db_per_khz` | How sharp a frequency cutoff must be to look artificial. |
| `loudness.reference_lufs` | What a scanned file is levelled to. -18 LUFS is ReplayGain 2.0. |
| `loudness.true_peak` | Catch inter-sample peaks as well as the loudest sample. On by default. |
| `artwork.require_artist_match` | Refuse cover art belonging to a different artist. |
| `artwork.contact` | A URL or email identifying you to MusicBrainz and AcoustID. Optional. |
| `genres.map_path` | Your artist-to-genre map. Start from `data/genres.example.toml`. |
| `organise.pattern` | Where a file belongs, from its tags. Default `{artist}/{album}/{track} - {title}`. |
| `organise.track_number_width` | How many digits a track number is padded to. Default 2. |
| `organise.destination` | Consolidate everything here. Unset means each file stays under its own root. |

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
does not arrive. The offer is shown before it is taken, and `--tracks` names
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
stk tags apply sheet.csv   # write back an edited one
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
doubted, `FAKE` and `SUSPECT` together -- and stay empty until you have run it. A
column a measurement predates reads `-` rather than zero: that scan did not
count it, which is not the same as having counted none.

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
