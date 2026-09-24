# Changelog

Notable changes to this project. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org/).

## [Unreleased]

The project became an installable package with a single `stk` command, replacing
a set of ad-hoc scripts and a Nicotine+ plugin, and was then rewritten in Rust.

### Added

- `stk completions bash|zsh|fish|powershell|elvish` prints a completion
  script, generated from the command definitions so it covers every command
  group and flag (#19). It needs no configuration, so a broken `stk.toml`
  does not stop it.

- A Last.fm profile is read as a wantlist, named `lastfm:<user>` wherever an
  export file goes (#20). Its loved tracks are wanted, and `--min-plays N`
  adds every track the user has played at least N times, read from their
  top tracks until the play counts drop below N. Last.fm needs an API key
  even for a public profile; it is read from `LASTFM_API_KEY`, and a read
  without one is refused before any request is made. `stk doctor` reports
  whether it is set.

- A Discogs collection export is read as a wantlist (#20). It lists releases,
  so each one's tracklist is looked up through the Discogs API, paced to 25
  requests a minute or to 60 with a `DISCOGS_TOKEN`, and kept in
  `wantlist.discogs_cache` so a release is only ever looked up once. A
  compilation's tracks keep their own artists, Discogs' `(2)` disambiguation
  is dropped from a name, and the movements of an indexed work are named
  after it. The export is recognised by its `release_id` column, and
  `--format discogs` names it. `stk doctor` reports whether the token is set.

- A wantlist can be more than a streaming-service CSV (#18). Whatever reads
  an export -- `wantlist diff`, `playlist export`, `sync` -- now also reads:

  - a Last.fm export, with a header (where the title column is `track`) or in
    the headerless `artist,album,title,date` shape `lastfm-to-csv` writes;
  - a YouTube Music Takeout, and a list of video titles with no artist
    column, split on ` - ` the way `stk get` splits a query and marked as a
    guess so a download's title tag wins over it;
  - an Apple Music or iTunes `Library.xml`, songs only: podcasts, films, TV,
    music videos and audiobooks are skipped, and Apple's own fields --
    `Track Number` among them -- are kept for tagging the download;
  - an M3U or M3U8 playlist, the inverse of what `playlist` writes: an entry
    whose file is on this machine is described by its tags, otherwise by its
    `#EXTINF` line, and failing that by its filename, marked as a guess.

  The format is recognised from the file's content, and `--format` overrides
  it. `--min-plays N` keeps the tracks listed at least N times, once each,
  which is what makes a scrobble log a wantlist rather than every song ever
  heard once.

  A `track` column that holds the title is no longer read back as a track
  number when a download is tagged: a song called "22" is not track 22.

- A stalled preferred uploader takes its whole group with it (#17). The batch
  pass steers a batch to one peer, but the stall retry worked one track at a
  time: when that peer went offline, each of its tracks timed out on its own
  and fell to its own next candidate -- a different peer per track, since the
  preference had reordered the lists. Eight tracks from one uploader became
  eight uploaders, the outcome the preference exists to avoid, reached slowly.

  Now, once more than one of a preferred peer's tracks has stalled, or slskd
  reports the peer offline, every track still queued with them is cancelled
  and the preference is run again over their untried candidates, so a second
  peer covering two or more of them takes them together. `fetch download` and
  `sync` report it as one line -- which peer stalled, how many tracks moved and
  to whom, how many fell back individually -- and under `--json` as
  `stalled_peers`. A peer stalling on one file while delivering the rest, or a
  stall on a candidate that was never preferred, keeps the per-track retry.

- `stk verify` catches upsampled files, not only transcodes (#16). A 128 kbps
  MP3 re-encoded at 320 and a 44.1 kHz/16-bit master padded out to 96 kHz/24-bit
  both got through: the first was never looked at, since only lossless
  containers were, and the second's cutoff at 22.05 kHz matched no encoder, so
  it came out `SUSPECT` or `OK`.

  Neither is a transcode -- the audio is intact, the file is merely bigger than
  it needs to be -- so both get a new verdict, `UPSAMPLED`. An MP3 is judged
  against its header bitrate: a brickwall a band or more below where that
  bitrate would put it is an upsample, with a VBR average discounted to three
  quarters first. A file above 48 kHz whose content ends in a steep edge at a
  lower standard rate's Nyquist is an upsample from that rate. The edge is
  sought at the ordinary noise floor, where a sharp resampler walls content
  off, and at -85 dB, where ffmpeg's softer default drops it to silence a
  few kHz past. A 24-bit file whose samples use 16 bits is an upsample on that
  alone, read off the decoded samples at no extra cost.

  `--lossy` adds MP3s to a library sweep, which otherwise still means the
  lossless files; a named file or folder is checked whatever it holds. `stk
  fetch` checks every MP3 that arrives, and counts `UPSAMPLED` as a failure
  there. A verify run still fails only on `FAKE`. `library stats` counts
  `UPSAMPLED` as flagged. In the reference library, two files turned out to be
  16-bit audio in 24-bit containers, which ffmpeg confirms, and three 96 kHz
  files turned out to end at 21-23.5 kHz over digital silence.

- `stk dupes resolve`, which keeps one copy of each duplicated track and moves
  the rest into a quarantine beside the index (#15). `dupes titles` and `dupes
  content` were the only maintenance commands that stopped at a report, handing
  over a list to take to the file manager.

  Both passes run and their pairs are merged into groups, since three copies of
  a track come back as three pairs. `--keep` picks the copy that stays --
  `lossless` by default, then `largest`, `tagged` or `oldest` -- and the plan
  names each other copy with the reason it lost. A group with any pair below
  `dupes.resolve_min_score` (0.95) is shown and left alone. `--apply` moves the
  losers to `<index dir>/quarantine/<run>/`, keeping their path below the root;
  nothing is deleted. Each move is logged, so `changes undo --last --apply` puts
  them back, the index forgets them so `stats` stops counting them, and folders
  the move emptied are removed only if genuinely empty.

- `stk sync`, which runs the recurring loop -- scan, diff, search, pick,
  download, organise, scan again -- as one command under one `--apply` (#14).
  Every step already existed on its own, but keeping a library in step with a
  growing export meant remembering six commands and their order, and one step
  had no command at all: `fetch download` queues and collects once, so waiting
  for what it queued meant running it again by hand until the queue emptied.

  The waiting is new. After queueing the approved picks, the sync keeps
  collecting what finishes, retrying what stalls with the next candidate and
  sending whatever a failure promoted, until nothing it sent is still on its way
  or `--timeout` passes. A track queued by an earlier run is waited for too,
  since nothing else will collect it; a timeout abandons nothing, and the next
  sync records what arrived in the meantime. What arrived is then organised out
  of `slskd.download_directory` into the library and the library is scanned
  again, so `library trend` gets its point.

  The steps are the library calls the individual commands wrap, not the
  commands, so the output is one report and `--json` one document with a section
  per step. Without `--apply` only the index and the review queue change -- the
  scan, diff, search and pick, which write nothing else on their own either --
  and the download and the organise are described instead. The pick is the
  `review pick` prompt, skipped under `--yes` for the top candidate everywhere;
  `--json` without `--yes` is refused, as `pick` refuses it. `--limit` caps
  searches and downloads, and the exit code is non-zero when a search, a
  download or a move failed, or a download is fake.

- `stk get --album`, which fetches a peer's folder as one decision, starting
  from an album name (#13). `stk get "DECO*27 - Roki"` got one track and there
  was no equivalent for a record: getting *Ghost Rule EP* meant typing every
  title and hoping `review albums` then offered a folder, or writing a CSV.
  Neither is "get me this album". The album logic existed but could only run
  over tracks already in the queue, grouping *their* candidates by peer folder,
  and it could not start from a name because the tracklist is not known.

  So the folder is what is scored. The search, the client and the reject and
  penalise pass over the remote path are the ones `stk get` already uses, read
  across the folder's files together rather than one at a time -- a folder is
  queued whole or not at all, so one `(Karaoke)` track inside it is evidence
  about all of them. What cannot come along is the per-file name match, which
  has no counterpart without a tracklist, and `--lossless` and `--min-bitrate`
  therefore apply per folder: one MP3 among the FLACs is a warning on the
  listing rather than a file quietly dropped out of a folder that would arrive
  whole either way. Every file is named before the prompt, and the `.cue`, the
  `.log` and the scan are counted and left where they are -- the album was asked
  for, not the uploader's paperwork.

  Taking one writes a review row per audio file, each approved with that peer's
  file as its only candidate. That is the same shape `review approve-album`
  leaves behind, so `fetch download`, `collect` and the stall retry need no
  change: the folder is a unit of decision, not of transfer. With one candidate
  there is nothing for the retry to promote, so a file that never arrives fails
  on its own and `stk get` by name is how to go and find it elsewhere. A row
  already queued or downloaded is left alone and named.

  The titles are read off the filenames, because a folder offers nothing else to
  read them off, so `review.db` v4 records that they are guesses and
  `fetch download` never writes one over a title the peer had right -- `replace`
  is a statement about rows that were asked for by name. The track number comes
  off the filename too and reaches the file through the column an export's
  number already travels in. `review.album_minimum_tracks` decides how many
  audio files make a folder worth offering, for this and for `review albums`
  alike.

- `stk tags identify`, which identifies a file from its audio and tags it
  (#12). `tags dump` could show `01 track.flac` under `Unknown Artist` and
  nothing in the tool could say what it was: a download from before the review
  row existed, a file copied in from elsewhere, a row that was wrong. The
  acoustic fingerprint `dupes content` already computes is the expensive half
  of an AcoustID lookup, and the other half is one request, answered with the
  MusicBrainz recording the print belongs to and every release it is on.

  A match is proposed only when the service's score clears `identify.min_score`;
  below it nothing is written, the same asymmetry `wantlist diff` keeps,
  because a wrongly tagged file is a lie `organise` will then file under. Among
  the releases the first rule that separates two decides, and the plan says
  which: a title matching the file's album tag, then the folder name, then a
  recording credited to the artist the file is filed under, then the earliest
  date, then the original over a compilation, then the id. The recording's
  credit is the artist and the release's the album artist, so a compilation
  reads `Dave Rodgers` on `Various Artists` rather than the other way round.
  Genres and the cover are left to the commands that own them.

  Fingerprints are computed in parallel and cached in the index, so a library
  that has run `dupes content` pays nothing, and a tagged file's print is
  re-filed under its new modification time so the next command finds it too.
  Every write goes through the change log as one `tags identify` run.

- A TLS backend. `reqwest` was built with no TLS feature, so every request to
  an https service failed to send; `art fetch` reported each provider as
  unavailable rather than failing, which is how it went unseen. The rate
  limiter, the User-Agent and the transport moved out of `artwork/` into
  `services.rs`, being shared by every web service the tool talks to;
  `artwork.contact` is sent to all of them.

- `stk review pick`, which walks the tracks awaiting review and asks one question
  each, so a decision costs a keystroke rather than a track id read off a table
  and typed back into `stk review approve`. Reviewing a wantlist is the only
  high-volume decision loop in the tool -- everything else is a batch job that
  runs to completion or a report read once -- and over a 147-track wantlist it
  was the worst ergonomics in it (#8). Enter takes the top pick, a number takes
  that candidate, `r` rejects, `s` passes over, `u` takes back the decision just
  made and offers that track again, `q` stops.

  It is a prompt loop, not the full-screen app the Python's Textual build had.
  Every command carries `--json` and a meaningful exit code, so the tool is
  scriptable throughout; a full-screen app would reimplement each command's
  presentation and give the project a second place where output is decided, and
  it would have landed without automated coverage in a project whose rule is
  that no change lands without tests for its behaviour. The loop instead takes
  its input and its transcript as arguments, so a whole session is a string of
  answers in a test, and `stk review list/approve/reject` remain the scriptable
  form of the same decisions. It refuses to run under `--json` rather than
  blocking a pipeline on a keystroke nobody can see it waiting for.

  Only tracks awaiting review are walked, ranks are passed to the store exactly
  as the listing shows them, and nothing is downloaded: a decision changes
  `review.db` and `stk fetch download` is what acts on it. Undo clears the
  candidate's `attempted` mark along with the approval, or the retry added in #2
  would fall past a candidate nothing had actually tried and the best copy would
  never be fetched.
- `stk fetch download` tags each finished file from the review row it was
  fetched for (#11). A download used to keep whatever its uploader left on it:
  nothing, `Track 01` under `Unknown Artist`, or a full block with a URL in the
  album field and a `ripped by` comment. The row carried the artist, title and
  album that were asked for, and nothing wrote them, so `organise`, `genre
  check`, `dupes titles` and `art fetch` were blind to the file until someone
  went through `tags dump` and `tags apply` by hand.

  `fetch.tags` decides how the two meet. `replace`, the default, writes the row
  over the peer's fields and drops the rest of the block; `fill` writes only
  what the peer left empty; `keep` leaves the file as it arrived. A blank in the
  row never blanks the file, so an `stk get` row with no album leaves the
  peer's. Album artist, genre, disc number, the loudness tags and the embedded
  cover are the peer's to keep under every policy, being either what the row
  cannot say or what another command owns; the track number comes from the
  export when it carries one. Every write is logged under one `fetch download`
  run, the dropped items as a single `extras` row, so `stk changes undo --last
  --apply` puts the peer's block back whole.
- `stk loudness`, which measures how loud every file actually is and writes the
  ReplayGain tags a player levels playback with. Albums are mastered at wildly
  different levels, so a shuffled library jumps in volume between tracks; the
  fix is metadata rather than a re-encode, so it is reversible and costs nothing
  in quality. Both figures are written -- track gain for shuffle, album gain so
  a quiet interlude stays quiet relative to the track after it -- because
  players choose between them per context. The scan is EBU R 128 against a
  -18 LUFS reference, which is ReplayGain 2.0 and what `loudgain`, `rsgain` and
  foobar2000 write today, with a true peak alongside it so a player applying
  positive gain has a clipping guard.

  It measures through the `ebur128` crate, a port of libebur128, rather than
  ffmpeg's filter of the same name: a scan built on parsing a subprocess's text
  output would have been a line-for-line rewrite when #9 removed ffmpeg. Checked against the filter on the same files, the two agree to
  within 0.04 dB -- which is the rounding the filter's one-decimal output
  imposed, so the crate is the more precise of the pair.

  An album is defined by its tags and nothing else, since that is how a player
  groups when it applies album gain; a file with no album tag gets a track gain
  only, rather than sharing a figure with whatever else sits in its folder. A
  record is measured as one programme rather than averaged from its tracks --
  the gate deciding which 400 ms blocks count is relative to the programme, so
  an album's threshold is the album's -- which is why a run is grouped, decided
  and scanned per record throughout, and why an existing scan is never
  overwritten without `--force` a record at a time: an album gain derived from
  half a record is not one. A run covering only part of a record -- one track
  named on the command line, or a record with a track that could not be read --
  writes a track gain and says why there is no album one.

  Opus keeps its gain in the container header, so those files also get
  `R128_TRACK_GAIN`, the one place `library/tags.rs` dispatches on a container.
  All four figures share one write-log row rather than one apiece, against the
  rule the other fields follow: they are a single measurement, and a row apiece
  would let an undo restore a gain without the peak that bounds it (#7).
- `library.db` v6: `files` gains the six loudness columns and `snapshots` gains
  `with_replay_gain`, added in place like every version before them. They come
  off the header parse `describe` already performs -- which now happens once
  rather than three times per file -- so `stk library stats` and `stk library
  trend` can count the files nothing has level-matched for free. That figure is
  useful even to someone who scans elsewhere: a fetch that drops forty
  un-scanned files into the library is otherwise invisible, and `stk` is the
  thing that put them there. The snapshot column is nullable, so a measurement
  taken before it existed reads `-` rather than zero. Adding a column moves
  neither the size nor the mtime a row is invalidated by, so the first scan
  after the upgrade fills the six columns for rows an older build wrote --
  updating those alone, since re-describing a row would throw away the verdict
  `stk verify` recorded against it (#7).
- `stk review albums` and `stk review approve-album`, so a peer's folder can be
  taken as one decision instead of one approval per track. Soulseek's real unit
  is the folder: one peer holding four of the waiting tracks is one queue
  position, one rip and one tagging convention. An offer is derived from the
  candidates already found — `username` plus the folder of the `remote_path`, no
  new column — and only ever covers tracks still waiting, so the folder's other
  contents are never fetched and a track already decided by hand is never
  overruled. Each covered track keeps its own candidate, so a file that does not
  arrive still falls back to its own next one. `review.album_minimum_tracks`
  sets how many tracks make a folder worth offering (#6).
- `review.db` v3: `candidates` gains `attempted`, added in place like every
  version before it. The retry promoted `approved_rank + 1`, which assumes the
  choice being retried was the first one — so anything ranked *above* it was
  unreachable and a track could fail with its own best copy never tried. That
  was reachable by hand before (`stk review approve <id> 3`) and is routine once
  an album can be approved, since preferring a peer's folder is exactly the case
  where the pick is not rank 1. The fallback now promotes the best candidate
  nobody has tried yet (#6).
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
- Acoustic fingerprinting for content duplicates, over Chromaprint -- the
  fingerprint AcoustID uses -- compared locally by bit error rate, so no web
  service and no API key are involved.
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

- The crate moved from `rs/crates/stk` to the repository root. `rs/` existed
  to sit beside `py/` while both builds shared the tree; with the Python gone
  the extra level only made every `cargo` command need a `--manifest-path`,
  and the workspace around it had one member. `cargo build`, `cargo test` and
  `cargo install --path .` now work from a fresh clone as they would in any
  Rust repository.
- `oracle/` is committed. It is the reference output every step of the port was
  checked against, and the Python build that produced it is gone, so the one
  untracked copy on one machine was the only one there could ever be. The
  filenames it carries come from a private library, which is what had kept it
  out of the repository; the repository is private too (#10). Tests that need
  only the capture, such as the 434 name forms, now run on CI as well; those
  that also need the library still skip without it.
- **`stk` needs nothing installed to run.** ffmpeg, ffprobe and fpcalc are gone
  (#9), which was one of the three reasons for the Rust port and the last of
  them still outstanding: the README claimed the binary was self-contained two
  bullets above requiring two external programs. Audio is decoded by
  `symphonia`, fingerprinted by `rusty-chromaprint`, measured by `ebur128` and
  drawn by `realfft` and `image`, all compiled in the way `rusqlite` already
  compiled SQLite. `cargo test` needs nothing installed either, and CI installs
  no audio tooling.

  `verify` used to spawn five processes per file -- one probe and one decode per
  window. It now opens each file once and seeks within it, and the four windows
  it reads come off that single parse. Every one of `oracle/verify.csv`'s 258
  rows is unchanged except one, and the three pairs in
  `oracle/dupes-content.json` are unchanged.

  Two things were lost with the subprocesses. Monkey's Audio (`.ape`) and
  WavPack (`.wv`) have no mature pure-Rust decoder, so they leave the extension
  lists entirely -- a `.ape` upload also stops counting as a lossless upgrade;
  the reference library contains none of either. Opus has no pure-Rust decoder
  either but does have tags this project writes, so it stays taggable and is
  skipped by `stk loudness` and `stk dupes content` rather than dropped. And
  one file in the reference library is 32-bit FLAC, which symphonia accepts and
  then decorrelates through an `i32` too narrow to hold `2*mid + side`: it is
  refused with a reason rather than decoded into noise.

  The spectrogram behind `verify --spectrograms` is drawn here now instead of by
  ffmpeg's `showspectrumpic`. It is not pixel-identical and does not need to be
  -- nothing compares these images to anything, a person looks at one and
  decides whether to keep a file -- but it does carry a labelled frequency axis,
  because a picture that shows content stopping without saying where is no use.
- **A cached fingerprint from `fpcalc` is recomputed rather than reused.**
  `rusty-chromaprint` agrees with `fpcalc` on about 98.5% of a fingerprint's
  bits: far above the 0.9 that decides a duplicate, and far below the 1.0 two
  reads of one file should score. The `fingerprints` table already recorded what
  produced each row and treats a name it does not know as a miss, so an existing
  `library.db` invalidates the old rows by itself.
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
- The `legacy/` directory: the original scripts and the Nicotine+ plugin, kept
  so their output could be diffed against the CLI during the first migration.
  The implementation they were to be compared with has itself been removed, so
  the comparison is over. They remain at the `original-scripts` tag (#10).
  `stk review import` still reads the plugin's `soulseek_review.csv`.
- The loudness-envelope fingerprint, which existed only as the fallback for a
  missing `fpcalc`. Chromaprint is compiled in and cannot be missing, so the
  fallback had no way to be reached -- and it was the only thing in the crate
  that needed to resample.
- The `ffmpeg`, `ffprobe` and `fpcalc` checks from `stk doctor`, which now
  reports on configuration, the library, the index and slskd.

### Fixed

- `stk organise` folded case only on Windows, but macOS volumes are
  case-insensitive too. There a case-only rename was refused as a collision
  with itself, and two moves whose destinations differed only in case were
  both planned, so the second rename replaced the first file. The collision
  check now folds case on macOS as well, and whether a destination is the
  file being moved is asked of the filesystem on Unix rather than guessed from
  the platform, so a case-sensitive APFS volume or an exFAT drive on Linux is
  judged by what it actually does. Found while adding macOS to CI (#19).
- `stk get` read an exhausted stdin as the empty answer, which takes the top
  candidate, so `stk get "..." < /dev/null` chose and downloaded a file with
  nobody at the keyboard -- against the invariant that nothing downloads without
  a human choosing it. End of input is now "nothing downloaded". Found while
  building the review picker on the same prompt, where it would have approved a
  whole queue.
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
