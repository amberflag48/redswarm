# User guide

How to run and use every feature of the RedSwarm dashboard. This is a how-to and reference doc for users; for the project overview and quickstart see the [README](../README.md), for the engineering standard see [AGENTS.md](../AGENTS.md), for the REST/SSE API see the README's [API section](../README.md#api).

## Table of contents

- [Starting the server](#starting-the-server)
- [The dashboard](#the-dashboard)
- [Tasks](#tasks)
  - [Creating a task](#creating-a-task)
  - [Editing a task](#editing-a-task)
  - [Starting and stopping](#starting-and-stopping)
  - [Deleting a task](#deleting-a-task)
  - [Per-task configuration reference](#per-task-configuration-reference)
- [Goals](#goals)
  - [Creating a goal](#creating-a-goal)
  - [Binding tasks to a goal](#binding-tasks-to-a-goal)
  - [Goal reached actions](#goal-reached-actions)
- [Fingerprint capture](#fingerprint-capture)
- [Settings](#settings)
  - [Server](#server)
  - [Tracker](#tracker)
  - [Engine](#engine)
  - [Defaults](#defaults)
  - [Swarm](#swarm)
  - [Peer server](#peer-server)
  - [Clients](#clients)
- [Client emulation specs](#client-emulation-specs)
  - [Adding a client manually](#adding-a-client-manually)
  - [Editing a client](#editing-a-client)
  - [Removing a client](#removing-a-client)
- [Live updates](#live-updates)
- [Hot-reload](#hot-reload)
- [Crash recovery](#crash-recovery)

## Starting the server

```bash
cargo run --release
```

The server reads `config.toml` from the working directory (override with `REDSWARM_CONFIG`). It binds to `server.bind_addr` (default `0.0.0.0:3000`) and opens the dashboard at that address.

| Variable | Default | Effect |
|---|---|---|
| `REDSWARM_CONFIG` | `config.toml` | Path to the config file |
| `REDSWARM_ADDR` | (none) | Overrides `server.bind_addr` at startup; if set, runtime `bind_addr` changes via the settings UI are suppressed |
| `REDSWARM_DB` | (none) | Overrides `server.db_url` at startup |
| `RUST_LOG` | `redswarm=info` | Overrides `server.log_filter` at startup |

The first build also generates the frontend bundle. If you edit frontend CSS or JS, regenerate it with `./build.sh` before restarting.

## The dashboard

Open `http://YOUR_IP:3000` from any machine on your network. The page is server-side rendered on first paint (no JS hydration flash, no layout shift) and then driven by a single global SSE stream - no polling.

The dashboard has three regions:

- **Topbar** - the RedSwarm logo, a connection badge (`Live` green when the SSE stream is open, `Reconnecting` amber while the browser auto-reconnects), running/stopped task counts, and one ETA tile per enabled global goal. The **Settings** button opens the settings modal; **New task** (or **New goal** when the Goals tab is active) opens the creation modal.
- **Tasks/Goals card** - a segmented tab switcher between the Tasks table and the Goals table.
- **Live log card** - the per-task log panel (audit info, stats strip, event table). Click any task row to load its log.

## Tasks

The task list is a 10-column table: Name, Tracker, Client, Mode, Strategy, Uploaded, Downloaded, Status, Created, and per-row Actions. On narrow viewports columns hide and the table restacks into labeled cards. Each row carries Edit + (Stop or Start) + Delete buttons.

A task is one emulated peer faking ratio on one torrent. Multiple torrents can share the same configuration by creating them as a batch (paste multiple magnets or drop multiple `.torrent` files).

### Creating a task

1. Click **New task** (topbar).
2. In the modal, drop one or more `.torrent` files onto the dropzone (or click it to browse), or switch to the **Magnet** tab and paste magnet links (one per line). Each input is parsed by the backend; a preview shows the name, tracker, hash, and size for each torrent. Duplicates (same info hash + announce URL as an existing task or another entry in the same batch) are flagged red and block submission.
3. Configure the attack parameters (see [Per-task configuration reference](#per-task-configuration-reference)). Defaults come from `[defaults]` and `[swarm_defaults]` in `config.toml`, editable in Settings.
4. Click **Start task**. Each torrent creates a task (sharing the configuration) and immediately starts. A single torrent switches the log panel to it; multiple show a "Started N tasks" toast.

### Editing a task

Click **Edit** on a task row. The torrent identity (announce URL, info hash, size) is locked - it cannot be changed. Only the configuration is editable. The **Save changes** button stays disabled until the form diverges from the stored snapshot. On save:

- If the config is unchanged, it's a no-op.
- If the config changed, the task is stopped (if running), the event log and peer state are wiped (counters, peer id, key, working client), the new config is persisted, and the task restarts only if it was running. This is equivalent to a fresh start with a new peer id and full re-probe.

### Starting and stopping

- **Start** (on a stopped task) - sends `POST /api/audits/{id}/start`. If a `forced_client` is set or a `working_client` is stored from a previous run, probing is skipped and the stored client is reused. Otherwise the engine probes every configured client in random order and uses the first the tracker accepts.
- **Stop** (on a running task) - sends `POST /api/audits/{id}/stop`. The engine sends a `stopped` announce, drains the DB writer, flips the status, and the event log is preserved (not cleared).

### Deleting a task

Click **Delete** and confirm. The task is stopped first (if running), then the row and its events are removed. This cannot be undone.

### Per-task configuration reference

Every field is pre-filled from `[defaults]`/`[swarm_defaults]`; override per task in the create/edit modal.

| Field | Options / range | Notes |
|---|---|---|
| Client emulation | Auto (probe all) or a specific client | `Auto` probes every client in random order; a specific client skips probing. |
| Speed strategy | Dynamic / Fixed | Dynamic calculates speed from swarm counts; Fixed uses a manual upload speed. |
| Transfer mode | Download + Upload / Upload only | Download+Upload simulates a leech then seed lifecycle; Upload only starts as a seeder immediately. |
| Upload speed | number + unit (B/KiB/MiB/GiB) | Fixed mode only. |
| Download speed | number + unit | Fixed mode + Download+Upload mode only. |
| Jitter | 0-100% | Random speed variation per second. Constant speed is a detection tell. |
| Ramp-up | 0-86400s | Speed grows from 0 to target over this duration. |
| Start at % | 0-100 | Download+Upload mode only. Pretend the download already reached this percentage (0 = from scratch, 100 = start as seeder). |
| Swarm multiplier | 0.1-5.0 | Dynamic mode only. 1.0 = match the swarm average, 2.0 = upload twice as fast. |
| Max upload | number + unit, 0 = unlimited | Dynamic mode only. Hard cap on upload speed. |
| Max download | number + unit, 0 = unlimited | Dynamic mode only. Hard cap on download speed. |
| Pause upload when no leechers | on/off | Stops upload growth when 0 leechers (uploading to nobody is the #1 detection vector). Hidden in Upload only mode. |
| Pause download when no seeders | on/off | Stops download growth when 0 seeders (downloading from nobody is impossible). Hidden in Upload only mode. |
| Enable goal | on/off | Reveals the per-task goal block (see [Goals](#goals)). |

Per-task goals reuse the same fields as standalone goals ([goal reference](#creating-a-goal)), scoped to that single task.

## Goals

A goal tracks the aggregate progress of one or more tasks toward a target (upload bytes, download bytes, a time deadline, or any combination) and can take an action when reached. Each enabled goal shows a tile in the topbar with its ETA and name.

### Creating a goal

1. Switch to the **Goals** tab and click **New goal**.
2. Fill in the form:

| Field | Options / range | Notes |
|---|---|---|
| Name | text, non-empty | Required. |
| Enabled | on/off | When off the goal stops tracking (no topbar tile, no progress events). |
| Direction | Upload / Download + Upload | Download+Upload reveals the download-target field. |
| Upload target | number + unit (MiB) | Cumulative upload bytes to reach. Required > 0 (for Upload direction) or both targets > 0 (for DU) unless a time is set. Max 1 TiB. |
| Download target | number + unit | DU direction only. Max 1 TiB. |
| Time | 0-31536600s | Deadline measured from goal start. 0 = ETA only (no speed adjustment). The goal is reached when the target is hit OR the time expires, whichever is first. |
| On goal reached | Stop / Continue (initial speed) / Continue (custom speed) | See [Goal reached actions](#goal-reached-actions). |
| Reached speed | number + unit (KiB) | Continue (custom speed) only. 0 = freeze the counter. |

3. Select the bound tasks (see [Binding tasks to a goal](#binding-tasks-to-a-goal)).
4. Click **Create goal**. The goal appears in the Goals table and (if enabled) as a topbar tile.

### Binding tasks to a goal

The Associated tasks picker lists every existing task with a status dot. A task can be bound to at most one goal - tasks already bound to another goal are disabled (grayed out, line-through). The picker has a filter input (searches task names), an **All** button (selects all visible, non-disabled tasks), and a **None** button (deselects all visible, non-disabled tasks). At least one task must be selected to create the goal.

The goal's progress is the sum of the counters and speeds across all bound running tasks, recomputed on every audit tick. The binding ETA shown in the topbar tile is the minimum of the target-based ETA and the deadline countdown.

### Goal reached actions

- **Stop** - the bound tasks are stopped when the goal is reached.
- **Continue (initial speed)** - the bound tasks keep running at the speed they were using when the goal was reached.
- **Continue (custom speed)** - the bound tasks switch to the **Reached speed**. Setting it to 0 freezes the counter (the task stops growing but keeps announcing).

## Fingerprint capture

Capture extracts the exact fingerprint of a real BitTorrent client so you can add it as an emulated client spec. The flow runs entirely in the browser; no manual packet inspection is needed.

1. Open **Settings** and go to the **Clients** pane. Click **Auto capture**.
2. The app mints a token, generates a dummy `.torrent` (1 MiB, 16 KiB pieces, dummy hashes) with the app's own capture tracker URL, and downloads it to your machine. Add this `.torrent` to your real BitTorrent client.
3. Your client announces to the capture tracker. The app records the `peer_id`, `User-Agent`, `numwant`, query-param order, and all HTTP headers.
4. The capture tracker responds with the app's IP and peer port as the only peer, so your client connects to the peer server. The app records the 8 reserved bytes and the peer id (cross-checked against the announce).
5. The client sends the BEP-10 LTEP extension handshake. The app records the `v` string, `m` dict, `reqq`, encryption, upload only, complete ago, yourip, listen port, metadata size, ipv4/ipv6, and share mode.
6. Keepalive measurement runs for about 2 minutes (needs ~2 keepalives). The progress bar shows "measuring..." until done.
7. Progress arrives live via SSE (no polling). The three-segment bar (Announce, Handshake, Ext Handshake) advances pending to active to done, and a collapsible "Raw fields" list shows all 22 captured fields. Omitted fields show "not sent"; keepalive shows "not measured (connection too short)" if the connection was too brief.
8. When capture completes, an editable client card is built from the fingerprint (using `KEEPALIVE_DEFAULT = 90` if not measured). Review and edit the fields, then either:
   - **Copy TOML** - copies a `[[clients]]` block to the clipboard.
   - **Add client** - adds it to the config. If the `peer_id_prefix` already exists, you're asked whether to overwrite (same version) or replace (different version with a newer/older hint). If only the label matches (different prefix), you can add anyway (with a warning) or replace. On confirm, the config is written and hot-reloaded.

Closing the modal or starting a new capture aborts the server-side session (`DELETE /api/capture/{token}`). Unsaved edits prompt a discard confirm.

## Settings

Open with the **Settings** button. The modal has a left nav with seven panes and a footer with **Save** (disabled until the form is dirty) and **Cancel**. Saving validates the full `AppConfig`, writes `config.toml`, and hot-reloads. Validation failures highlight the offending field. Every field below is also editable directly in `config.toml`; the file watcher hot-reloads either path.

### Server

Server, HTTP, database, UI, NAT, and watcher settings in one pane.

| Field | Range / notes |
|---|---|
| Bind address | Valid `host:port`. Changing triggers an HTTP rebind (unless `REDSWARM_ADDR` is set). |
| Database URL | SQLx connection string. Changing recreates the pool (the old pool drops when running audits end). |
| Log filter | tracing env-filter (e.g. `redswarm=info`, `redswarm=debug`). Reloaded in place. |
| Rebind retry (s) | >= 1. Seconds between rebind attempts if the port is in use. |
| SSE keepalive (s) | >= 1. Keep-alive comment frames on `/api/events`. |
| HTTP timeout (s) | >= 1. Tracker HTTP timeout. |
| DB max connections | >= 1. |
| Event log limit | >= 1. Max events stored per audit (the DOM is pruned to this). |
| NAT gateway IP | Empty = NAT-PMP disabled. A valid IP enables it. |
| Config watcher debounce (ms) | 1-10000. Coalesces a burst of config writes into one reload. |

### Tracker

| Field | Range / notes |
|---|---|
| Peer port | 1-65535. Port advertised in announces (overridden by the NAT public port if active). |
| Default interval (s) | >= 1, >= min interval. Used when the tracker doesn't return one. |
| Min interval (s) | >= 1. Floor for announce spacing. |
| Max interval (s) | > min interval. Clamps tracker-returned values. |

### Engine

| Field | Range / notes |
|---|---|
| Tick interval (s) | >= 1. The engine loop tick (speed recalculation, freeze checks). |
| Stat interval (s) | >= 1. How often peer state is persisted to SQLite (for crash recovery). |
| Announce jitter % | 0-100. Random skew on announce intervals (metronomic timing is a fingerprint). |
| Leech upload factor | 0-1. Upload fraction while leeching. |
| Burst choke probability | 0-1. Chance of choking per tick in burst mode (simulates no-requests). |
| Stop grace (s) | >= 1. Grace period before force-stopping on edit/delete. |

### Defaults

The per-new-task defaults. Every field here is pre-filled into the create-task modal and is documented in [Per-task configuration reference](#per-task-configuration-reference). Includes mode, speed mode, upload/download speeds, jitter, ramp-up, start download %, both freeze toggles, and the full goal block (enabled, direction, targets, time, reached action, reached speed).

### Swarm

These shape the dynamic speed calculation. `avg_leecher_download_bps` and `seed_share_factor` are settings-only (not per-task); the other three are also per-task overrides.

| Field | Range / notes |
|---|---|
| Avg leecher download (B/s) | >= 1. Typical private-tracker leecher download speed. |
| Seed share factor | (0.0, 1.0]. The fraction of demand seeders meet (P2P covers the rest). |
| Fair share multiplier | >= 0. Multiplier on the calculated fair-share speed. |
| Max upload (B/s) | >= 0. 0 = unlimited. Global cap (overridable per task). |
| Max download (B/s) | >= 0. 0 = unlimited. Global cap (overridable per task). |

### Peer server

The connectable peer-wire server. Accepting inbound connections is what makes the emulated peer "connectable" (a real-tracker check).

| Field | Range / notes |
|---|---|
| Enabled | on/off. Accept incoming peer-wire connections. |
| Max connections | >= 1. Global concurrent peer limit. |
| Max per IP | >= 1. DoS flood defense. |
| Handshake timeout (s) | >= 1. Drop peers that don't complete the BT handshake in time. |
| Write timeout (s) | >= 1. |
| Idle timeout (s) | >= 1. Drop peers that send no keepalives. |
| Body read timeout (s) | >= 1. |
| Accept error backoff (ms) | >= 1. Backoff after an accept error storm. |
| Capture keepalive (s) | >= 1. Keepalive interval during fingerprint capture. |

### Clients

The list of emulated client specs. At least one is required; labels must be unique. The pane header has **Add manually** and **Auto capture** (see [Fingerprint capture](#fingerprint-capture)) buttons. Each client is an editable card with four sections (see [Client emulation specs](#client-emulation-specs)).

## Client emulation specs

Each client card has 18 fields across four sections. The header shows the display name and a **Remove** button. The card is collapsible (chevron toggle).

### Adding a client manually

Click **Add manually** in Settings -> Clients. A card with default values is prepended and expanded. Fill in the fields (see below) and Save.

### Editing a client

Expand the card, edit any field, and Save. Validation runs on save (see the field notes below for the constraints).

### Removing a client

Click **Remove** on the card header and confirm. The client is spliced out of the config. You cannot remove the last client (at least one is required).

### Identity

| Field | Required | Notes |
|---|---|---|
| Label | yes | e.g. `qBittorrent`. |
| Version | yes | e.g. `5.2.2`. |
| Peer ID prefix | yes | The 8-char Azureus-style prefix (e.g. `-qB5220-`). This is the unique identity key; max 20 chars; must be unique across clients. |
| User-Agent | yes | The HTTP User-Agent sent in announces. |
| v_string | yes | The BEP-10 LTEP handshake `v` field. |
| Aliases | no | One per line. Alternative names for client matching. |

### Tracker announce

| Field | Required | Notes |
|---|---|---|
| Query template | yes | Must contain `{info_hash}` and `{peer_id}`. URL query params in the client's real order. |
| Numwant | yes | >= 1. Peers requested per announce. |
| Key format | yes | `lower_hex` / `upper_hex` / `decimal`. The 8-char `key` param format. |

### Peer wire

| Field | Required | Notes |
|---|---|---|
| Reserved bytes (hex) | yes | 16 hex chars (8 bytes). Sets the capability bits. |
| Keepalive (s) | yes | >= 1. |
| reqq | no | >= 1, blank = omit. Max outstanding block requests. |
| Fast extension | yes | Must match the reserved byte 7 bit 0x04 (validated). |
| m_dict | no | `key=value` per line. BEP-10 extension message IDs; values > 0. |

### BEP-10 ext handshake fields

| Field | Notes |
|---|---|
| Send upload_only | on/off. BEP-21 partial-seed flag. |
| Send yourip | on/off. Peer IP as seen by us. |
| Encryption (e) | not sent / true / false. |
| complete_ago | Seconds since completed. Blank = omit, -1 = never, else >= -1. |

## Live updates

The dashboard uses a single global `EventSource('/api/events')` connection. The 13 event types (defined in `src/data/sse.rs`) are:

| Event | Effect |
|---|---|
| `audit` | Prepends a log row to the active task's event table and patches the stats tiles. |
| `task_created` | Inserts a server-rendered row and adjusts the counts. |
| `task_deleted` | Removes the row and clears the log if it was active. |
| `task_status` | Updates the status badge, the action buttons, and the counts. |
| `task_client` | Updates the Client cell in both the task list and the log panel (set directly when a forced client skips probing). |
| `task_progress` | Updates the Uploaded/Downloaded cells with a flash. |
| `task_updated` | Updates the Mode/Strategy cells and the goal attributes. |
| `config_reloaded` | Refreshes the cached settings, runtime tunables, and client dropdown; refreshes the settings modal if open and clean. |
| `capture_progress` | Advances the capture progress bar and the raw-fields list. |
| `goal_progress` | Patches the topbar goal tile ETA. |
| `goal_created` | Refreshes the goals table and the topbar tiles. |
| `goal_deleted` | Removes the topbar tile and refreshes the goals table. |
| `goal_updated` | Refreshes the goals table and the topbar tiles. |

Each handler patches only the specific DOM cells that changed - no `innerHTML` replacement, no full re-render. On reconnect, the badge flips to `Live` and the full state is reconciled (task list, goals, log panel, client dropdown).

## Hot-reload

`config.toml` is watched by a filesystem watcher. Edits via the Settings UI write the file then hot-reload; direct edits to the file are debounced (default 300ms) then hot-reloaded. Either path triggers the same reload:

1. The config is re-loaded and validated. On a validation failure the old config is kept and a warning is logged (no crash).
2. Byte-identical configs are a no-op (the file watcher's reload after a settings save is suppressed by this guard).
3. The live config is atomically swapped. Per-request handlers and new audits pick it up immediately.
4. Structural changes are re-applied in place: log filter, DB pool (recreated; old drops when running audits end), peer-wire server (restarted unless running audits hold the port), NAT-PMP, and the HTTP listener (rebind, unless `REDSWARM_ADDR` is set).
5. A `config_reloaded` SSE event is broadcast with the full new config.

**Running audits are unaffected** - each is frozen on the config, pool, and peer-server snapshot taken at start time. Only new audits use the reloaded config.

## Crash recovery

Peer state (uploaded, downloaded, left, peer id, key, working client, lifecycle phase, elapsed time) is persisted to SQLite every `engine.stat_interval_secs` (default 5s). On boot, any task still marked `running` is automatically restarted by `start_engine`, resuming from the persisted state - no lost progress, no re-probing for the working client. The restart is logged as "auto-restarting audit on boot".
