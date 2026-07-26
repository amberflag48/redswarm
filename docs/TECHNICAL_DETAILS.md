# Technical details

Implementation reference for every stealth technique, subsystem, and invariant in RedSwarm. This documents *how the code works and why each mechanism evades detection*, sourced from the actual implementation. For the user-facing how-to see [USER_GUIDE.md](USER_GUIDE.md); for the protocol background see [BEP_EXTENSIONS_REFERENCE.md](../BEP_EXTENSIONS_REFERENCE.md); for the engineering standard see [AGENTS.md](../AGENTS.md).

All `file:line` references are against the project root. Config defaults are quoted from `config.toml`.

## Table of contents

- [Probe phase](#probe-phase)
- [Attack phase](#attack-phase)
- [Speed jitter](#speed-jitter)
- [Ramp-up](#ramp-up)
- [Bursty upload](#bursty-upload)
- [Freeze on 0 leechers](#freeze-on-0-leechers)
- [Freeze on 0 seeders](#freeze-on-0-seeders)
- [Dynamic upload speed (fair-share)](#dynamic-upload-speed-fair-share)
- [Dynamic download speed](#dynamic-download-speed)
- [Balance evasion](#balance-evasion)
- [Counters never decrease](#counters-never-decrease)
- [Announce timing jitter](#announce-timing-jitter)
- [Peer-server handshake](#peer-server-handshake)
- [Bitfield and have_all](#bitfield-and-have_all)
- [Keepalive](#keepalive)
- [Unchoke and no data serving](#unchoke-and-no-data-serving)
- [Per-IP connection limits](#per-ip-connection-limits)
- [Goal feedback loop](#goal-feedback-loop)
- [Crash recovery](#crash-recovery)
- [Hot-reload](#hot-reload)
- [NAT-PMP](#nat-pmp)
- [Singleton and process takeover](#singleton-and-process-takeover)
- [Input validation](#input-validation)

## Probe phase

Before the attack, the engine tries each emulated client with a `started` announce until the tracker accepts one. That client is then used for the whole session.

**Algorithm** (`src/engine.rs:629-748`):

1. Build an index vector `0..cfg.clients.len()` and Fisher-Yates shuffle it. Only the iteration order changes; each probe body still indexes `cfg.clients[i]`.
2. For each index, build a throwaway `PeerIdentity` (fresh `peer_id` from the client's prefix + fresh `key` from its `key_format`), construct an `AnnounceSession`, compute the probe `(downloaded, left)` from the mode, and send `Event::Started`.
3. Acceptance is `!resp.is_failure()`. On the first accepted client, set `working_client = Some(i)` and break.
4. Emit a `probe` `AuditEvent` for every attempt (accepted or rejected) carrying `working_client: working_client.map(|idx| cfg.clients[idx].peer_id_prefix.clone())`.
5. If all reject, `working_client` stays `None` and the run returns early.

**Skip paths:**

- **Forced client** - `RunOptions.known_client` is pre-resolved in `api::start_engine` from `config.forced_client` (matched via `peer_id::find_by_client`: prefix exact, alias case-insensitive). In `engine::run`, `working_client` is seeded from `opts.known_client` and the probe loop exits immediately. Because the probe event never runs, `start_engine` records the working client to the DB and emits `TaskClient` SSE itself (`src/api.rs:647-662`).
- **Resume** - the same `known_client` path: the persisted `working_client` prefix from a prior probe is resolved to an index and used as `known_client`, so no re-probe on restart.

**Config:** the `[[clients]]` array (peer_id_prefix, user_agent, query, numwant, reserved_bytes, fast_extension, key_format, ...). `AuditConfig.forced_client` (per-task, defaults to `None` = auto-probe). The stock config ships 7 clients.

**Evasion rationale:** a deterministic probe order is itself a fingerprint when two audits hit the same tracker; randomizing the order spreads first-contact across clients. Probing with a real `started` announce + real peer_id/UA/query shape means acceptance reflects the tracker's real per-client whitelist, not a guess.

## Attack phase

One `AnnounceSession` (one `peer_id` + `key`) runs the whole audit: `started` -> periodic regular announces -> one `completed` (leech to seed) -> `stopped` on exit.

**Identity** (`src/engine.rs:757-774`): if resuming, reuse the persisted `peer_id`/`key` (decoded from hex); else generate fresh from the working client's prefix + `key_format`. The `peer_id` is 20 bytes: prefix + random alphanumeric. The `key` is 8 chars: `LowerHex` -> `{:08x}`, `UpperHex` -> `{:08X}`, `Decimal` -> digits (`src/config.rs:308-318`).

**Lifecycle (leech to seed)** (`src/engine.rs:818-838, 979-985`): `lifecycle_phase` starts as `leech` (download+upload mode, `left > 0`) or `seed` (upload-only mode / `left == 0`). On the first announce where `lifecycle_phase == "leech" && state.left == 0 && !completed_sent`, set `completed_sent = true`, flip to `seed`, and send `Event::Completed`. Regular announces send `Event::None` (wire label `regular`). `stopped` is always sent on loop exit, then a final `save_peer_state`.

**Config:** `[tracker] default_interval_secs=1800, min_interval_secs=60, max_interval_secs=86400`; `[http] timeout_secs=60`; per-audit `mode` (default `download_and_upload`), `start_download_pct` (default `0`).

**Evasion rationale:** a real BitTorrent session is one peer_id from `started` to `stopped`; reusing the persisted peer_id across restarts means the tracker credits resumed cumulative counters to the same peer (a new random peer_id would reset the delta baseline to 0 and lose un-announced upload). The leech-to-seed `completed` event avoids the "ghost seeder that was never a leecher" fingerprint.

## Speed jitter

Random +/- `jitter_pct`% variation applied to the target speed each tick.

**Formula** (`src/engine.rs:1248`):

```
jitter = 1.0 + uniform[-jitter_pct, +jitter_pct] / 100
effective_bps = base_bps * ramp_factor * jitter
```

With the default `jitter_pct = 20`, the multiplier is in `[0.8, 1.2]`. It multiplies the effective speed: seed upload (`engine.rs:1290`); leech download (`engine.rs:1265`); leech upload (`engine.rs:1258, 1274`).

**Config:** per-audit `jitter_pct` (default `20` from `[defaults] jitter_pct = 20`). Validated `0..=100` at `src/engine.rs:496-501` and `src/config.rs:233`.

**Evasion rationale:** a constant exact speed is a telltale of automation; real connections fluctuate. Jitter is recomputed every tick (1s) so the trace is noisy, not band-limited.

## Ramp-up

Speed grows linearly from 0 to full target over `ramp_up_secs`.

**Formula** (`src/engine.rs:1241-1245`):

```
ramp_factor = (elapsed_secs / ramp_up_secs).clamp(0.0, 1.0)   // if ramp_up_secs > 0
ramp_factor = 1.0                                              // if ramp_up_secs == 0
```

`elapsed` is `start.elapsed()`, where `start = Instant::now() - Duration::from_secs(resume_elapsed)` (`src/engine.rs:795, 845`) - so resumed audits continue the ramp from their persisted elapsed, not from 0. At `elapsed >= ramp_up_secs` the clamp yields `1.0` (full speed) for the rest of the run.

**Config:** per-audit `ramp_up_secs` (default `120` from `[defaults] ramp_up_secs = 120`). Validated `1..=86_400` (24h) at `src/config.rs:234-235` (`SECS_PER_DAY = 86_400`).

**Evasion rationale:** a real client doesn't max out its link the instant a torrent starts; ramping masks the burst-on-start heuristic.

## Bursty upload

In seed phase, each tick rolls a random float; if below `burst_choke_probability`, that tick produces zero upload (simulating a choked state with no peer requests).

**Algorithm** (`src/engine.rs:1284-1288`):

```
if random() < burst_choke_probability:
    return  # choked this second - no upload growth
```

`state.uploaded` is left unchanged that tick (the `+=` line is skipped). Over time ~`(1 - p)` of ticks upload.

**Config:** `[engine] burst_choke_probability = 0.3`. Validated `0.0..=1.0` at `src/config.rs:145-148`.

**Evasion rationale:** seeders aren't saturated 100% of the time - they choke when no peer is interested/requesting. A perfectly continuous upload curve is anomalous; intermittent zeros mimic real unchoke dynamics.

## Freeze on 0 leechers

In seed phase, if `freeze_on_zero_leechers` and the last seen leecher count is 0, upload growth is suspended for that tick.

**Code** (`src/engine.rs:1280-1282`):

```
if config.freeze_on_zero_leechers and ctx.leecher_count == 0:
    return  # no upload growth this tick
```

`ctx.leecher_count` is `last_leecher_count`, updated only from successful announce responses. Returning early means `state.uploaded` is unchanged (not zeroed - the counter is monotonic).

**Config:** per-audit `freeze_on_zero_leechers` (default `true`).

**Evasion rationale:** "uploading to nobody" is the #1 detection heuristic (a seeder reporting uploaded growth while the swarm has zero leechers is physically impossible). Freezing makes the counter honest.

## Freeze on 0 seeders

In leech phase, if `freeze_on_zero_seeders` and the seeder count is 0, download growth is suspended; leech-phase upload may still proceed if there are leechers.

**Code** (`src/engine.rs:1255-1263`):

```
if config.freeze_on_zero_seeders and ctx.seeder_count == 0:
    if not (freeze_on_zero_leechers and ctx.leecher_count == 0):
        state.uploaded += up_speed * dt   # leech upload still allowed
    return  # download skipped
```

Download is skipped (early return before the download block), but leech-phase upload is still allowed when leechers exist.

**Config:** per-audit `freeze_on_zero_seeders` (default `true`).

**Evasion rationale:** downloading from a swarm with zero seeders is impossible (P2P-only swarms exist, but the default assumes seeder supply). Freezing keeps `downloaded`/`left` physically plausible.

## Dynamic upload speed (fair-share)

In `speed_mode = Dynamic`, the upload target is recomputed from the announce response's `complete`/`incomplete` counts each announce (and once at `started`).

**Formula** (`src/swarm.rs:103-136`):

```
if leechers == 0: 0
seeders = max(swarm.seeders, 1)
fair_share = (leechers * avg_leecher_download_bps * seed_share_factor) / seeders
target = fair_share * fair_share_multiplier
if max_upload_bps > 0: target = min(target, max_upload_bps)
```

Defense-in-depth: NaN/Infinity in `seed_share_factor`/`fair_share_multiplier` -> return 0 (`swarm.rs:116-118`). `leechers == 0` short-circuits to 0. Recomputed after `started` (`engine.rs:864`) and after each regular announce (`engine.rs:1002`), stored in `dynamic_target_bps` -> `base_upload_bps` (`engine.rs:1171-1173`).

**Config:** `[swarm_defaults] avg_leecher_download_bps = 3000000` (3 MB/s), `seed_share_factor = 0.8`, `fair_share_multiplier = 1.0`, `max_upload_bps = 0` (unlimited). Validated at `config.rs:274-289`: `avg_leecher_download_bps >= 1`, `seed_share_factor` in `(0.0, 1.0]` (0.0 rejected), `fair_share_multiplier >= 0.0`.

**Example:** with defaults, 20 leechers / 5 seeders -> `(20 * 3M * 0.8) / 5 = 9.6 MB/s`.

**Evasion rationale:** a seeder's real upload is bounded by leecher demand divided among seeders. Faking more than ~2x fair share is a statistical outlier caught by per-torrent balance checks.

## Dynamic download speed

In Dynamic + download+upload mode, the leech-phase download target is the seeder supply split among leechers.

**Formula** (`src/swarm.rs:143-172`):

```
if seeders == 0: 0
leechers = max(swarm.leechers, 1)
total_supply = seeders * avg_leecher_download_bps * seed_share_factor
per_leecher = total_supply / leechers
capped = min(per_leecher, avg_leecher_download_bps)
if max_download_bps > 0: capped = min(capped, max_download_bps)
```

Note the implicit hard cap at `avg_leecher_download_bps` (3 MB/s default) regardless of `max_download_bps` - a single leecher can't exceed typical client download speed.

**Config:** same `[swarm_defaults]` plus `max_download_bps = 0` (default unlimited).

**Evasion rationale:** a leecher's download is bounded by aggregate seeder upload capacity; capping at `avg_leecher_download_bps` avoids reporting an implausibly fast single-stream download.

## Balance evasion

Per-torrent balance checks (Unit3D, Ocelot/Gazelle) flag peers whose `uploaded - downloaded` exceeds a small fraction of torrent size. The evasion strategy has two layers:

**Layer 1 - download offset (structural):** in download+upload mode, reporting `downloaded ~= torrent_size` makes the net balance near zero, giving the full torrent size of safe upload headroom. The `max_safe_upload_bps` helper (`src/swarm.rs:181-193`, test-only) encodes the math:

```
safe_balance = torrent_size / 25          # 4% margin
current_balance = uploaded - downloaded   # can be negative (more headroom)
remaining = safe_balance - current_balance
if remaining <= 0: 0 else remaining
```

With the download-offset strategy, `current_balance` stays near 0, so the full `safe_balance` (4% of torrent size) is available; without it, only 4% total is safe.

**Layer 2 - fair-share speed (runtime):** the production engine does *not* clamp `uploaded` against a balance threshold at runtime. Instead, the fair-share formula (see [Dynamic upload speed](#dynamic-upload-speed-fair-share)) keeps the speed near the swarm average, so the cumulative balance grows in line with what a real seeder would report. Combined with the download offset, this keeps the net balance within the safe band.

**Evasion rationale:** a hard cumulative clamp would create a visible "speed drops to 0 at a threshold" discontinuity; the fair-share + offset strategy produces a smooth, swarm-consistent upload curve that stays within the safe band naturally.

## Counters never decrease

`uploaded` and `downloaded` only ever increase; `left` only ever decreases.

**Mechanism:** the engine never writes tracker-reported counter values back into `state`. After each announce it consumes only `resp.effective_interval()`, `resp.leechers`, `resp.seeders`, `resp.peer_count` (`src/engine.rs:853-856, 991-994`). The counters advance solely by:

- `state.uploaded += (effective_bps * dt) as u64`
- `state.downloaded += dl_bytes`
- `state.left = state.left.saturating_sub(dl_bytes)` (saturating, so it floors at 0 and never underflows)
- Consistency clamp on leech completion: `if state.left == 0 { state.downloaded = config.torrent_size; }`

There is no `state.uploaded = resp.x` anywhere, so a "lower tracker response" cannot overwrite the local counter - the invariant is enforced by construction, not by a `max(old, new)` guard. Tests: `uploaded_never_decreases`, `downloaded_never_decreases`, `left_never_increases_during_leech` (`src/engine.rs:1438-1474`).

**Evasion rationale:** trackers compute deltas between announces; a counter that goes backwards is either a bug or a cheat attempt and is a strong ban signal. Structural monotonicity makes regress impossible.

## Announce timing jitter

The next-announce instant is `interval +/- announce_jitter_pct%`, recomputed each announce.

**Formula** (`src/engine.rs:530-545`):

```
jitter_secs = int(interval * announce_jitter_pct / 100)
if jitter_secs <= 0: return interval
delta = uniform[-jitter_secs, +jitter_secs]
return max(1, interval + delta)
```

With the default `interval=1800, announce_jitter_pct=5.0` -> `jitter_secs=90`, result in `[1710, 1890]`. `announce_jitter_pct = 0.0` returns exactly `interval`. Floored at 1 second.

`interval` itself is `resp.effective_interval()` = `max(resp.interval, resp.min_interval)` when the tracker advertises a higher `min_interval` (`src/announce.rs:61-66`), clamped to `[min_interval_secs, max_interval_secs]` at parse (`src/announce.rs:228-232`).

**Config:** `[engine] announce_jitter_pct = 5.0` (validated `0.0..=100.0` at `config.rs:139-140`); `[tracker] default_interval_secs=1800, min_interval_secs=60, max_interval_secs=86400`.

**Evasion rationale:** metronomic announce timing (exactly every 1800s) is an automation fingerprint; real clients drift slightly. Jitter is recomputed each announce so it tracks tracker interval changes.

Note: this is distinct from [Speed jitter](#speed-jitter). Timing jitter uses `[engine] announce_jitter_pct` (default 5%); speed jitter uses per-audit `jitter_pct` (default 20%).

## Peer-server handshake

On inbound TCP, read exactly 68 bytes under `handshake_timeout`, validate, and echo our handshake.

**Sequence** (`src/peer_server.rs:323-430`):

1. `read_exact(68 bytes)` under `tokio::time::timeout(handshake_timeout)`. Timeout -> drop (slow-loris defense).
2. Validate `hs[0] == 19` and `hs[1..20] == "BitTorrent protocol"`; else silently drop.
3. Extract `peer_info_hash = hs[28..48]`, `peer_reserved = hs[20..28]`. Offsets: `RESERVED_OFFSET=20, INFO_HASH_OFFSET=28, PEER_ID_OFFSET=48, HANDSHAKE_LEN=68` (`src/data/protocol.rs:69, 131-135`).
4. Detect capability bits: `fast_ext = peer_reserved[7] & 0x04`, `ltep = peer_reserved[5] & 0x10` (masks `protocol.rs:103-115`).
5. Look up registration by `peer_info_hash`; unknown -> drop (or capture session path).
6. Build our handshake: `[19][pstr][reg.reserved][peer_info_hash][reg.peer_id]` and write it. `reg.peer_id` is the same peer_id advertised to the tracker (registered in `engine.rs:787-789`).

**Config:** `[peer_server] handshake_timeout_secs = 5`, `write_timeout_secs = 5` (validated `>= 1`). `reserved_bytes` per `[[clients]]` (hex, 8 bytes).

**Evasion rationale:** a connectable peer must answer the BT handshake with a matching info_hash and a peer_id identical to what the tracker advertised; a mismatched or missing handshake is a connectability-fail signal. Timeouts prevent slow-loris resource exhaustion.

## Bitfield and have_all

After the handshake, claim all pieces - via `have_all` (BEP-6) when both sides support Fast Extension, else a classic bitfield.

**Code** (`src/peer_server.rs:432-444`):

```
if capture_mode:
    if peer_supports_fast_ext: write MSG_HAVE_NONE else: write MSG_BITFIELD [0x00]
elif reg.fast_extension and peer_supports_fast_ext:
    write MSG_HAVE_ALL                    # seeder, has everything
else:
    write MSG_BITFIELD [0xFF]             # SEEDER_BITFIELD - all bits set
```

`SEEDER_BITFIELD = [0xFF]` (`protocol.rs:129`) - a single byte with all bits set (claims 8 pieces; sufficient to appear as a seeder since we serve no data). BEP-6 requires *both* peers to set the Fast Ext bit for Fast Extension to be active. `MSG_HAVE_ALL=14`, `MSG_HAVE_NONE=15`, `MSG_BITFIELD=5` (`protocol.rs:79, 84-86`).

**Config:** `[[clients]] fast_extension` (bool) + `reserved_bytes` (the Fast Ext bit `0x04` in byte 7 must agree with `fast_extension` - cross-checked at `config.rs:410-415`).

**Evasion rationale:** `have_all` is what real libtorrent/Transmission/uTorrent seeders send; a full bitfield with the right piece-count shape would require knowing the real piece count. `have_all` avoids that and matches each client's actual wire behavior.

## Keepalive

Periodic 4-byte zero message (`[0,0,0,0]`) to keep the connection alive; per-client interval.

**Code** (`src/peer_server.rs:478-692`):

```
keepalive_interval = tokio::interval(reg.keepalive)
keepalive_interval.set_missed_tick_behavior(Skip)
on tick:
    if first_tick: skip (don't fire immediately post-handshake)
    if last_recv.elapsed() > idle_timeout: drop
    write [0,0,0,0]
```

`KEEPALIVE_MSG = [0,0,0,0]` (`protocol.rs:125`). `reg.keepalive = Duration::from_secs(client.keepalive_secs)`. The first interval tick is skipped so the first keepalive fires after a full interval, not immediately post-handshake. Idle timeout drops the connection if no bytes are received for `idle_timeout_secs`.

**Config:** `[[clients]] keepalive_secs` (qBittorrent 90, Transmission 100, Deluge 90, uTorrent 120, BitTorrent 120, rTorrent 120, Vuze 120); `[peer_server] idle_timeout_secs = 240`, `write_timeout_secs = 5`, `capture_keepalive_secs = 90`. All validated `>= 1`.

**Evasion rationale:** each real client has a characteristic keepalive cadence; matching it (per-client `keepalive_secs`) makes the wire fingerprint consistent with the announced peer_id/UA. Skipping the immediate first tick matches real client behavior.

## Unchoke and no data serving

Send `unchoke` to the peer, then silently drain all messages and never send a piece. Piece-request messages from the peer are drained and ignored; a `piece` message from the peer (impossible for a seeder) causes an immediate connection drop.

**Code:**

- Unchoke: `write_msg(MSG_UNCHOKE, &[])` (`peer_server.rs:452-454`). (Capture mode sends `interested` instead.)
- Message loop reads a 4-byte big-endian length prefix + 1-byte id (`peer_server.rs:499-529`):
  - `len == 0` -> keepalive.
  - `len > MAX_PEER_MSG_LEN (65536)` -> drop.
  - `id == MSG_PIECE (7)` -> drop connection (a piece from the peer means the peer thinks *we* requested data - impossible for a seeder).
  - Otherwise drain `len-1` bytes into a fixed `DISCARD_BUF_LEN = 256` buffer, looping until consumed. The buffer never grows - bounded memory per connection.
- No `MSG_REQUEST` handling and no `MSG_PIECE` sending: piece requests are drained and ignored. We never emit a piece, so no hash-mismatch can ever occur -> no hash-mismatch ban.

**Config:** `MAX_PEER_MSG_LEN`/`DISCARD_BUF_LEN` are physical constants (`protocol.rs:120, 123`); timeouts via `[peer_server]`.

**Evasion rationale:** appearing connectable + unchoked passes Unit3D-style connectability probes; serving no data means a peer can never detect a bad piece, so the hash-mismatch ban path can't trigger. Silently draining (rather than rejecting) keeps the connection alive and looks like a slow/unlucky seeder.

## Per-IP connection limits

Cap concurrent connections per source IP; reject excess before allocating any state.

**Code** (`src/peer_server.rs:247-301`):

```
per_ip = HashMap<IpAddr, count>
on connect:
    if per_ip[ip] >= max_per_ip: reject
    per_ip[ip] += 1
    # IpGuard RAII decrements on drop
on semaphore-reject:
    per_ip[ip] -= 1
```

An `IpGuard` RAII struct decrements the counter on connection drop. A global `Semaphore(max_connections)` caps total concurrent connections; a failed `try_acquire_owned` decrements the per-IP counter and rejects. Accept errors back off `accept_error_backoff_ms`.

**Config:** `[peer_server] max_per_ip = 20`, `max_connections = 10000`, `accept_error_backoff_ms = 100` (validated `>= 1`).

**Evasion rationale:** a tracker/peer flooding the advertised port to probe or DoS the emulated peer can't exhaust resources; the per-IP cap prevents a single host from saturating the global semaphore.

## Goal feedback loop

When `goal.enabled && target_secs > 0` ("reverse mode"), each tick overrides the effective base speed so the remaining bytes land in the remaining time. Once all tracked targets are reached, `reached_action` decides the post-reach behavior.

**Required-speed formula** (`src/engine.rs:451-464`):

```
if target_secs == 0: 0          # forward/ETA mode - no override
remaining_bytes = target - current
if remaining_bytes == 0: 0      # target reached
remaining_secs = target_secs - elapsed_secs
if remaining_secs == 0: 0       # deadline passed
return ceil(remaining_bytes / remaining_secs)
```

**Per-tick override** (`src/engine.rs:1195-1238`):

- If `goal_reached(state, goal)` (all tracked targets met):
  - `Stop` | `ContinueInitial` -> leave base as-is (Stop breaks the loop after this tick; ContinueInitial resumes default speed).
  - `ContinueCustom` -> `base_upload_bps = goal.reached_bps` (capped by `max_upload_bps` if > 0; `0` freezes the counter).
- Else (not yet reached), per direction:
  - upload: if `up_t > 0 && state.uploaded < up_t` -> `req = goal_required_bps(...)`; if `req > 0`, `base_upload_bps = min(req, max_upload_bps)`.
  - download: same with `dl_t`/`state.downloaded`/`max_download_bps`.

The Stop break is placed after the announce block so a due `completed` lifecycle transition fires first.

**Config:** per-audit `goal` (`[defaults] goal_enabled=false, goal_direction="upload", goal_upload_target=0, goal_download_target=0, goal_target_secs=0, goal_reached_action="stop", goal_reached_bps=0`). Bounds: `goal_target_secs <= 31_536_600` (`GOAL_MAX_TIME_SECS`), targets `<= 1_099_511_627_776` (1 TiB, `GOAL_MAX_TARGET_BYTES`).

**Evasion rationale:** lets a user hit an exact ratio/amount by a deadline (e.g. "1 GiB uploaded within 1h") by dynamically adjusting the speed coefficient - accelerating when behind, coasting when ahead - bounded by `max_upload_bps` so the override can't push you into the detectable outlier zone.

## Crash recovery

Peer state is persisted to the `audits` row every stat tick and on every announce; on boot, audits still marked `running` are auto-restarted and resume from the saved counters without re-probing.

**Persisted fields** - `PeerStateRow` (`src/db.rs:205-214`): `uploaded, downloaded, left, lifecycle_phase, completed_sent, elapsed_secs, peer_id (hex), key`. Written via `save_peer_state` at: after `started`, each stat tick, each announce, and the final `stopped`.

**Cadence:** `stat_interval = stat_interval_secs` (default 5s).

**Resume path** (`src/engine.rs:547-573, 795-845`): peer_id hex decoded (invalid/truncated -> `None` -> fresh identity); `lifecycle_phase` empty -> defaults to `leech`; `start = Instant::now() - Duration::from_secs(resume_elapsed)` so elapsed/ramp continue.

**Boot auto-restart** (`src/main.rs:155-174`): `db::list_audits` filtered by `status == "running"`; for each, `api::start_engine`. `start_engine` loads `get_peer_state`; if any counter is nonzero it becomes `Some(resume)`, and `known_client` is resolved from the persisted `row.working_client` -> the probe is skipped. `start_seq` continues from `db::get_max_seq`. `reset_audit` (called on config edit of a running task) wipes counters/peer_id/key/working_client so the next start is a fresh probe.

**Config:** `[engine] stat_interval_secs = 5`, `tick_interval_secs = 1` (validated `>= 1`).

**Evasion rationale:** a crash/kill mid-seed shouldn't lose un-announced upload credit or force a re-probe (which would change the peer_id and reset the tracker's delta baseline). Persisting `peer_id` + `key` + `elapsed` + `lifecycle_phase` makes resume indistinguishable to the tracker from a brief network gap.

## Hot-reload

`config.toml` is re-read, validated, and the whole `AppConfig` is atomically swapped into `AppState`. Running audits are unaffected (they hold frozen `Arc` snapshots); only per-request handlers and newly-started audits see new values. Structural subsystems are re-applied as needed.

**Algorithm** (`src/reload.rs:48-138`):

1. `config::load_from_path(path)` - on parse/validate error, keep old config, return `Err`.
2. **No-op guard:** `if new == old { return }` - byte-identical content skips swap + re-apply + SSE (prevents double-toast when the settings API writes and the watcher fires on the same write).
3. `state.config.store(Arc::new(new))` - atomic `ArcSwap`.
4. **Log filter:** if `server.log_filter` changed, reload the tracing `EnvFilter` in place.
5. **DB pool:** if `server.db_url` or `database.max_connections` changed, connect a new pool and swap it in. The old pool drops once running audits end.
6. **Peer server:** if `peer_server.*` or `tracker.peer_port` changed:
   - Same port + running audits > 0 -> defer (can't rebind; log a warning).
   - Same port + no running audits -> swap (stop old -> bind new).
   - Port changed -> old and new coexist on different ports; bind new first, swap on success; old drops when the last audit holding it ends.
7. **NAT-PMP:** if `nat.gateway_ip` changed, or `peer_port` changed while NAT active -> cancel old lease, clear stale mapping, spawn a background `nat::resolve_and_maintain` (non-blocking so a slow gateway doesn't stall the reload).
8. **HTTP listener:** if `server.bind_addr` changed and no `REDSWARM_ADDR` env override -> signal a graceful rebind.
9. Broadcast `AppEvent::ConfigReloaded`.

**New-audits-only policy:** `start_engine` snapshots `cfg_snap`, `pool_snap`, `ps_snap` and passes them by `Arc` into `engine::run`. A running audit keeps its frozen snapshot for its whole lifetime.

**Config:** all of them (reload is global); `[watcher] debounce_ms = 300` (validated `1..=10_000`).

**Evasion rationale:** lets the user retune live (speeds, clients, ports, NAT) without restarting and dropping running audits, while guaranteeing a running audit's wire identity (peer_id, reserved bytes, port) can't change mid-flight (which would be a detection signal). Validate-before-swap means a malformed edit can't corrupt live state.

## NAT-PMP

When `[nat] gateway_ip` is set, query the gateway for its public IP and a UDP+TCP port mapping; announce the **public** port to the tracker while the peer-wire server keeps listening on the **internal** port (`[tracker] peer_port`).

**Algorithm** (`src/nat.rs:62-130`):

1. `internal = NonZeroU16::new(internal_port)` (0 -> error).
2. `natpmp::external_address(gateway)` -> public IPv4.
3. Request UDP and TCP mappings on `internal` with `external_port: None` (gateway chooses) and `lifetime_seconds: lease_lifetime_secs`.
4. `public_port = udp.external_port()`; if TCP differs, warn and use UDP.
5. Spawn a renew task: every `renew_interval_secs`, renew both mappings, observing a `CancellationToken`.

**Public-port override of `tracker.peer_port`:** at `start_engine`, `cfg_snap.tracker.peer_port = m.public_port` when NAT is active (`src/api.rs:581-585`). The peer server still binds the internal port. RFC 6886: the gateway translates inbound public-port traffic to the internal port. The override is computed at snapshot time (not by mutating stored config) so a hot-reload of config.toml (which holds the internal port) never clobbers the NAT override.

**Config:** `[nat] gateway_ip = ""` (empty = disabled), `lease_lifetime_secs = 60`, `renew_interval_secs = 45`. Validated: `lease >= 1`, `renew >= 1`, `renew < lease` (renew before lapse), `gateway_ip` parses as `IpAddr` if non-empty.

**Evasion rationale:** behind a NAT/VPN, the local port isn't reachable from the tracker's peers; without a public port the emulated peer is "non-connectable" - a strong detection signal. NAT-PMP makes the advertised port actually connectable.

## Singleton and process takeover

At startup, terminate any other running `redswarm` process so this one can bind HTTP + peer-wire ports cleanly. There is no pidfile; singleton enforcement scans `/proc/*/comm`.

**Algorithm** (`src/singleton.rs:43-161`):

- `PROC_NAME = env!("CARGO_PKG_NAME")` (`"redswarm"`); compile-time `assert!(PROC_NAME.len() <= 15)` because the kernel truncates `/proc/<pid>/comm` to 15 chars.
- `find_other_instances()`: iterate `/proc` entries, skip non-numeric names and `self_pid`, return PIDs where `is_our_process(pid)` is true.
- `is_our_process(pid)`: `read_to_string("/proc/{pid}/comm").trim() == PROC_NAME`.
- `terminate(pid)`:
  1. `kill(pid, SIGTERM)`. `ESRCH` (already gone) -> return.
  2. Poll `is_running(pid)` every 100 ms for up to `GRACE_SECS = 5`.
  3. If still running, **re-verify identity** (`is_our_process(pid)`) before escalating - a recycled PID is never force-killed.
  4. `kill(pid, SIGKILL)`; wait up to `KILL_WAIT_SECS = 2`.
- `is_running(pid)`: read the state char from `/proc/{pid}/stat` (char after the last `)` in the line, robust to spaces in `comm`); `None` or `'Z'` (zombie) -> treated as gone (zombies have released their ports/FDs).
- Called once before binding anything. No-op on non-Linux.

**Config:** none - `GRACE_SECS=5`, `KILL_WAIT_SECS=2`, `POLL_MILLIS=100` are compile-time constants.

**Evasion rationale:** a restart shouldn't fail with "Address already in use" and leave a zombie prior instance holding ports. `/proc/<pid>/comm` identity (re-verified before SIGKILL) avoids killing an unrelated process that recycled a PID; treating zombies as gone avoids waiting on an unreaped child.

## Input validation

All bounds below are enforced in `config.rs` (and mirrored in `engine.rs`/`swarm.rs` for hand-built configs). `AppConfig::validate()` runs every sub-validator plus cross-section invariants (`config.rs:572-593`): `clients` non-empty, every `peer_id_prefix` unique. On any failure `config::load()` returns `Err` and the app refuses to start; hot-reload rejects the file and keeps the old config.

| Section | Field | Bound | Source |
|---|---|---|---|
| `server` | `bind_addr` | non-empty, parses as `SocketAddr` | `config.rs:54-58` |
| `server` | `db_url`, `log_filter` | non-empty (trimmed) | `config.rs:59-60` |
| `server` | `rebind_retry_secs`, `sse_keepalive_secs` | `>= 1` | `config.rs:61-62` |
| `http` | `timeout_secs` | `>= 1` | `config.rs:78` |
| `tracker` | `peer_port` | `> 0` | `config.rs:100` |
| `tracker` | `min_interval_secs` | `>= 1` | `config.rs:101` |
| `tracker` | `default_interval_secs` | `>= min_interval_secs` | `config.rs:102-105` |
| `tracker` | `max_interval_secs` | `> min_interval_secs` | `config.rs:106-109` |
| `engine` | `tick_interval_secs`, `stat_interval_secs`, `stop_grace_secs` | `>= 1` | `config.rs:137-149` |
| `engine` | `announce_jitter_pct` | `0.0..=100.0` | `config.rs:139-140` |
| `engine` | `leech_upload_factor` | `0.0..=1.0` | `config.rs:141-144` |
| `engine` | `burst_choke_probability` | `0.0..=1.0` | `config.rs:145-148` |
| `database` | `max_connections` | `>= 1` | `config.rs:165` |
| `ui` | `event_log_limit` | `>= 1` | `config.rs:181` |
| `defaults` | `upload_bps`, `download_bps` | `>= 1` | `config.rs:231-232` |
| `defaults` | `jitter_pct` | `<= 100` (`PERCENT`) | `config.rs:233` |
| `defaults` | `ramp_up_secs` | `1..=86_400` (`SECS_PER_DAY`) | `config.rs:234-235` |
| `defaults` | `start_download_pct` | `<= 100` | `config.rs:236` |
| `defaults` | `goal_upload_target`, `goal_download_target` | `<= 1 TiB` (`GOAL_MAX_TARGET_BYTES`) | `config.rs:237-246` |
| `defaults` | `goal_target_secs` | `<= 31_536_600` (`GOAL_MAX_TIME_SECS`) | `config.rs:247-251` |
| `swarm_defaults` | `avg_leecher_download_bps` | `>= 1` | `config.rs:275-278` |
| `swarm_defaults` | `seed_share_factor` | `(0.0, 1.0]` (0.0 rejected) | `config.rs:279-283` |
| `swarm_defaults` | `fair_share_multiplier` | `>= 0.0` | `config.rs:284-287` |
| `peer_server` | `max_connections`, `max_per_ip`, all `*_timeout_secs`, `accept_error_backoff_ms`, `capture_keepalive_secs` | `>= 1` | `config.rs:471-478` |
| `nat` | `lease_lifetime_secs`, `renew_interval_secs` | `>= 1` | `config.rs:509-510` |
| `nat` | `renew_interval_secs < lease_lifetime_secs` | renew before lapse | `config.rs:511-514` |
| `nat` | `gateway_ip` | parses as `IpAddr` if non-empty | `config.rs:518-521` |
| `watcher` | `debounce_ms` | `1..=10_000` (`DEBOUNCE_MS_MAX`) | `config.rs:541-547` |
| `clients[]` | `label`, `version`, `peer_id_prefix`, `user_agent`, `query`, `v_string` | non-empty (trimmed) | `config.rs:394-417` |
| `clients[]` | `peer_id_prefix` | `<= 20` chars (`PEER_ID_PREFIX_MAX_LEN`) | `config.rs:397-401` |
| `clients[]` | `query` | must contain `{info_hash}` and `{peer_id}` | `config.rs:404-405` |
| `clients[]` | `numwant` | `> 0` | `config.rs:406` |
| `clients[]` | `reserved_bytes` | hex, decodes to exactly 8 bytes (`RESERVED_LEN`) | `config.rs:407-409` |
| `clients[]` | `fast_extension` | must match the Fast Ext bit (`0x04` in byte 7) of `reserved_bytes` | `config.rs:410-415` |
| `clients[]` | `keepalive_secs` | `>= 1` | `config.rs:416` |
| `clients[]` | `reqq` (if present) | `> 0` | `config.rs:418-419` |
| `clients[]` | `m_dict` values | `> 0` | `config.rs:421-423` |
| `clients[]` | `send_complete_ago` (if present) | `>= -1` | `config.rs:424-425` |
| `AppConfig` | `clients` | non-empty; unique `peer_id_prefix` | `config.rs:584-591` |

Mirrored validators for runtime-constructed configs: `AuditConfig::validate` (`engine.rs:492-512`), `SwarmConfig::validate` (`swarm.rs:65-96`), `GoalConfig::validate` (`engine.rs:380-400`).

**Rationale:** bounds prevent both misconfiguration (zero port, zero interval -> hot loop / ban) and abuse vectors (a `query` template without `{info_hash}` would announce garbage; a `seed_share_factor` of 0.0 would zero all upload and look like a broken seeder; a `peer_id_prefix` longer than 20 bytes would overflow the peer_id). The `fast_extension` <-> reserved-bit cross-check prevents advertising a Fast-Ext client that then sends a classic bitfield (a wire inconsistency trackers can flag). `max_per_ip`/timeouts are DoS hardening. No defaults exist in Rust code - a missing/invalid value is a hard start failure.
