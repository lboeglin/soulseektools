# Contributing

## Getting set up

```bash
cd rs
cargo build
cargo run -- --help
```

ffmpeg and ffprobe need to be on `PATH`. Tests that need them say so and return
rather than failing, but a large part of the suite does need them.

## Checks

Everything CI runs, locally:

```bash
cd rs
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs all three on Linux and Windows.

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
  an implementation detail to be optimised away.
- **Audio fixtures are generated, not committed.** `tests/support/mod.rs` encodes
  short files with ffmpeg, so the suite exercises real containers written by an
  independent encoder.
- **Prefer a pure function you can test directly** over reaching into a
  subprocess. The spectral classifier and the table rendering are both pure for
  this reason.

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
rs/crates/stk/src/
  config.rs         layered settings; every tunable lives here
  models.rs         the domain types shared across layers
  matching.rs       fuzzy name matching, used by everything
  playlist.rs       rendering M3U, so an order can leave the library again
  text/             normalisation and romaji transliteration
  library/          scanning, tags, the SQLite index, the --apply write log,
                    and the per-scan measurements behind `library trend`
  audio/            ffmpeg wrappers, spectral analysis, fingerprinting
  artwork/          providers and the artist-verification guard
  soulseek/         the client trait, the slskd backend, ranking
  review/           the review queue
  cli/              one module per command group
```

There is no `tui/`. The review app was Textual and did not survive the port;
`review list/approve/reject` cover the same ground.

Two rules keep this from drifting back into what it replaced:

1. **One place per concern.** Container formats are handled in
   `library/tags.rs` and nowhere else. Soulseek's wire format lives in
   `soulseek/slskd.rs` and nowhere else. Subprocesses live in `audio/ffmpeg.rs`.
2. **No literals for things that vary.** Paths, thresholds and term lists are
   configuration. If you find yourself typing a path into a module, it belongs in
   `config.rs`.

## Anything that writes to a user's files

Must show its plan and do nothing without `--apply`. A music library is not
recoverable from a backup nobody made.

It must also log what it wrote, through `library/changes.rs`, so `stk changes
undo` can put it back. A half-covered log is worse than none: it implies a
completeness it does not have.
