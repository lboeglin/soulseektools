# Contributing

## Two implementations

The tool is being ported to Rust ([#1](https://github.com/lboeglin/soulseektools/issues/1)).
Until parity, both trees live side by side and both must stay green:

| | |
| --- | --- |
| `py/` | the Python implementation, installed as `stk` |
| `rs/` | the Rust port, built as `stk-rs` |
| `oracle/` | reference output from `py/`, which `rs/` is diffed against |

Side by side rather than on a branch, because the point of the oracle is running
both against the same library and comparing. `oracle/` is not committed: it names
every file in a private library.

## Getting set up

```bash
uv sync --directory py --dev
uv run pre-commit install
uv run --directory py stk --help

cargo build --manifest-path rs/Cargo.toml
```

ffmpeg and ffprobe need to be on `PATH`; tests that need them skip without them,
but a large part of both suites does need them.

## Checks

Everything CI runs, locally:

```bash
cd py
uv run ruff check .
uv run ruff format --check .
uv run mypy
uv run pytest --cov

cd ../rs
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the Python suite on Linux and Windows across Python 3.11, 3.12 and 3.13,
and the Rust suite on Linux and Windows.

## Code as documentation

Write code that explains itself, and reserve prose for what the code cannot say.

- **Full type hints.** `mypy` runs in strict mode.
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
  establishes: `test_a_strong_title_carries_a_weak_artist`, not `test_score_2`.
- **A fixed bug gets a regression test**, named after the failure. Several
  existing tests are named this way and carry a comment explaining the original
  failure — `test_a_silent_file_is_not_mistaken_for_genuine_lossless` is one.
- **No network in tests.** HTTP is mocked with `respx`. The Soulseek backend is a
  protocol, and the flows are tested against a fake implementing it.
- **Audio fixtures are generated, not committed.** `py/tests/conftest.py` encodes
  short files with ffmpeg, so the suite exercises real containers written by an
  independent encoder.
- **Prefer a pure function you can test directly** over reaching into a widget or
  a subprocess. The spectral classifier and the TUI's rendering are both pure for
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
py/src/soulseektools/
  config.py         layered settings; every tunable lives here
  models.py         the domain types shared across layers
  matching.py       fuzzy name matching, used by everything
  text/             normalisation and romaji transliteration
  library/          scanning, tags, the SQLite index
  audio/            ffmpeg wrappers, spectral analysis, fingerprinting
  artwork/          providers and the artist-verification guard
  soulseek/         the client protocol, the slskd backend, ranking
  review/           the review queue
  tui/              the review app
  cli/              one module per command group
```

`rs/crates/stk/src/` mirrors this module for module, so the two trees can be read
against each other. It has no `tui/`: the TUI is deferred out of the Rust v1, and
`review list/approve/reject` cover the same ground.

Two rules keep this from drifting back into what it replaced:

1. **One place per concern.** Container formats are handled in
   `library/tags.py` and nowhere else. Soulseek's wire format lives in
   `soulseek/slskd.py` and nowhere else. Subprocesses live in `audio/ffmpeg.py`.
2. **No literals for things that vary.** Paths, thresholds and term lists are
   configuration. If you find yourself typing a path into a module, it belongs in
   `config.py`.

## Anything that writes to a user's files

Must show its plan and do nothing without `--apply`. A music library is not
recoverable from a backup nobody made.
