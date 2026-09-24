# Contributing

## Getting set up

```bash
cargo build
cargo run -- --help
```

Nothing else needs installing, for the build or for the tests.

## Checks

Everything CI runs, locally:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs all three on Linux, macOS and Windows.

CI also checks the dependency tree against the RustSec advisory database and
the licence list in `deny.toml`, so a vulnerable, yanked or unexpectedly
licensed crate fails the build rather than waiting to be noticed. Locally:

```bash
cargo install cargo-deny --locked
cargo deny check
```

A new dependency whose licence is not on the list needs a decision, not just a
line added to it. `kakasi` is the one exception: it is GPL-3.0, which makes a
distributed `stk` binary GPL-3.0 as well, until #21 replaces it.

The tests that diff against `oracle/` -- reference output from the original
Python build, captured against a real library -- skip when it is absent, which is
what CI sees: `oracle/` is not committed, because it names every file in a
private library. Run them locally against your own capture.

## Code as documentation

Write code that explains itself, and reserve prose for what the code cannot say.

- **Clippy runs at `pedantic`, denied.** Where a lint is relaxed it is relaxed in
  the smallest scope that works, with the reason written down -- the numeric
  casts in `audio/` are bounded by an FFT size, not by an address space.
- **Domain types, not dicts.** A `LibraryEntry` beats a dict with
  `"artist_norm"` and `"title_romaji"` keys; the first version of this project
  was the second thing, and the keys drifted apart between copies.
- **Name for intent.** A function whose name explains it needs no comment saying
  the same thing again.
- **No comment restates a line.** `# increment the counter` is noise. Comments
  exist for *why*, where the why is not recoverable from the code: a threshold
  derived from measurement, a protocol quirk, a deliberate asymmetry. There are
  a few of those in this codebase and each earns its place.
- **Docstrings on public API.** A module docstring should say what the module is
  for and what problem shaped it. Internals get a clear name instead.

## Test everything

No change lands without tests for its behaviour.

- **Test behaviour, not implementation.** Name a test for the property it
  establishes: `a_strong_title_carries_a_weak_artist`, not `score_2`.
- **A fixed bug gets a regression test**, named after the failure. Several
  existing tests are named this way and carry a comment explaining the original
  failure — `a_silent_file_is_not_mistaken_for_genuine_lossless` is one.
- **No network in tests.** The Soulseek backend and both HTTP clients are traits,
  and the flows run against recorded responses. That boundary is what makes the
  search and download tests runnable with no daemon and no account -- it is not
  an implementation detail to be optimised away. The real transport itself is
  tested against a one-shot server on the loopback interface, never a live
  service.
- **Audio fixtures are generated where they can be.** `tests/support/mod.rs`
  writes FLAC and WAV from a signal it synthesises, so no tool has to be
  installed to run the suite. What it cannot write is committed under
  `tests/data/audio` instead: there is no pure-Rust encoder for MP3, AAC,
  Vorbis or Opus, and the transcode check needs audio a real lossy encoder
  produced, since the brickwall it recognises is LAME's and an authentic one
  cannot be synthesised. The upsampled MP3s are committed for the same reason:
  they are LAME's 128 kbps wall under a 320 kbps header. The hi-res fakes are
  not -- `hi_res_flac` writes 24-bit, padded or brickwalled FLAC at any rate.
  `scripts/make-fixtures.sh` regenerates the committed files with ffmpeg; no
  test runs it.
- **`scripts/word_chars.py` is a fixture generator, not a build step.** It dumps
  the characters Python's `\w` and `\s` match, and `character_classes.rs` checks
  the crate's own predicates against that dump: Rust's `\w` would otherwise take
  in 10,549 combining marks Python folds to a space, and the difference would
  surface as shifted matching scores rather than a failing test. The dump is
  committed under `tests/data`, so no build or test run needs Python. Only
  regenerating it does, which happens approximately never.
- **Prefer a pure function you can test directly** over one that has to open a
  file. The spectral classifier and the table rendering are both pure for this
  reason.

## Commits

[Conventional commits](https://www.conventionalcommits.org/), one logical change
each:

```
feat(matching): add shared romaji-aware fuzzy matching core
fix(verify): treat a silent file as unanalysable
docs: rewrite the readme around the cli
```

Types in use: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `chore`, `ci`.
A `!` after the scope marks a breaking change.

Write the body for someone who was not there: what changed, and what problem it
solves. If a defect surfaced while writing tests, say what it was.

## Project shape

```
src/
  config.rs         layered settings; every tunable lives here
  models.rs         the domain types shared across layers
  matching.rs       fuzzy name matching, used by everything
  organise.rs       the layout pattern, and moving files into it
  playlist.rs       rendering M3U, so an order can leave the library again
  replaygain.rs     loudness figures, album grouping, and the tags they become
  retag.rs          what a fetched file is tagged with, from its review row
  tagsheet.rs       reading an edited `tags dump` back
  wantlist/         reading an export into wanted tracks, one reader per
                    format -- a Discogs collection and a Last.fm profile
                    through their APIs -- and diffing them against the library
  identify/         the AcoustID lookup, and choosing a release from its answer
  text/             normalisation and romaji transliteration
  library/          scanning, tags, the SQLite index, the --apply write log,
                    and the per-scan measurements behind `library trend`
  audio/            decoding, spectral analysis, fingerprinting, the
                    spectrogram behind `verify --spectrograms`, and the
                    EBU R 128 loudness scan behind `stk loudness`
  services.rs       what the public web services share: pacing, identification,
                    the transport
  artwork/          providers and the artist-verification guard
  soulseek/         the client trait, the slskd backend, ranking
  review/           the review queue, the album view over it -- over the queue,
                    and over a search for one album -- and the picker that
                    walks it
  cli/              one module per command group
```

There is no `tui/`, and `review/picker.rs` is a prompt loop rather than a
full-screen app. Every command has `--json` and a meaningful exit code, so the
tool is scriptable throughout; a full-screen app would reimplement each command's
presentation and give the project a second place where output is decided. The
picker takes its input and its transcript as arguments for the same reason the
spectral classifier is pure: a whole session is then a string of answers in a
test.

Two rules keep this from drifting back into what it replaced:

1. **One place per concern.** Tags are handled in `library/tags.rs` and nowhere
   else, audio is decoded in `audio/decode.rs` and nowhere else, and Soulseek's
   wire format lives in `soulseek/slskd.rs` and nowhere else. Nothing spawns a
   subprocess: `stk` is one binary and depends on none (#9).
2. **No literals for things that vary.** Paths, thresholds and term lists are
   configuration. If you find yourself typing a path into a module, it belongs in
   `config.rs`.

## Anything that writes to a user's files

Must show its plan and do nothing without `--apply`. A music library is not
recoverable from a backup nobody made.

It must also log what it wrote, through `library/changes.rs`, so `stk changes
undo` can put it back. A half-covered log is worse than none: it implies a
completeness it does not have.
