# stk — Python implementation

The original implementation, and the reference the Rust port in [`../rs/`](../rs)
is diffed against. It works and stays working until parity; see
[issue #1](https://github.com/lboeglin/soulseektools/issues/1).

- What the tool does, and how to use it: [`../README.md`](../README.md)
- Working on it: [`../CONTRIBUTING.md`](../CONTRIBUTING.md)

```bash
uv sync --all-extras --dev
uv run pytest
uv run mypy
uv run ruff check .
```

Installed as `stk` with `uv tool install --force ./py` from the repository root.
