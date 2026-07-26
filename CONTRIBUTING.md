# Contributing to RedSwarm

PRs are welcome. This project enforces a strict engineering standard - see [`AGENTS.md`](AGENTS.md) for the full rules. The short version: fix root causes (no band-aids), one source of truth per value, 100% test coverage for new code, zero clippy warnings.

## Build

```bash
cargo build --release          # backend
./build.sh                     # frontend bundle (run after any CSS/JS edit)
cargo run --release            # run the binary (requires config.toml in the cwd)
```

The frontend is zero-dependency (no npm). `build.sh` concatenates the CSS (inlined into `index.html`) and the JS (content-hash-fingerprinted bundle). Both the Askama template and the JS bundle are compiled/served at build/runtime; the Rust test suite enforces cross-language sync (labels, DOM hooks, module lists).

## Test and lint

```bash
cargo test                     # 625 Rust tests
cargo clippy -- -W warnings    # must be zero warnings
./build.sh                     # regenerate the bundle after JS/CSS edits
```

Frontend tests are browser-based: visit `/static/tests/index.html` while the server is running (218 tests, 29 files, zero-dependency harness).

## Code style

- **Rust**: edition 2024. Follow `rustfmt` defaults. `#[allow(...)]` attributes are forbidden - fix the underlying issue, never silence the compiler.
- **Comments**: explain *why*, not *what*. Doc comments (`///`) on every public item, following RFC 1574 (summary sentence in third person, `# Panics` / `# Errors` / `# Safety` sections where applicable). No commented-out code. No `// TODO` without a linked issue.
- **Single source of truth**: any value used more than once lives in exactly one place - `config.toml` if configurable, `data/` if not. Never retype the same literal at two call sites. The enforcement tests in `data/mod.rs` catch regressions.
- **Frontend**: no duplicate logic, no `innerHTML` with variable interpolation (use `escHtml`/`escAttr`), no bare `fetch()` outside `utils/net.js`, no `transition: all`.

## PR process

1. Fork and branch from `main`.
2. Write tests for the new behavior; ensure `cargo test` and `cargo clippy -- -W warnings` pass with zero warnings.
3. If you changed CSS/JS, run `./build.sh` and commit the regenerated bundle... actually, the bundle is gitignored - just run `./build.sh` locally to verify.
4. Keep the diff focused - one logical change per PR. Stage only intended files.
5. Reference any related issue in the PR description.

## Reporting bugs

Open an issue with:
- What you expected vs what happened
- Steps to reproduce (the `config.toml` settings, the tracker, the torrent - redact passkeys/provider names)
- The relevant log lines (`RUST_LOG=redswarm=debug`)

For security issues, see [`SECURITY.md`](SECURITY.md) - do **not** open a public issue.
