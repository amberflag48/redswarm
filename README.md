# RedSwarm

![Rust](https://img.shields.io/badge/Rust-edition%202024-dea584?logo=rust)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)
![Tests](https://img.shields.io/badge/tests-625%20Rust%20%2B%20218%20JS-brightgreen)

A hands-free, sophisticated P2P ratio cheater for private BitTorrent trackers.

Drop a `.torrent` file or paste a magnet link - the tool probes all client emulations until one passes your tracker's whitelist, then fakes upload/download stats with swarm-aware stealth. It runs unattended: set a task once and the engine keeps ghost-seeding perpetually, adapting speed to live seeder/leecher counts, evading per-torrent balance detection, and resuming automatically after crashes.

## Table of contents

- [Quick start](#quick-start)
- [How it works](#how-it-works)
- [Emulated clients](#emulated-clients)
- [Configuration](#configuration)
- [Architecture](#architecture)
- [Technical details](#technical-details)
- [API](#api)
- [Testing](#testing)
- [Research sources](#research-sources)
- [Legal notice](#legal-notice)
- [User guide](#user-guide)
- [Contributing](#contributing)
- [License](#license)

## Quick start

```bash
cargo run --release
```

Open `http://YOUR_IP:3000` from any machine on your network. Click **+ New task**, drop a torrent or paste a magnet, configure the attack parameters (defaults are tuned for most trackers), and hit **Start task**.

The first build also generates the frontend bundle. If you edit frontend CSS/JS, regenerate it:

```bash
./build.sh
```

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `REDSWARM_ADDR` | `0.0.0.0:3000` | Listen address |
| `REDSWARM_DB` | `sqlite:redswarm.db` | SQLite database URL |
| `RUST_LOG` | `redswarm=info` | Log level filter |

## How it works

### Flow

1. **Probe** - tries each of the 7 emulated clients with a `started` announce. The first one the tracker accepts becomes the working client. On restart, skips probing and reuses the known client. You can also force a specific client per task (skip probing entirely).

2. **Attack** - a single announce session (one peer_id) fakes upload/download stats:
   - **Download + upload mode**: starts as leecher, simulates download progress (decreasing `left`), sends `completed` when done, transitions to seeder, then grows `uploaded`.
   - **Upload only mode**: starts as seeder immediately (`left=0`, `downloaded=torrent_size`), grows `uploaded` from the first announce.

3. **Stealth features** (all on by default):
   - **Speed jitter** - ±20% random variation per second (constant speed is a tell)
   - **Ramp-up** - speed grows from 0 to target over 120 seconds
   - **Bursty upload** - 30% of ticks produce 0 upload (simulating choke/no-requests)
   - **Freeze on 0 leechers** - stops upload growth when nobody can receive
   - **Freeze on 0 seeders** - stops download growth when nobody can supply
   - **Timing jitter** - ±5% random skew on announce intervals (metronomic timing is a fingerprint)
   - **Realistic peer_id/UA** - 7 clients verified against source code as of 2025-2026

4. **Auto-restart on crash** - running tasks are automatically resumed when the backend restarts. The status and last-known peer state (counters, lifecycle phase, elapsed time) are persisted to SQLite every 5 seconds, so a restart picks up exactly where it left off - no lost progress, no re-probing for the working client.

5. **Connectable peer server** - listens on `peer_port` and accepts inbound BitTorrent peer connections. Completes the BT handshake, sends a bitfield (or `have_all` for Fast Extension clients), unchokes, and keeps connections alive with keepalives. Never serves piece data - piece requests are silently ignored, avoiding hash-mismatch bans while appearing as a real, connectable, protocol-participating seeder. Each emulated client's peer-wire fingerprint (reserved bytes, Fast Extension support, keepalive interval) is matched exactly.

### Dynamic speed mode

Instead of a fixed upload speed, the tool reads real seeder/leecher counts from the announce response and calculates a **fair-share upload speed**:

```
fair_share = (leechers × avg_leecher_download × seed_share) / max(seeders, 1)
```

- `avg_leecher_download` = 3 MB/s (typical private tracker leecher)
- `seed_share` = 0.8 (seeders meet ~80% of demand; P2P covers the rest)
- 0 leechers → 0 upload (uploading to nobody is the #1 detection vector)

Download speed in dynamic mode is calculated as:

```
per_leecher = (seeders × avg_download × seed_share) / max(leechers, 1)
```

Both are recalculated on every announce response. The "next announce" countdown in the stats panel shows the time remaining until the next re-announce.

### Per-torrent balance evasion

Private trackers (Unit3D, Ocelot/Gazelle) flag torrents where `|Σuploaded − Σdownloaded| > 5% of torrent size`. In download+upload mode, reporting `downloaded ≈ uploaded` keeps the balance near zero. The `max_safe_upload_bps` function in `src/swarm.rs` calculates how much upload is safe before hitting the threshold.

## Emulated clients

All peer_id formats, User-Agent headers, and query strings were verified against client source code as of 2025-2026. BitComet was removed (banned on most trackers); rTorrent and BitTorrent Mainline were added (on virtually every whitelist).

| Client | peer_id prefix | User-Agent | numwant |
|--------|---------------|------------|---------|
| qBittorrent 5.2.2 | `-qB5220-` | `qBittorrent/5.2.2` | 200 |
| Transmission 4.1.2 | `-TR4120-` | `Transmission/4.1.2` | 80 |
| Deluge 2.2.0 | `-DE220s-` | `Deluge/2.2.0 libtorrent/1.2.19.0` | 200 |
| µTorrent 3.5.5 | `-UT3550-` | `uTorrent/3550` | 50 |
| BitTorrent 7.11.0 | `-BT7B00-` | `BitTorrent/7.11.0` | 50 |
| rTorrent 0.9.8 | `-lt098-` | `rtorrent/0.9.8` | 80 |
| Vuze 5.7.5.0 | `-AZ5750-` | `Vuze 5.7.5.0;Windows 10;Java 1.8.0_301` | 50 |

Each client has its own query parameter ordering and client-specific fields (e.g. Vuze sends `azudp` and `azver=3`, qBittorrent sends `supportcrypto=1&redundant=0`). The `key` parameter is 8 uppercase hex digits, matching what libtorrent, Transmission, and µTorrent send.

## Configuration

### Transfer mode

| Mode | Description |
|------|-------------|
| **Download + upload** | Leech→seed lifecycle: starts as leecher, simulates download, sends `completed`, then seeds. Stealthier. |
| **Upload only** | Ghost seed: starts as seeder immediately. Faster but easier to detect. |

### Speed strategy

| Strategy | Description |
|----------|-------------|
| **Dynamic** (default) | Uses announce responses to calculate fair-share speed from seeder/leecher counts. Adapts as swarm changes. |
| **Fixed** | Manual upload speed (in KiB/s, MiB/s, etc.). |

### Dynamic mode parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| Swarm multiplier | 1.0 | 1.0 = match swarm average, 2.0 = upload twice as fast |
| Max upload cap | 0 (∞) | Hard cap on upload speed (0 = unlimited) |
| Max download cap | 0 (∞) | Hard cap on download speed (0 = unlimited) |

### Fixed mode parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| Upload speed | 512 KiB/s | Target upload speed |
| Download speed | 1 MiB/s | Simulated download speed (leech phase only) |

### General parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| Jitter ±% | 20 | Random speed variation per second |
| Ramp-up | 120s | Gradual speed increase at start |
| Start at % | 0 | Pretend we already downloaded this percentage |
| Pause upload when no leechers | On | Prevents impossible-upload detection |
| Pause download when no seeders | On | Prevents impossible-download detection |

All tunable values live in `config.toml` (project root) - the single source of truth. No defaults exist in Rust code. See `AGENTS.md` for the full architecture and the `data/` module's role as the non-config constant owner.

## Architecture

```
redswarm/
├── Cargo.toml                ← package manifest (edition 2024, Axum 0.8)
├── config.toml               ← single source of truth for all tunable values
├── build.sh                  ← frontend bundler (CSS inlined, JS content-hashed)
├── templates/
│   └── index.html            ← dashboard (Askama template; fragments rendered by src/render.rs)
├── frontend/                 ← zero-dependency frontend (inline CSS/JS, served via /static)
│   ├── js/                   ← ES modules (state, utils, components, services, app)
│   ├── styles/               ← CSS source (bundled + inlined into index.html at build time)
│   └── tests/                ← zero-dependency browser test harness (29 .test.js files)
├── scripts/                  ← verification + capture helpers (auto_verify.py, raw_capture.py, forwarder.cs)
└── src/
    ├── main.rs               ← Entry point: dual-stack bind, engine/db/NAT/peer-server/watcher setup
    ├── announce.rs           ← HTTP announce client (URL encoding, query templates, response parsing)
    ├── api.rs                ← REST + SSE handlers, Axum router, compression + Cache-Control middleware
    ├── bencode.rs            ← Bencode codec + info_hash computation + hex utils
    ├── capture.rs            ← Fingerprint capture (peer-id/client decoding, capture tracker server)
    ├── config.rs             ← config.toml loader + per-section validate() (no defaults in code)
    ├── data/                 ← Single source of truth for non-config constants
    │   ├── mod.rs            ← module wiring + cross-language sync enforcement tests
    │   ├── schema.rs         ← SQL table/column names + DDL
    │   ├── vocab.rs          ← status/phase/event/lifecycle controlled vocabulary
    │   ├── protocol.rs      ← BitTorrent protocol keys, lengths, paths, cache headers
    │   ├── sse.rs            ← SSE wire event names (13 event types)
    │   ├── units.rs          ← byte/duration/speed formatters
    │   ├── labels.rs         ← UI display labels (mirrored by frontend/js/data/labels.js)
    │   └── fixtures.rs       ← test fixtures (test-only)
    ├── db.rs                 ← SQLite persistence (tasks + events + schema migration)
    ├── engine.rs             ← Audit engine: probe → attack, jittered re-announce, SSE events
    ├── magnet.rs             ← Magnet link parser (hex + base32 info_hash)
    ├── nat.rs                ← NAT-PMP port mapping (gateway lease maintenance)
    ├── peer_id.rs            ← peer_id/key generation + client lookup (data-free; specs in config.toml)
    ├── peer_server.rs        ← Peer-wire server (handshake, bitfield, keepalive - no data serving)
    ├── reload.rs              ← Hot config reload (atomic swap of the live config)
    ├── render.rs             ← Server-side HTML fragments (task list, log, topbar, settings)
    ├── singleton.rs          ← Single-instance enforcement (pidfile + process takeover)
    ├── swarm.rs              ← Swarm dynamics: fair-share speed + balance-safe upload
    ├── templates.rs          ← Askama template structs (path = "index.html")
    ├── torrent.rs            ← .torrent file parser
    └── watcher.rs            ← config.toml filesystem watcher (debounced → reload)
```

### Tech stack

- **Rust** (edition 2024) - zero-GC, zero-cost abstractions
- **Tokio** - async runtime
- **Axum 0.8** - web framework with SSE support
- **reqwest** - HTTP client (rustls TLS, gzip)
- **sqlx** - async SQLite (WAL mode)
- **Askama** - compile-time HTML templates
- **tower-http** - gzip compression middleware
- **serde** - JSON serialization for API + SSE events

### Data flow

```
.torrent/magnet → parse → AuditConfig → engine::run()
                                        │
                     ┌───────────────────┤
                     ▼                    ▼
                broadcast channel    DB writer task
                     │                    │
                     ▼                    ▼
               global SSE endpoint   SQLite (events)
                     │
                     ▼
           EventSource (browser)
                     │
                     ▼
           ┌─────────┴──────────┐
           ▼                    ▼
    appendLogRow() +       task list diff
    updateLogStats()       (status/client/progress)
```

The engine emits `AuditEvent`s to a per-audit `tokio::broadcast` channel. The DB writer task persists them to SQLite and bridges them to a global `AppEvent` broadcast (`engine::AppEvent`). The global SSE endpoint stream carries 13 event types - `audit`, `task_created`, `task_deleted`, `task_status`, `task_client`, `task_progress`, `task_updated`, `config_reloaded`, `capture_progress`, `goal_progress`, `goal_created`, `goal_deleted`, `goal_updated` - to a single browser `EventSource`. The JS dispatcher routes each event type to its updater; updaters only touch DOM cells whose values actually changed - no `innerHTML` replacement, no polling.

### Performance

- Backend response time: ~1-2ms per request
- Compression: gzip (~75% bandwidth reduction)
- Live updates: single global SSE (sub-100ms event delivery, zero polling)
- Diff-based DOM updates: each SSE event only patches the specific cell(s) that changed
- Actions: Start/Stop/Delete send a POST and the global SSE event drives the UI update

## Technical details

For the full implementation reference - every stealth technique (jitter, ramp-up, bursty upload, freeze logic, fair-share speed, balance evasion), the peer-wire server, the goal feedback loop, crash recovery, hot-reload, NAT-PMP, singleton takeover, and the complete input-validation bounds table - see [`docs/TECHNICAL_DETAILS.md`](docs/TECHNICAL_DETAILS.md).

## API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Dashboard HTML (server-side rendered, full task list + log panel) |
| GET | `/api/bootstrap` | Initial app bootstrap JSON (task list, goals, counts) |
| GET | `/api/events` | Global SSE stream (13 event types; see [Data flow](#data-flow)) |
| GET | `/api/audits` | Task list (JSON) |
| GET | `/html/audits` | Task list (server-rendered HTML fragment) |
| POST | `/api/audits` | Create task (JSON body) |
| GET \| PUT \| DELETE | `/api/audits/{id}` | Get / update config / delete a task (JSON) |
| POST | `/api/audits/{id}/start` | Start task |
| POST | `/api/audits/{id}/stop` | Stop task |
| GET | `/api/audits/{id}/log` | Event log (JSON) |
| GET | `/html/audits/{id}/log` | Event log (server-rendered HTML fragment) |
| GET \| POST | `/api/goals` \| `/html/goals` | Goal list (JSON / HTML fragment) + create |
| GET \| PUT \| DELETE | `/api/goals/{id}` | Get / update / delete a goal |
| GET \| PUT | `/api/goals/{id}/tasks` | Get / set the task set bound to a goal |
| GET \| PUT | `/api/settings` | Get / update the full `AppConfig` (JSON) |
| GET | `/api/clients` | Emulated-client identities (prefix, display name) |
| POST | `/api/parse-torrent` | Parse .torrent file body → metadata JSON |
| POST | `/api/parse-magnet` | Parse magnet link string → metadata JSON |
| POST | `/api/capture/start` | Start a fingerprint capture session |
| GET \| DELETE | `/api/capture/{token}` | Get status / cancel a capture |
| GET | `/capture/{token}/announce` | Capture tracker announce endpoint |
| GET | `/capture/{token}/scrape` | Capture tracker scrape endpoint |
| GET | `/static/*` | Frontend assets (fingerprinted JS bundle, favicon) |

## Testing

```bash
cargo test          # 625 Rust tests
cargo clippy -- -W warnings   # zero warnings
./build.sh          # regenerate frontend bundle (run after CSS/JS edits)
```

625 Rust tests covering failure paths across all modules:

| Module | Tests | Coverage |
|--------|-------|----------|
| `config` | 106 | config.toml loader + per-section `validate()` failure paths (NaN/Infinity, bounds, duplicates, invalid values) |
| `capture` | 79 | peer-id prefix decoding (base62/hex), capture state machine, fingerprint reconstruction, capture tracker |
| `engine` | 63 | Leech/seed phases, ramp-up, bursty upload, counters never decrease, jitter bounds, config defaults |
| `bencode` | 52 | Empty input, invalid types, truncated data, dict key validation, info_hash, hex |
| `db` | 43 | Schema drift, migration from old schemas, full field round-trip, peer state persistence, event ordering |
| `data` | 39 | Raw literal detection, DDL drift, vocab/SSE/label const wiring, percent-encode dedup, bencode key centralization |
| `api` | 35 | REST + SSE handlers, per-route Cache-Control, HTML/JSON contract tests, settings validation |
| `swarm` | 34 | Fair-share formula, 0-leecher freeze, balance-safe upload, caps, dynamic download, validation |
| `announce` | 32 | Percent-encoding, compact/dict peer parsing, malformed responses, non-dict rejection, URL building |
| `templates` | 22 | EventView formatting, speed cell composition, log column visibility, DOM hook wiring |
| `peer_server` | 22 | Handshake constants, BEP-3 message IDs, fast/lt extensions, per-IP limits, security bounds |
| `render` | 20 | Topbar stats, log rows, settings fields, capture snippets (server-side HTML fragments) |
| `torrent` | 18 | Missing keys, non-dict input, single/multi-file, info_hash determinism |
| `magnet` | 15 | Missing fields, wrong hash lengths, invalid hex/base32, URL encoding, multiple trackers |
| `peer_id` | 14 | 20-byte length, prefix correctness, alphanumeric suffix, uniqueness, key format, distinct prefixes/UAs |
| `watcher` | 9 | fs-event kind/path matching, config-file detection, debounce |
| `singleton` | 7 | Pidfile single-instance, dead-pid detection, process takeover |
| `main` | 6 | Dual-stack bind behavior, port-in-use error paths |
| `reload` | 5 | Hot-reload atomic swap, reject-invalid-keeps-old, unchanged-config noop |
| `nat` | 4 | NAT-PMP mapping values, nonzero port rejection |

The frontend ships 218 browser tests across 29 files (zero-dependency harness at `frontend/tests/`). Run them by visiting `/static/tests/index.html` while the server is running.

## Research sources

All protocol details, client versions, and detection techniques were verified against primary sources:

- **Client source code**: qBittorrent (`sessionimpl.cpp`), Transmission (`announcer-http.cc`), Deluge (`core.py`), BiglyBT/Vuze (`Constants.java`), libtorrent (`http_tracker_connection.cpp`)
- **Tracker anti-cheat source**: Unit3D (`CheatedTorrentController.php`, `ProcessAnnounce.php`), Ocelot (`worker.cpp`), Gazelle (`schedule/index.php`)
- **Protocol specs**: BEP-3, BEP-15, BEP-48, theory.org BitTorrentSpecification
- **Existing tools**: RatioMaster.NET (`TorrentClientFactory.cs`), JOAL (`.client` profiles), webtorrent/bittorrent-peerid (`index.js`)
- **Swarm dynamics**: anacrolix/torrent choking/unchoking, BitTorrent BEP-3 choking algorithm constants, Ookla/HSI speed data

## Legal notice

This tool fakes BitTorrent ratio. Be aware of your tracker's rules and your local laws before using it. You are responsible for how you use it.

## User guide

For the full dashboard walkthrough - tasks, goals, fingerprint capture, every settings pane, client management, and the live-update/hot-reload/crash-recovery behavior - see [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md).

## Contributing

PRs are welcome. See [`CONTRIBUTING.md`](CONTRIBUTING.md) for build instructions, test/lint commands, code style, and the PR process. The project enforces zero clippy warnings and 100% test coverage for new code; see [`AGENTS.md`](AGENTS.md) for the full engineering standard.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
