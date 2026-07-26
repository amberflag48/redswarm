# AGENTS.md - project rules for RedSwarm

## Commands
- **Run**: `cargo run --release` (requires `config.toml` in working directory)
- **Test**: `cargo test`
- **Build**: `cargo build --release`
- **Lint**: `cargo clippy -- -W warnings`

## Architecture
- Rust + Tokio + Axum + SQLite + Askama templates
- Single binary, zero external frontend dependencies (all CSS/JS inline)
- Single global SSE connection (`GET /api/events`) drives all dynamic UI - no polling
- 13 modules: announce, api, bencode, config, data, db, engine, magnet, peer_id, peer_server, swarm, templates, torrent, plus `capture` (fingerprint capture), `nat` (NAT-PMP), `render` (server-side HTML fragments), `reload` (hot config reload), `singleton` (process takeover), and `watcher` (config.toml fs watcher) - 19 `mod` declarations in `src/main.rs`
- **config.toml** (project root) is the single source of truth for all tunable values - server settings, engine timing, per-audit defaults, swarm parameters, and client emulation data (peer_id prefixes, user agents, query templates). No defaults exist in Rust code; `config::load()` returns `Err` on missing/invalid config
- `config.rs` contains struct definitions + `validate()` methods for every section, plus channel-capacity plumbing consts (`BROADCAST_CHANNEL_CAPACITY`, `SSE_CHANNEL_CAPACITY`)
- `data/` is the single source of truth for all non-config constants: SQL schema names and base/migration column partition (`schema.rs`), controlled vocabularies - status/phase/event/lifecycle strings (`vocab.rs`), BitTorrent protocol keys/lengths/placeholders (`protocol.rs`), SSE wire names and DOM hooks (`sse.rs`), binary byte units, `fmt_bytes`, `fmt_duration`, and `fmt_speed_*` formatters (`units.rs`), UI display labels (`labels.rs`), and test fixtures (`fixtures.rs`)
- `peer_id.rs` is data-free: only `generate_peer_id`, `generate_key`, and `find_by_client` - all client spec data comes from `config.toml` via `config::ClientSpecConfig`
- Engine probes clients by index into `config.clients`; `working_client` is `Option<usize>`. A task can force a specific client (`AuditConfig.forced_client`) which skips probing entirely - in that case `start_engine` records the working client to the DB and emits `TaskClient` SSE directly, since the probe event (the only path that normally carries it) never runs. On resume, the stored `working_client` is reused without re-probing.

## Rules
- DRY/KISS/YAGNI - no duplicate logic, no unnecessary abstraction
- **Single source of truth for any value used more than once** - if the same data appears in 2+ places in the app, it MUST live in exactly one of two places, never retyped as a literal at each call site:
  - **`config.toml`** if the value is *configurable* (a user might tune it: speeds, intervals, ports, client specs, bounds)
  - **`data/`** if the value is *non-configurable* (a physical constant, protocol key, schema name, controlled vocabulary, UI label, wire-protocol name, test fixture)
  - No value is hardcoded as a Rust literal at multiple call sites. Production code reads from `data::*` or `config.*` - never retypes the same string/number in two files. The enforcement tests in `data/mod.rs` catch regressions (forbidden raw literals, dead consts, DDL drift, etc.).
- Shared functions in shared modules (hex utils in bencode.rs, byte formatting in `data::units`, percent-encoding in `data::protocol`)
- No band-aids or workarounds - fix root causes
- `#[allow(...)]` attributes are forbidden - fix the underlying issue instead of silencing the compiler. This includes `#[allow(dead_code)]`, `#[allow(clippy::*)]`, etc. If code is dead, delete it. If clippy warns, fix the code.
- No over-engineering - solve the current problem
- Full refactoring - no backward compat for internal code
- MCP tools over built-in read/grep/glob
- Delegate file reading to agents to avoid context overflow
- Research with agents before implementing anything uncertain
- Test for failure, not success - every module has failure-path tests
- Zero warnings on build
- Never run `cargo run` yourself - only the user runs the binary. Use `cargo build` and `cargo test` freely
- **Chrome MCP workflow (mandatory for any UI/bug change)**: use Chrome MCP to confirm a bug exists, guide the fix, and verify it's resolved - from first reproduction through final confirmation. The binary must be running for this. If it isn't, stop and ask the user to run `cargo run --release`, then continue once they confirm it's up. Never skip Chrome MCP verification by citing rule 31 - rule 31 forbids *you* from running the binary, not from verifying in the browser
- Always investigate and fix pre-existing issues - don't leave known problems for later
- Always ensure the app is as efficient, responsive, and modern as possible - no re-rendering the entire UI for a single change, no delayed response on button clicks, etc.
- Always add safeguards and input validation as we go - ensure values cannot be set outside expected ranges/bounds
- **Reuse before creating** - before writing any new function, helper, or piece of logic, search the entire codebase (all modules, `data/`, `src/`, templates, JS) for an existing shared function that already does what you need - or something close enough to extend. Duplicate logic is a DRY violation; a near-duplicate is a smell. When adding code, first grep for the nouns and verbs of the operation (e.g. "format", "duration", "duration_since", "encode", "parse", "clamp") across `.rs` and template files, and reuse or extend what exists. Only create a new function if no existing one covers the case
- All tunable values live in `config.toml` - no hardcoded defaults in Rust code. `config.rs` defines structs with `validate()` methods, not defaults
- Never assume anything - always reproduce, test, and verify with Chrome MCP

## Test policy
- 100% test coverage - every new function, branch, and edge case gets a test
- When adding code, add the corresponding tests; when changing code, revise tests to match
- Write tests that actually discover flaws (adversarial inputs, edge cases, failure paths) - never tests that just assert success on the happy path
- Tests must compile and pass with zero warnings before any change is considered done
- Test config helpers live in `config::test_helpers` (no `Default` impls on config structs)
- **Regression discipline**: any bug discovered manually (outside automated tests) must be (1) analyzed for root cause, (2) reproduced by a new or modified test that fails against the unpatched code, (3) confirmed to pass after the fix - this closes the loop and prevents silent regressions
