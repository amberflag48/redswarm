//! Audit engine - one unified connection to the tracker that fakes ratio
//! using all stealth techniques combined.
//!
//! Flow:
//! 1. Probe: try each emulated client with a `started` announce. First one
//!    the tracker accepts → working client.
//! 2. Attack: a single announce session (one peer_id) that grows `uploaded`
//!    over time. Stealth is built in: realistic speed with jitter, proper
//!    event sequencing, respect for the tracker's interval, a real client's
//!    peer_id/UA/query shape. Reports everything back in real time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::announce::{AnnounceSession, Event, IntervalBounds, PeerIdentity, PeerState};
use crate::bencode;
use crate::data::{protocol, vocab};
use crate::db;

// Events

/// One event in the audit timeline, streamed to the UI and persisted.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEvent {
 pub audit_id: i64,
 pub seq: u64,
 pub timestamp: chrono::DateTime<chrono::Utc>,
 pub phase: &'static str, // "probe" | "attack"
 pub client: String,
 pub event: &'static str, // "probe" | "started" | "regular" | "stopped" | "completed" | "tick"
 pub uploaded: u64,
 pub downloaded: u64,
 pub left: u64,
 pub success: bool,
 pub failure_reason: Option<String>,
 pub interval: u32,
 pub seeders: i64,
 pub leechers: i64,
 pub peer_count: usize,
 pub latency_ms: u64,
 pub working_client: Option<String>,
 /// Fair-share speed calculated from swarm data (bytes/sec). Only set in dynamic mode.
 pub fair_share_bps: u64,
 /// Current dynamic target speed (bytes/sec). Only set in dynamic mode.
 pub dynamic_target_bps: u64,
 /// Seconds remaining until the next tracker announce. Computed at each
 /// stat tick from the scheduled `next_announce` instant; 0 on probe,
 /// started, stopped, and error events (no countdown is meaningful).
 pub next_announce_in_secs: u64,
 /// Seconds since the task started (engine `start` instant). 0 on probe
 /// and error events; updated at each tick. Used for time-only goal
 /// deadline countdowns.
 pub elapsed_secs: u64,
}

/// Summary of a task for the task list - sent over global SSE when a task is
/// created or when its row-level data changes (status, client, progress).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskSummary {
 pub id: i64,
 pub name: String,
 pub tracker: String,
 pub announce_url: String,
 pub info_hash: String,
 pub working_client: Option<String>,
 pub status: String,
 pub created_at: String,
 pub uploaded: u64,
 pub downloaded: u64,
 pub mode: String,
 pub strategy: String,
 /// The task's goal config - flows to the UI via `task_created` /
 /// `task_updated` SSE and `/api/audits` so the topbar + log panel can
 /// compute ETA/progress without an extra fetch.
 pub goal: GoalConfig,
}

/// Global event stream - one SSE connection drives all dynamic UI updates.
/// Each variant maps to a distinct SSE event name on the wire; the SSE
/// endpoint serializes the inner data (not the enum wrapper) so the JS
/// receives flat JSON fields it can destructure directly.
#[derive(Debug, Clone)]
pub enum AppEvent {
 /// `audit` - a timeline event for a specific task's log panel.
 Audit(AuditEvent),
 /// `task_created` - a new task appeared; carries the full row data.
 TaskCreated { task: TaskSummary },
 /// `task_deleted` - a task was removed.
 TaskDeleted { id: i64 },
 /// `task_status` - a task's running/stopped status changed.
 TaskStatus { id: i64, status: String },
 /// `task_client` - a task's working client was detected (or cleared).
 TaskClient { id: i64, working_client: Option<String> },
 /// `task_progress` - a task's uploaded/downloaded counters changed.
 TaskProgress { id: i64, uploaded: u64, downloaded: u64 },
 /// `task_updated` - a task's config was edited; mode/strategy may have changed.
 TaskUpdated { task: TaskSummary },
 /// `config_reloaded` - config.toml was hot-reloaded at runtime. Carries the
 /// full new `AppConfig` so the UI can surgically update fields without a
 /// re-fetch. Structural subsystems (pool, peer_server, NAT, log filter,
 /// HTTP bind) were re-applied as needed.
 ConfigReloaded { config: Arc<crate::config::AppConfig> },
 /// `capture_progress` - a fingerprint-capture session advanced (announce,
 /// handshake, ext-handshake, keepalive measured, or connection ended).
 /// Carries the session token, the new status, and the full fingerprint
 /// snapshot. Drives the capture modal via the global SSE stream - no polling.
 CaptureProgress {
 token: String,
 status: crate::capture::CaptureStatus,
 fingerprint: Box<crate::capture::CaptureFingerprintView>,
 },
 /// `goal_progress` - a global goal's summed counters advanced.
 GoalProgress {
 id: i64,
 uploaded: u64,
 downloaded: u64,
 up_bps: u64,
 down_bps: u64,
 /// `None` = unknown speed (no associated running tasks yet) → show "-".
 eta_secs: Option<u64>,
 },
 /// `goal_created` - a new global goal appeared. Carries the goal id.
 GoalCreated { id: i64 },
 /// `goal_deleted` - a global goal was removed. Carries the goal id.
 GoalDeleted { id: i64 },
 /// `goal_updated` - a global goal's config or associations changed.
 GoalUpdated { id: i64 },
}

// Config

/// Configuration for an audit run. Defaults are tuned for "look like a real
/// home connection seeding a torrent" - fast enough to build ratio, slow
/// enough to avoid speed-based flags.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AuditConfig {
 pub announce_url: String,
 pub info_hash: String, // hex
 pub torrent_size: u64,

 /// Target upload speed (bytes/sec). Used in fixed mode. Default 512 KiB/s.
 pub upload_bps: u64,
 /// Random jitter applied to upload speed each second (±percentage).
 /// Default 20% - a constant exact speed is a tell.
 pub jitter_pct: u8,
 /// Ramp-up: grow speed from 0 to target over this many seconds at start.
 /// Default 120 - a real client doesn't instantly max out.
 pub ramp_up_secs: u64,
 /// Operating mode: download+upload (leech→seed lifecycle) or upload-only
 /// (ghost seed). Default DownloadAndUpload.
 pub mode: Mode,
 /// Download speed for the leech phase (bytes/sec). Default 1 MiB/s.
 /// Only used in DownloadAndUpload mode.
 pub download_bps: u64,
 /// Freeze upload when the swarm has 0 leechers - uploading to nobody is
 /// physically impossible and the #1 detection heuristic.
 /// Default true.
 pub freeze_on_zero_leechers: bool,
 /// Freeze download when the swarm has 0 seeders - downloading from nobody
 /// is impossible.
 /// Default true.
 pub freeze_on_zero_seeders: bool,
 /// Pretend we already downloaded this percentage of the torrent at start.
 /// Only applies in DownloadAndUpload mode.
 /// 0 = start from scratch (left=torrent_size), 100 = start as seeder (left=0).
 /// Default 0.
 pub start_download_pct: u8,
 /// Speed control mode: fixed (manual) or dynamic (swarm-aware).
 /// Default Fixed.
 pub speed_mode: SpeedMode,
 /// Swarm dynamics config. Only used when speed_mode is Dynamic.
 pub swarm: crate::swarm::SwarmConfig,
 /// Per-audit goal (target amount + optional deadline). See [`GoalConfig`].
 /// In reverse mode the engine runs a live feedback loop that overrides the
 /// effective speed for `direction` so the target is reached in time.
 /// `#[serde(default)]` so audits stored before the goal feature existed
 /// (no `goal` key in their config_json) deserialize with a disabled goal
 /// instead of failing and falling back to `from_defaults` - which would
 /// silently reset the task's mode/speed/etc. to `[defaults]` on restart.
 #[serde(default = "goal_config_default")]
 pub goal: GoalConfig,
 /// Force a specific emulated client (by `peer_id_prefix` or alias). `None`
 /// = auto-probe: try each configured client in random order until the
 /// tracker accepts one, then use it for all announces.
 #[serde(default)]
 pub forced_client: Option<String>,
}

/// Speed control strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedMode {
 /// Fixed manual upload speed (upload_bps).
 Fixed,
 /// Dynamic: re-announce at the tracker's interval and recalculate upload
 /// speed to match the fair share for a seeder in the current swarm.
 Dynamic,
}

/// Operating mode for the audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
 /// Leech→seed lifecycle: start as leecher, simulate download progress,
 /// send `completed`, then seed. Avoids the ghost-seeder fingerprint.
 DownloadAndUpload,
 /// Ghost seed: start as seeder immediately (left=0, downloaded=torrent_size).
 /// No download phase - pure upload credit farming.
 UploadOnly,
}

/// `Display` mirrors the serde wire names (`#[serde(rename_all = "snake_case")]`)
/// so `mode.to_string()` yields the same lowercase string the frontend and TOML
/// use - a single source of truth for the on-the-wire representation.
impl std::fmt::Display for Mode {
 fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
 f.write_str(match self {
 Mode::DownloadAndUpload => crate::data::vocab::MODE_DU_WIRE,
 Mode::UploadOnly => crate::data::vocab::MODE_UO_WIRE,
 })
 }
}

impl std::fmt::Display for SpeedMode {
 fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
 f.write_str(match self {
 SpeedMode::Fixed => crate::data::vocab::SPEED_FIXED_WIRE,
 SpeedMode::Dynamic => crate::data::vocab::SPEED_DYNAMIC_WIRE,
 })
 }
}

/// Which cumulative counter(s) a goal tracks toward its target amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalDirection {
 /// Track `uploaded` toward `upload_target`.
 Upload,
 /// Track both: `uploaded` toward `upload_target` AND `downloaded` toward
 /// `download_target`. Both must be reached for `reached_action` to fire.
 DownloadAndUpload,
}

impl std::fmt::Display for GoalDirection {
 fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
 f.write_str(match self {
 GoalDirection::Upload => crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE,
 GoalDirection::DownloadAndUpload => crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE,
 })
 }
}

impl GoalDirection {
 /// Does this direction track the upload counter?
 pub fn tracks_upload(self) -> bool {
 matches!(self, GoalDirection::Upload | GoalDirection::DownloadAndUpload)
 }
 /// Does this direction track the download counter?
 pub fn tracks_download(self) -> bool {
 matches!(self, GoalDirection::DownloadAndUpload)
 }
}

/// What the engine does once the goal's cumulative counter reaches its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalReachedAction {
 /// Stop the audit cleanly - same path as the user clicking Stop (sends a
 /// `stopped` announce, flips the DB status, emits `TaskStatus{stopped}`).
 Stop,
 /// Drop the speed override and resume at the original configured speed
 /// (`upload_bps` / `download_bps` / fair-share). The counter keeps growing.
 ContinueInitial,
 /// Switch to a fixed custom speed (`reached_bps`). `reached_bps == 0` freezes
 /// the counter (no further growth) while keeping the task running.
 ContinueCustom,
}

impl std::fmt::Display for GoalReachedAction {
 fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
 f.write_str(match self {
 GoalReachedAction::Stop => crate::data::vocab::GOAL_REACHED_STOP_WIRE,
 GoalReachedAction::ContinueInitial => crate::data::vocab::GOAL_REACHED_CONTINUE_INITIAL_WIRE,
 GoalReachedAction::ContinueCustom => crate::data::vocab::GOAL_REACHED_CONTINUE_CUSTOM_WIRE,
 })
 }
}

/// Serde default for [`GoalReachedAction`] - `Stop` (the safest action; a
/// missing field in an older stored config means "stop when reached").
fn goal_reached_action_default() -> GoalReachedAction {
 GoalReachedAction::Stop
}

/// Serde default for [`GoalConfig`] - a fully-disabled goal. Used by
/// `#[serde(default = "GoalConfig::disabled")]` on [`AuditConfig::goal`] so
/// that config rows stored before the goal feature existed (no `goal` key in
/// the JSON) deserialize successfully instead of falling back to
/// `from_defaults` (which would silently reset the task's mode/speed/etc. to
/// the `[defaults]` values).
fn goal_config_default() -> GoalConfig {
 GoalConfig {
 enabled: false,
 direction: GoalDirection::Upload,
 upload_target: 0,
 download_target: 0,
 target_secs: 0,
 reached_action: GoalReachedAction::Stop,
 reached_bps: 0,
 }
}

/// Per-audit goal: reach a target amount of bytes in the configured
/// direction(s). In forward mode (`target_secs == 0`) the UI shows an ETA at
/// the current speed and the engine leaves the speed untouched. In reverse
/// mode (`target_secs > 0`) the engine runs a live feedback loop each tick,
/// overriding the effective base speed for each tracked direction so the
/// remaining bytes land within the remaining time - equivalent to
/// dynamically adjusting the speed coefficient. Once all tracked targets are
/// reached, [`reached_action`] decides whether to stop, resume the initial
/// speed, or switch to a custom speed.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GoalConfig {
 /// `false` = no goal (the UI hides the goal tiles and the engine skips the
 /// feedback loop).
 pub enabled: bool,
 /// Which counter(s) to grow toward their target. See [`GoalDirection`].
 pub direction: GoalDirection,
 /// Upload target in bytes. Used when `direction` tracks upload (Upload /
 /// DownloadAndUpload). `0` with `enabled` is a degenerate (already
 /// reached) target. `#[serde(default)]` so configs stored before the
 /// field was renamed from `target_bytes` to `upload_target` deserialize
 /// with 0 instead of failing (the old `target_bytes` value is lost in the
 /// rename - acceptable for a pre-release feature).
 #[serde(default)]
 pub upload_target: u64,
 /// Download target in bytes. Used when `direction` tracks download
 /// (Download / DownloadAndUpload). `0` with `enabled` is degenerate.
 /// `#[serde(default)]` so configs stored before this field existed
 /// (single-target era) deserialize with 0 instead of failing.
 #[serde(default)]
 pub download_target: u64,
 /// Deadline in seconds from the audit's start. `0` = forward/ETA-only mode
 /// (display only, no speed adjustment). `> 0` = reverse mode (engine
 /// adjusts speed to hit the target(s) within this time).
 pub target_secs: u64,
 /// What to do once all tracked targets are reached. See
 /// [`GoalReachedAction`]. `#[serde(default)]` so configs stored before
 /// this field existed deserialize with `Stop`.
 #[serde(default = "goal_reached_action_default")]
 pub reached_action: GoalReachedAction,
 /// Custom speed (bytes/sec) for [`GoalReachedAction::ContinueCustom`].
 /// Applied to the upload direction (the primary ratio-building counter).
 /// `0` freezes the counter while keeping the task running. Ignored for the
 /// other actions. `#[serde(default)]` for forward-compatibility.
 #[serde(default)]
 pub reached_bps: u64,
}

impl GoalConfig {
 /// Construct a `GoalConfig` from the flat `goal_*` defaults in `[defaults]`.
 pub fn from_defaults(d: &crate::config::DefaultsConfig) -> Self {
 Self {
 enabled: d.goal_enabled,
 direction: d.goal_direction,
 upload_target: d.goal_upload_target,
 download_target: d.goal_download_target,
 target_secs: d.goal_target_secs,
 reached_action: d.goal_reached_action,
 reached_bps: d.goal_reached_bps,
 }
 }

 /// Validate the goal fields. Bounds mirror `config::DefaultsConfig::validate`
 /// so a hand-built `AuditConfig` can't smuggle in an out-of-range goal.
 pub fn validate(&self) -> Result<(), String> {
 if self.upload_target > crate::data::units::GOAL_MAX_TARGET_BYTES {
 return Err(format!(
 "goal.upload_target must be <= {}, got {}",
 crate::data::units::GOAL_MAX_TARGET_BYTES, self.upload_target
 ));
 }
 if self.download_target > crate::data::units::GOAL_MAX_TARGET_BYTES {
 return Err(format!(
 "goal.download_target must be <= {}, got {}",
 crate::data::units::GOAL_MAX_TARGET_BYTES, self.download_target
 ));
 }
 if self.target_secs > crate::data::units::GOAL_MAX_TIME_SECS {
 return Err(format!(
 "goal.target_secs must be <= {}, got {}",
 crate::data::units::GOAL_MAX_TIME_SECS, self.target_secs
 ));
 }
 Ok(())
 }
}

// Goal target helpers (shared by the feedback loop, the reached check,
// the loop break, the render tiles, and the topbar aggregate)

/// The upload target this goal tracks, or 0 if the direction doesn't track
/// upload.
pub fn goal_upload_target(goal: &GoalConfig) -> u64 {
 if goal.direction.tracks_upload() { goal.upload_target } else { 0 }
}

/// The download target this goal tracks, or 0 if the direction doesn't track
/// download.
pub fn goal_download_target(goal: &GoalConfig) -> u64 {
 if goal.direction.tracks_download() { goal.download_target } else { 0 }
}

/// Has the upload target been reached? (No-op when the goal doesn't track
/// upload - `target == 0` means "not tracking", which is trivially satisfied.)
pub fn goal_upload_reached(state: &PeerState, goal: &GoalConfig) -> bool {
 let t = goal_upload_target(goal);
 t == 0 || state.uploaded >= t
}

/// Has the download target been reached?
pub fn goal_download_reached(state: &PeerState, goal: &GoalConfig) -> bool {
 let t = goal_download_target(goal);
 t == 0 || state.downloaded >= t
}

/// Have ALL tracked targets been reached? Centralizes the check so the loop
/// break and the override agree. Returns `false` when the goal is disabled or
/// has no targets set.
pub fn goal_reached(state: &PeerState, goal: &GoalConfig) -> bool {
 if !goal.enabled {
 return false;
 }
 let up = goal_upload_target(goal);
 let dl = goal_download_target(goal);
 if up == 0 && dl == 0 {
 return false;
 }
 goal_upload_reached(state, goal) && goal_download_reached(state, goal)
}

/// Required bytes/sec to reach `target` from `current` within the goal's
/// remaining time. Returns `0` when no override should be applied: forward
/// mode (`target_secs == 0`), target already reached, or the deadline has
/// passed. Uses ceiling division so a non-zero remainder doesn't under-shoot.
/// Shared by the upload and download feedback overrides.
fn goal_required_bps(target: u64, current: u64, target_secs: u64, elapsed: Duration) -> u64 {
 if target_secs == 0 {
 return 0;
 }
 let remaining_bytes = target.saturating_sub(current);
 if remaining_bytes == 0 {
 return 0;
 }
 let remaining_secs = target_secs.saturating_sub(elapsed.as_secs());
 if remaining_secs == 0 {
 return 0;
 }
 remaining_bytes.div_ceil(remaining_secs)
}

impl AuditConfig {
 /// Construct an `AuditConfig` from the `[defaults]` section of `config.toml`.
 pub fn from_defaults(d: &crate::config::DefaultsConfig, swarm: &crate::config::SwarmDefaultsConfig) -> Self {
 Self {
 announce_url: String::new(),
 info_hash: String::new(),
 torrent_size: 0,
 upload_bps: d.upload_bps,
 jitter_pct: d.jitter_pct as u8,
 ramp_up_secs: d.ramp_up_secs,
 mode: d.mode,
 download_bps: d.download_bps,
 freeze_on_zero_leechers: d.freeze_on_zero_leechers,
 freeze_on_zero_seeders: d.freeze_on_zero_seeders,
 start_download_pct: d.start_download_pct as u8,
 speed_mode: d.speed_mode,
 swarm: crate::swarm::SwarmConfig::from_defaults(swarm),
 goal: GoalConfig::from_defaults(d),
 forced_client: None,
 }
 }
}

impl AuditConfig {
 /// Validate range-constrained fields. Returns `Err(message)` naming the
 /// first invalid field, or `Ok(())` if every field is in range.
 pub fn validate(&self) -> Result<(), String> {
 if self.torrent_size == 0 {
 return Err("torrent_size must be greater than 0".into());
 }
 if self.jitter_pct > crate::data::units::PERCENT as u8 {
 return Err(format!(
 "jitter_pct must be 0..=100, got {}",
 self.jitter_pct
 ));
 }
 if self.start_download_pct > crate::data::units::PERCENT as u8 {
 return Err(format!(
 "start_download_pct must be 0..=100, got {}",
 self.start_download_pct
 ));
 }
 self.swarm.validate().map_err(|e| format!("swarm.{e}"))?;
 self.goal.validate().map_err(|e| format!("goal.{e}"))?;
 Ok(())
 }
}

/// Compute initial `(downloaded, left)` for DownloadAndUpload mode.
///
/// `start_download_pct` is clamped to `0..=100` and the subtraction uses
/// `saturating_sub`, so `left` can never underflow even if a caller bypasses
/// [`AuditConfig::validate`] (defense in depth).
fn initial_download_state(torrent_size: u64, start_download_pct: u8) -> (u64, u64) {
 let pct = start_download_pct.min(crate::data::units::PERCENT as u8) as u64;
 let dl = torrent_size * pct / crate::data::units::PERCENT as u64;
 (dl, torrent_size.saturating_sub(dl))
}

/// Compute the jittered announce delay in seconds: tracker's `interval`
/// plus symmetric ±`jitter_pct`% timing jitter. Perfectly metronomic timing
/// is itself a fingerprint of automated tools; real clients announce slightly
/// early or late. `jitter_pct = 0.0` produces exactly `interval`. The result
/// is floored at 1 second to avoid a zero or negative delay.
fn jittered_interval(interval: u32, jitter_pct: f64) -> u64 {
 let jitter_secs = (interval as f64 * jitter_pct / crate::data::units::PERCENT as f64) as i64;
 if jitter_secs <= 0 {
 return interval as u64;
 }
 let delta = rand::rng().random_range(-jitter_secs..=jitter_secs);
 (interval as i64 + delta).max(1) as u64
}

/// Compute the next announce instant: tracker's `interval` plus symmetric
/// ±`jitter_pct`% timing jitter. Jitter is recomputed each call so it tracks
/// interval changes from the tracker. `jitter_pct = 0.0` produces exactly
/// `interval`.
fn schedule_next_announce(interval: u32, jitter_pct: f64) -> Instant {
 Instant::now() + Duration::from_secs(jittered_interval(interval, jitter_pct))
}

/// Persisted peer state for resuming after stop/start.
#[derive(Debug, Clone, Default)]
pub struct ResumeState {
 pub uploaded: u64,
 pub downloaded: u64,
 pub left: u64,
 pub lifecycle_phase: String,
 pub completed_sent: bool,
 pub elapsed_secs: u64,
 pub peer_id: Option<[u8; protocol::PEER_ID_LEN]>,
 pub key: Option<String>,
}

impl From<crate::db::PeerStateRow> for ResumeState {
 fn from(r: crate::db::PeerStateRow) -> Self {
 ResumeState {
 uploaded: r.uploaded,
 downloaded: r.downloaded,
 left: r.left,
 lifecycle_phase: r.lifecycle_phase.unwrap_or_default(),
 completed_sent: r.completed_sent,
 elapsed_secs: r.elapsed_secs,
 peer_id: r.peer_id.as_deref().and_then(|h| bencode::hex_decode_20(h).ok()),
 key: r.key,
 }
 }
}

// Engine

/// Options for resuming a stopped audit.
#[derive(Default)]
pub struct RunOptions {
 pub known_client: Option<usize>,
 pub resume: Option<ResumeState>,
 pub start_seq: u64,
 pub peer_server: Option<std::sync::Arc<crate::peer_server::PeerServer>>,
}


/// Run the full audit: probe clients (unless we already know the working one),
/// then fake ratio with one connection.
pub async fn run(
 config: AuditConfig,
 cfg: &crate::config::AppConfig,
 audit_id: i64,
 opts: RunOptions,
 pool: &sqlx::SqlitePool,
 tx: broadcast::Sender<AuditEvent>,
 cancel: CancellationToken,
) {
 let info_hash = match crate::bencode::hex_decode_20(&config.info_hash) {
 Ok(h) => h,
 Err(e) => {
 tracing::error!(audit_id, hash = %config.info_hash, error = %e, "invalid info_hash");
 let _ = tx.send(AuditEvent {
 audit_id,
 seq: 0,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_PROBE,
 client: String::new(),
 event: vocab::EVENT_PROBE,
 uploaded: 0,
 downloaded: 0,
 left: config.torrent_size,
 success: false,
 failure_reason: Some(format!("invalid info_hash: {e}")),
 interval: cfg.tracker.default_interval_secs,
 seeders: 0,
 leechers: 0,
 peer_count: 0,
 latency_ms: 0,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 return;
 }
 };
 let mut seq: u64 = opts.start_seq;

 // Phase 1: Probe - find a client the tracker accepts
 // Skip if we already know the working client from a previous run.
 let mut working_client: Option<usize> = opts.known_client;

 if let Some(idx) = opts.known_client {
 tracing::info!(audit_id, client = cfg.clients[idx].display_name().as_str(), "skipping probe - using known working client");
 }

 // Probe clients in random order so two audits hitting the same tracker
 // don't always probe the same client first (a deterministic probe order
 // is itself a fingerprint). Fisher-Yates over the index vector - the
 // probe body indexes `cfg.clients[i]`, so only the iteration order
 // changes. Stop at the first accepted client.
 let mut probe_order: Vec<usize> = (0..cfg.clients.len()).collect();
 {
 let mut rng = rand::rng();
 for i in (1..probe_order.len()).rev() {
 let j = rng.random_range(0..=i);
 probe_order.swap(i, j);
 }
 }
 for i in probe_order {
 if working_client.is_some() {
 break;
 }
 if cancel.is_cancelled() {
 return;
 }
 let probe_identity = PeerIdentity {
 peer_id: crate::peer_id::generate_peer_id(&cfg.clients[i].peer_id_prefix),
 key: crate::peer_id::generate_key(cfg.clients[i].key_format),
 };
 let session = AnnounceSession::new(&config.announce_url, info_hash, &cfg.clients[i], cfg.tracker.peer_port, cfg.http.timeout_secs, IntervalBounds { min_secs: cfg.tracker.min_interval_secs, max_secs: cfg.tracker.max_interval_secs, default_secs: cfg.tracker.default_interval_secs }, probe_identity);
 let (probe_dl, probe_left) = match config.mode {
 Mode::UploadOnly => (config.torrent_size, 0),
 Mode::DownloadAndUpload => {
 initial_download_state(config.torrent_size, config.start_download_pct)
 }
 };
 let state = PeerState {
 uploaded: 0,
 downloaded: probe_dl,
 left: probe_left,
 };
 let t0 = Instant::now();
 match session.announce(state, Event::Started).await {
 Ok(resp) => {
 let accepted = !resp.is_failure();
 if accepted {
 tracing::info!(audit_id, client = cfg.clients[i].display_name().as_str(), "probe accepted");
 } else {
 tracing::warn!(
 audit_id,
 client = cfg.clients[i].display_name().as_str(),
 reason = resp.failure_reason.as_deref().unwrap_or("unknown"),
 "probe rejected"
 );
 }
 if accepted && working_client.is_none() {
 working_client = Some(i);
 }
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_PROBE,
 client: cfg.clients[i].display_name(),
 event: vocab::EVENT_PROBE,
 uploaded: 0,
 downloaded: 0,
 left: state.left,
 success: accepted,
 failure_reason: resp.failure_reason.clone(),
 interval: resp.interval,
 seeders: resp.seeders,
 leechers: resp.leechers,
 peer_count: resp.peer_count,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: working_client.map(|idx| cfg.clients[idx].peer_id_prefix.clone()),
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 if accepted {
 break;
 }
 }
 Err(e) => {
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_PROBE,
 client: cfg.clients[i].display_name(),
 event: vocab::EVENT_PROBE,
 uploaded: 0,
 downloaded: 0,
 left: config.torrent_size,
 success: false,
 failure_reason: Some(e.to_string()),
 interval: cfg.tracker.default_interval_secs,
 seeders: 0,
 leechers: 0,
 peer_count: 0,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 }
 }
 }

 let Some(client_idx) = working_client else {
 tracing::info!(audit_id, "all clients rejected - check if the tracker is reachable and your passkey/announce URL is valid");
 return;
 };
 let client = &cfg.clients[client_idx];
 tracing::info!(audit_id, client = client.display_name().as_str(), "working client - starting attack");

 // Decide the peer identity for this audit: reuse the persisted peer_id/key
 // (so the tracker credits resumed counters to the same peer) or generate
 // fresh ones for a new audit. A new random peer_id on every restart would
 // make the tracker treat the resumed cumulative counters as a brand-new
 // peer's baseline (delta = 0) - losing all un-announced upload credit.
 let identity = match opts.resume.as_ref().and_then(|r| {
 let pid = r.peer_id?;
 let key = r.key.as_ref()?;
 Some(PeerIdentity { peer_id: pid, key: key.clone() })
 }) {
 Some(id) => {
 tracing::info!(audit_id, peer_id = %bencode::hex_encode(&id.peer_id), "reusing persisted peer identity");
 id
 }
 None => {
 let id = PeerIdentity {
 peer_id: crate::peer_id::generate_peer_id(&client.peer_id_prefix),
 key: crate::peer_id::generate_key(client.key_format),
 };
 tracing::info!(audit_id, peer_id = %bencode::hex_encode(&id.peer_id), "generated new peer identity");
 id
 }
 };
 let peer_id_hex = bencode::hex_encode(&identity.peer_id);

 // Phase 2: Attack - one connection, fake upload with stealth
 let session = AnnounceSession::new(&config.announce_url, info_hash, client, cfg.tracker.peer_port, cfg.http.timeout_secs, IntervalBounds { min_secs: cfg.tracker.min_interval_secs, max_secs: cfg.tracker.max_interval_secs, default_secs: cfg.tracker.default_interval_secs }, identity.clone());

 // Register with the global peer-wire server so the emulated peer is
 // "connectable". Leechers connecting to our advertised peer_port get a
 // valid BT handshake + bitfield + unchoke + keepalives - but no piece
 // data. This passes Unit3D's connectability probe without triggering
 // hash-mismatch bans. The peer server is shared across all audits.
 // Uses the same peer_id as the announce so the wire handshake matches
 // what the tracker advertised to other peers.
 let peer_server = opts.peer_server.as_ref();
 if let Some(ps) = peer_server
 && let Err(e) = ps.register(info_hash, identity.peer_id, client) {
 tracing::warn!(audit_id, error = %e, "peer server registration failed - continuing without connectability for this torrent");
 }

 // Initialize state - resume from persisted counters if available, otherwise
 // compute from config (first run or mode change).
 let resume_elapsed = opts.resume.as_ref().map(|r| r.elapsed_secs).unwrap_or(0);
 let (mut state, mut lifecycle_phase, mut completed_sent) = match &opts.resume {
 Some(r) if r.left > 0 || r.uploaded > 0 || r.downloaded > 0 => {
 tracing::info!(
 audit_id,
 uploaded = r.uploaded,
 downloaded = r.downloaded,
 left = r.left,
 phase = r.lifecycle_phase.as_str(),
 "resuming from persisted state"
 );
 let phase = if r.lifecycle_phase.is_empty() { vocab::LIFECYCLE_LEECH } else { r.lifecycle_phase.as_str() };
 (
 PeerState {
 uploaded: r.uploaded,
 downloaded: r.downloaded,
 left: r.left,
 },
 phase,
 r.completed_sent,
 )
 }
 _ => {
 let (initial_downloaded, initial_left, lifecycle_phase_start, completed_sent_start) = match config.mode {
 Mode::UploadOnly => (config.torrent_size, 0, vocab::LIFECYCLE_SEED, true),
 Mode::DownloadAndUpload => {
 let (dl, left) = initial_download_state(config.torrent_size, config.start_download_pct);
 if left == 0 {
 (config.torrent_size, 0, vocab::LIFECYCLE_SEED, true)
 } else {
 (dl, left, vocab::LIFECYCLE_LEECH, false)
 }
 }
 };
 (
 PeerState {
 uploaded: 0,
 downloaded: initial_downloaded,
 left: initial_left,
 },
 lifecycle_phase_start,
 completed_sent_start,
 )
 }
 };
 let mut dynamic_target_bps: u64 = 0;
 let mut dynamic_download_bps: u64 = 0;
 let mut last_leecher_count = 0;
 let mut last_seeder_count = 0;
 let mut last_peer_count = 0;
 let start = Instant::now() - Duration::from_secs(resume_elapsed);
 let mut interval = cfg.tracker.default_interval_secs;

 // Send `started`
 let t0 = Instant::now();
 match session.announce(state, Event::Started).await {
 Ok(resp) => {
 if !resp.is_failure() {
 interval = resp.effective_interval();
 last_leecher_count = resp.leechers;
 last_seeder_count = resp.seeders;
 last_peer_count = resp.peer_count;
 // Calculate fair share immediately from the announce response
 if config.speed_mode == SpeedMode::Dynamic {
 let swarm_data = crate::swarm::SwarmData {
 seeders: resp.seeders,
 leechers: resp.leechers,
 };

 dynamic_target_bps = crate::swarm::fair_share_bps(&swarm_data, &config.swarm);
 if config.mode == Mode::DownloadAndUpload {
 dynamic_download_bps = crate::swarm::dynamic_download_bps(&swarm_data, &config.swarm);
 }
 tracing::info!(
 audit_id,
 seeders = resp.seeders,
 leechers = resp.leechers,
 upload_bps = dynamic_target_bps,
 download_bps = dynamic_download_bps,
 "dynamic speed calculated from started announce"
 );
 }
 }
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: client.display_name().as_str().to_string(),
 event: vocab::EVENT_STARTED,
 uploaded: state.uploaded,
 downloaded: state.downloaded,
 left: state.left,
 success: !resp.is_failure(),
 failure_reason: resp.failure_reason,
 interval: resp.interval,
 seeders: resp.seeders,
 leechers: resp.leechers,
 peer_count: resp.peer_count,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 let _ = db::save_peer_state(pool, audit_id, db::SavePeerState { uploaded: state.uploaded, downloaded: state.downloaded, left: state.left, lifecycle_phase, completed_sent, elapsed_secs: start.elapsed().as_secs(), peer_id: &peer_id_hex, key: &identity.key }).await;
 }
 Err(e) => {
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: client.display_name().as_str().to_string(),
 event: vocab::EVENT_STARTED,
 uploaded: state.uploaded,
 downloaded: state.downloaded,
 left: state.left,
 success: false,
 failure_reason: Some(e.to_string()),
 interval: cfg.tracker.default_interval_secs,
 seeders: 0,
 leechers: 0,
 peer_count: 0,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 }
 }

 // Main loop: tick every second, announce at tracker's interval.
 // Emit a stat event every 5 seconds so the UI shows real-time progress
 // even between announces (which can be 30-60 min apart).
 let mut next_announce = schedule_next_announce(interval, cfg.engine.announce_jitter_pct);
 let stat_interval = Duration::from_secs(cfg.engine.stat_interval_secs);
 let mut next_stat = Instant::now() + stat_interval;
 let mut ticker = tokio::time::interval(Duration::from_secs(cfg.engine.tick_interval_secs));
 ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

 loop {
 tokio::select! {
 _ = cancel.cancelled() => break,
 _ = ticker.tick() => {
 let elapsed = start.elapsed();

 let tick_speeds = tick(&mut state, elapsed, &config, &cfg.engine, lifecycle_phase, &TickContext { leecher_count: last_leecher_count, seeder_count: last_seeder_count, dynamic_target_bps, dynamic_download_bps });

 let now = Instant::now();
 if now >= next_stat {
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: client.display_name().as_str().to_string(),
 event: vocab::EVENT_TICK,
 uploaded: state.uploaded,
 downloaded: state.downloaded,
 left: state.left,
 success: true,
 failure_reason: None,
 interval,
 seeders: last_seeder_count,
 leechers: last_leecher_count,
 peer_count: last_peer_count,
 latency_ms: 0,
 working_client: None,
 fair_share_bps: tick_speeds.upload_bps,
 dynamic_target_bps: tick_speeds.download_bps,
 next_announce_in_secs: next_announce.saturating_duration_since(now).as_secs(),
 elapsed_secs: start.elapsed().as_secs(),
 });
 let _ = db::save_peer_state(pool, audit_id, db::SavePeerState { uploaded: state.uploaded, downloaded: state.downloaded, left: state.left, lifecycle_phase, completed_sent, elapsed_secs: start.elapsed().as_secs(), peer_id: &peer_id_hex, key: &identity.key }).await;
 next_stat = now + Duration::from_secs(cfg.engine.stat_interval_secs);
 }

 if now >= next_announce {
 // Determine event: send `completed` once when leech finishes
 let event = if lifecycle_phase == vocab::LIFECYCLE_LEECH && state.left == 0 && !completed_sent {
 completed_sent = true;
 lifecycle_phase = vocab::LIFECYCLE_SEED;
 Event::Completed
 } else {
 Event::None
 };

 let t0 = Instant::now();
 match session.announce(state, event).await {
 Ok(resp) => {
 if !resp.is_failure() {
 interval = resp.effective_interval();
 last_leecher_count = resp.leechers;
 last_seeder_count = resp.seeders;
 last_peer_count = resp.peer_count;
 // Recalculate fair share from fresh announce data
 if config.speed_mode == SpeedMode::Dynamic {
 let swarm_data = crate::swarm::SwarmData {
 seeders: resp.seeders,
 leechers: resp.leechers,
 };

 dynamic_target_bps = crate::swarm::fair_share_bps(&swarm_data, &config.swarm);
 if config.mode == Mode::DownloadAndUpload {
 dynamic_download_bps = crate::swarm::dynamic_download_bps(&swarm_data, &config.swarm);
 }
 }
 }
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: client.display_name().as_str().to_string(),
 event: if event == Event::Completed { vocab::EVENT_COMPLETED } else { vocab::EVENT_REGULAR },
 uploaded: state.uploaded,
 downloaded: state.downloaded,
 left: state.left,
 success: !resp.is_failure(),
 failure_reason: resp.failure_reason,
 interval: resp.interval,
 seeders: resp.seeders,
 leechers: resp.leechers,
 peer_count: resp.peer_count,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 let _ = db::save_peer_state(pool, audit_id, db::SavePeerState { uploaded: state.uploaded, downloaded: state.downloaded, left: state.left, lifecycle_phase, completed_sent, elapsed_secs: start.elapsed().as_secs(), peer_id: &peer_id_hex, key: &identity.key }).await;
 }
 Err(e) => {
 tracing::warn!(audit_id, client = client.display_name().as_str(), error = %e, "announce failed");
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: client.display_name().as_str().to_string(),
 event: vocab::EVENT_REGULAR,
 uploaded: state.uploaded,
 downloaded: state.downloaded,
 left: state.left,
 success: false,
 failure_reason: Some(e.to_string()),
 interval: cfg.tracker.default_interval_secs,
 seeders: 0,
 leechers: 0,
 peer_count: 0,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 }
 }
 next_announce = schedule_next_announce(interval, cfg.engine.announce_jitter_pct);
 }

 // Goal-reached stop: if the goal's counter has reached its
 // target and the action is Stop, break the loop now. Placed
 // AFTER the announce block so a due `completed` lifecycle
 // transition (leech→seed) fires first. `cancel.cancel()` is
 // defensive - it signals any other token awaiter; the `break`
 // drops into the existing cleanup tail (stopped announce +
 // STATUS_STOPPED + TaskStatus{stopped} + RunningAudit removal),
 // reusing the same path as the user clicking Stop.
 if config.goal.reached_action == GoalReachedAction::Stop
 && goal_reached(&state, &config.goal)
 {
 cancel.cancel();
 break;
 }
 }
 }
 }

 // Send `stopped`. Both Ok and Err are recorded so a failed shutdown
 // announce is visible in the event log - identical to every other
 // announce call site (probe/started/regular/completed).
 let t0 = Instant::now();
 match session.announce(state, Event::Stopped).await {
 Ok(resp) => {
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: client.display_name().as_str().to_string(),
 event: vocab::EVENT_STOPPED,
 uploaded: state.uploaded,
 downloaded: state.downloaded,
 left: state.left,
 success: !resp.is_failure(),
 failure_reason: resp.failure_reason,
 interval: resp.interval,
 seeders: resp.seeders,
 leechers: resp.leechers,
 peer_count: resp.peer_count,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 }
 Err(e) => {
 tracing::warn!(audit_id, client = client.display_name().as_str(), error = %e, "stopped announce failed");
 seq += 1;
 let _ = tx.send(AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: client.display_name().as_str().to_string(),
 event: vocab::EVENT_STOPPED,
 uploaded: state.uploaded,
 downloaded: state.downloaded,
 left: state.left,
 success: false,
 failure_reason: Some(e.to_string()),
 interval: cfg.tracker.default_interval_secs,
 seeders: 0,
 leechers: 0,
 peer_count: 0,
 latency_ms: t0.elapsed().as_millis() as u64,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 });
 }
 }
 // Always save final state, even if the stopped announce failed
 let _ = db::save_peer_state(pool, audit_id, db::SavePeerState { uploaded: state.uploaded, downloaded: state.downloaded, left: state.left, lifecycle_phase, completed_sent, elapsed_secs: start.elapsed().as_secs(), peer_id: &peer_id_hex, key: &identity.key }).await;

 if let Some(ps) = peer_server {
 ps.deregister(&info_hash);
 }
 tracing::info!(audit_id, "audit stopped");
}

/// Swarm context passed to `tick` to avoid too many arguments.
struct TickContext {
 leecher_count: i64,
 seeder_count: i64,
 dynamic_target_bps: u64,
 dynamic_download_bps: u64,
}

/// Actual speeds achieved during a tick, for stat reporting.
struct TickSpeeds {
 upload_bps: u64,
 download_bps: u64,
}

/// Update counters for one tick (1 second) with realistic behavior.
/// Returns the actual upload/download speeds achieved this tick.
/// - Leech phase: grow `downloaded`, shrink `left`
/// - Seed phase: grow `uploaded` with ramp-up, jitter, and bursts
/// - Freeze upload when 0 leechers (if enabled)
fn tick(state: &mut PeerState, elapsed: Duration, config: &AuditConfig, engine_cfg: &crate::config::EngineConfig, lifecycle_phase: &str, ctx: &TickContext) -> TickSpeeds {
 let mut rng = rand::rng();
 let dt = engine_cfg.tick_interval_secs as f64;

 // Determine effective speeds: dynamic or fixed
 let base_upload_bps = if config.speed_mode == SpeedMode::Dynamic {
 ctx.dynamic_target_bps
 } else {
 config.upload_bps
 };
 let base_download_bps = if config.speed_mode == SpeedMode::Dynamic {
 ctx.dynamic_download_bps
 } else {
 config.download_bps
 };

 // Goal live feedback: two phases. (1) Before all tracked targets are
 // reached, reverse mode overrides the base speed for each tracked
 // direction so the remaining bytes land within the remaining time -
 // equivalent to dynamically adjusting the speed coefficient. In
 // DownloadAndUpload mode both upload and download speeds are overridden
 // independently from their respective targets. (2) After all targets
 // are reached, `reached_action` decides what happens: `Stop` (the loop
 // breaks after this tick), `ContinueInitial` (drop the override → default
 // speed resumes), `ContinueCustom` (override the upload direction with
 // `reached_bps`; 0 freezes the counter). The per-direction hard cap
 // (max_upload_bps / max_download_bps, 0 = unlimited) bounds every override.
 let mut base_upload_bps = base_upload_bps;
 let mut base_download_bps = base_download_bps;
 if config.goal.enabled {
 let up_t = goal_upload_target(&config.goal);
 let dl_t = goal_download_target(&config.goal);
 let all_reached = goal_reached(state, &config.goal);
 if all_reached {
 // All tracked targets reached - apply the reached-action override.
 match config.goal.reached_action {
 GoalReachedAction::Stop | GoalReachedAction::ContinueInitial => {
 // No override: Stop will break the loop; ContinueInitial
 // resumes at the default speed. Both leave the base as-is.
 }
 GoalReachedAction::ContinueCustom => {
 let bps = config.goal.reached_bps;
 base_upload_bps = if config.swarm.max_upload_bps > 0 {
 bps.min(config.swarm.max_upload_bps)
 } else {
 bps
 };
 }
 }
 } else {
 // Not all reached - reverse-mode feedback override per direction.
 if up_t > 0 && state.uploaded < up_t {
 let req = goal_required_bps(up_t, state.uploaded, config.goal.target_secs, elapsed);
 if req > 0 {
 base_upload_bps = if config.swarm.max_upload_bps > 0 {
 req.min(config.swarm.max_upload_bps)
 } else {
 req
 };
 }
 }
 if dl_t > 0 && state.downloaded < dl_t {
 let req = goal_required_bps(dl_t, state.downloaded, config.goal.target_secs, elapsed);
 if req > 0 {
 base_download_bps = if config.swarm.max_download_bps > 0 {
 req.min(config.swarm.max_download_bps)
 } else {
 req
 };
 }
 }
 }
 }

 // Ramp-up: linear from 0 to target over ramp_up_secs
 let ramp_factor = if config.ramp_up_secs > 0 {
 (elapsed.as_secs_f64() / config.ramp_up_secs as f64).clamp(0.0, 1.0)
 } else {
 1.0
 };

 // Jitter: ±jitter_pct% random variation
 let jitter = 1.0 + (rng.random_range(-(config.jitter_pct as f64)..=config.jitter_pct as f64) / crate::data::units::PERCENT as f64);

 let mut speeds = TickSpeeds { upload_bps: 0, download_bps: 0 };

 match lifecycle_phase {
 vocab::LIFECYCLE_LEECH => {
 // Freeze download if 0 seeders - downloading from nobody is impossible
 if config.freeze_on_zero_seeders && ctx.seeder_count == 0 {
 // Still upload during leech if there are leechers to upload to
 if !(config.freeze_on_zero_leechers && ctx.leecher_count == 0) {
 let up_speed = base_upload_bps as f64 * ramp_factor * jitter * engine_cfg.leech_upload_factor;
 state.uploaded += (up_speed * dt) as u64;
 speeds.upload_bps = up_speed as u64;
 }
 return speeds;
 }
 // Simulate download progress
 let dl_speed = base_download_bps as f64 * jitter;
 let dl_bytes = (dl_speed * dt) as u64;
 state.downloaded += dl_bytes;
 state.left = state.left.saturating_sub(dl_bytes);
 speeds.download_bps = dl_speed as u64;
 if state.left == 0 {
 state.downloaded = config.torrent_size; // ensure consistency
 }
 // Also upload during leech (real clients upload while downloading)
 let up_speed = base_upload_bps as f64 * ramp_factor * jitter * engine_cfg.leech_upload_factor;
 state.uploaded += (up_speed * dt) as u64;
 speeds.upload_bps = up_speed as u64;
 }
 vocab::LIFECYCLE_SEED => {
 // Freeze upload if 0 leechers - uploading to nobody is impossible
 if config.freeze_on_zero_leechers && ctx.leecher_count == 0 {
 return speeds; // no upload growth this tick
 }

 // Bursty upload: ~30% of ticks produce 0 (choked, no requests)
 let burst_roll: f64 = rng.random();
 if burst_roll < engine_cfg.burst_choke_probability {
 return speeds; // choked this second - no upload
 }

 let effective_bps = base_upload_bps as f64 * ramp_factor * jitter;
 state.uploaded += (effective_bps * dt) as u64;
 speeds.upload_bps = effective_bps as u64;
 }
 other => {
 tracing::warn!(phase = other, "unknown lifecycle_phase in tick - no-op");
 }
 }
 speeds
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::announce::PeerState;

 fn default_config() -> AuditConfig {
 AuditConfig {
 announce_url: "http://t.com/a".into(),
 info_hash: "0000000000000000000000000000000000000000".into(),
 torrent_size: 1_073_741_824,
 upload_bps: 524_288,
 jitter_pct: 20,
 ramp_up_secs: 120,
 mode: Mode::DownloadAndUpload,
 download_bps: 1_048_576,
 freeze_on_zero_leechers: true,
 freeze_on_zero_seeders: true,
 start_download_pct: 0,
 speed_mode: SpeedMode::Fixed,
 swarm: crate::swarm::SwarmConfig::from_defaults(&crate::config::test_helpers::swarm_defaults_cfg()),
 goal: GoalConfig { enabled: false, direction: GoalDirection::Upload, upload_target: 0, download_target: 0, target_secs: 0, reached_action: GoalReachedAction::Stop, reached_bps: 0 },
 forced_client: None,
 }
 }

 fn default_engine_cfg() -> crate::config::EngineConfig {
 crate::config::test_helpers::engine_cfg()
 }

 // tick: leech phase

 #[test]
 fn leech_phase_grows_downloaded() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: 0, left: config.torrent_size };
 let elapsed = Duration::from_secs(10);
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "leech", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 assert!(state.downloaded > 0, "downloaded should grow during leech");
 assert!(state.left < config.torrent_size, "left should decrease during leech");
 }

 #[test]
 fn leech_phase_uploads_half_speed() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: 0, left: config.torrent_size };
 // After ramp-up is complete (elapsed > ramp_up_secs)
 let elapsed = Duration::from_secs(200);
 let before = state.uploaded;
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "leech", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 let delta = state.uploaded - before;
 // Should upload at roughly half the upload_bps (with jitter)
 // upload_bps=524288, half=262144, with ±20% jitter: [209715, 314572]
 assert!(delta > 100_000 && delta < 400_000, "leech upload delta {delta} should be ~half upload_bps");
 }

 #[test]
 fn leech_phase_left_reaches_zero() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: 0, left: 100 };
 let elapsed = Duration::from_secs(200);
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "leech", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 assert_eq!(state.left, 0, "left should reach 0 when download completes");
 assert_eq!(state.downloaded, config.torrent_size, "downloaded should equal torrent_size");
 }

 // tick: seed phase

 #[test]
 fn seed_phase_grows_uploaded() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(200); // past ramp-up
 // Run multiple ticks since 30% produce 0 (bursty)
 for _ in 0..100 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "seed", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 }
 assert!(state.uploaded > 0, "uploaded should grow during seed with leechers present");
 }

 #[test]
 fn seed_phase_freezes_on_zero_leechers() {
 let config = default_config();
 let mut state = PeerState { uploaded: 100, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(200);
 let before = state.uploaded;
 // Run 1000 ticks with 0 leechers - should never grow
 for _ in 0..1000 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "seed", &TickContext { leecher_count: 0, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 }
 assert_eq!(state.uploaded, before, "uploaded must NOT grow when 0 leechers and freeze enabled");
 }

 #[test]
 fn seed_phase_uploads_with_zero_leechers_when_freeze_disabled() {
 let mut config = default_config();
 config.freeze_on_zero_leechers = false;
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(200);
 for _ in 0..100 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "seed", &TickContext { leecher_count: 0, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 }
 assert!(state.uploaded > 0, "uploaded should grow when freeze disabled even with 0 leechers");
 }

 // tick: ramp-up

 #[test]
 fn ramp_up_at_start_produces_low_speed() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 // At t=1s with 120s ramp, factor = 1/120 ≈ 0.008
 let elapsed = Duration::from_secs(1);
 for _ in 0..200 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "seed", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 }
 let total = state.uploaded;
 // Should be very small compared to 200 seconds at full speed (524288*200 = 104857600)
 assert!(total < 10_000_000, "ramp-up should produce low upload: got {total}");
 }

 #[test]
 fn ramp_up_complete_produces_full_speed() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 // Past ramp-up
 let elapsed = Duration::from_secs(500);
 for _ in 0..200 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "seed", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 }
 // Should be near full speed * 200 ticks (minus the ~30% bursty zeros)
 // Expected ~70% of 200 * 524288 = ~73M, with jitter
 assert!(state.uploaded > 30_000_000, "past ramp-up should produce high upload: got {}", state.uploaded);
 }

 // tick: counters never decrease

 #[test]
 fn uploaded_never_decreases() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(200);
 let mut prev = state.uploaded;
 for _ in 0..500 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "seed", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 assert!(state.uploaded >= prev, "uploaded decreased from {prev} to {}", state.uploaded);
 prev = state.uploaded;
 }
 }

 #[test]
 fn downloaded_never_decreases() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: 0, left: config.torrent_size };
 let elapsed = Duration::from_secs(200);
 let mut prev = state.downloaded;
 for _ in 0..500 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "leech", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 assert!(state.downloaded >= prev, "downloaded decreased from {prev} to {}", state.downloaded);
 prev = state.downloaded;
 }
 }

 #[test]
 fn left_never_increases_during_leech() {
 let config = default_config();
 let mut state = PeerState { uploaded: 0, downloaded: 0, left: config.torrent_size };
 let elapsed = Duration::from_secs(200);
 let mut prev = state.left;
 for _ in 0..500 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "leech", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 assert!(state.left <= prev, "left increased from {prev} to {}", state.left);
 prev = state.left;
 }
 }

 // ResumeState::from(PeerStateRow) - peer_id hex decode

 #[test]
 fn resume_state_decodes_valid_peer_id_hex() {
 let row = crate::db::PeerStateRow {
 uploaded: 100, downloaded: 50, left: 0,
 lifecycle_phase: Some("seed".into()), completed_sent: true, elapsed_secs: 42,
 peer_id: Some("2d7142353232302dabcdef0123456789abcdef01".into()),
 key: Some("DEADBEEF".into()),
 };
 let resume = ResumeState::from(row);
 assert_eq!(resume.uploaded, 100);
 assert_eq!(resume.downloaded, 50);
 assert_eq!(resume.left, 0);
 assert_eq!(resume.lifecycle_phase, "seed");
 assert!(resume.completed_sent);
 assert_eq!(resume.elapsed_secs, 42);
 assert!(resume.peer_id.is_some(), "valid 40-char hex must decode to Some");
 assert_eq!(resume.key.as_deref(), Some("DEADBEEF"));
 // Verify the decoded bytes start with "-qB5220-" (the prefix)
 let pid = resume.peer_id.unwrap();
 assert_eq!(&pid[..8], b"-qB5220-");
 }

 #[test]
 fn resume_state_returns_none_for_invalid_peer_id_hex() {
 // A corrupted or truncated peer_id in the DB must not panic - it
 // falls back to None so the engine generates a fresh identity.
 let row = crate::db::PeerStateRow {
 uploaded: 100, downloaded: 50, left: 0,
 lifecycle_phase: Some("seed".into()), completed_sent: true, elapsed_secs: 42,
 peer_id: Some("not-valid-hex".into()),
 key: Some("DEADBEEF".into()),
 };
 let resume = ResumeState::from(row);
 assert!(resume.peer_id.is_none(), "invalid hex must yield None, not panic");
 assert_eq!(resume.key.as_deref(), Some("DEADBEEF"), "key is independent of peer_id validity");
 }

 #[test]
 fn resume_state_returns_none_for_truncated_peer_id_hex() {
 // 39 chars instead of 40 - must fail hex_decode_20's length check
 let row = crate::db::PeerStateRow {
 uploaded: 0, downloaded: 0, left: 0,
 lifecycle_phase: None, completed_sent: false, elapsed_secs: 0,
 peer_id: Some("2d7142353232302dabcdef0123456789abcdef0".into()), // 39 chars
 key: None,
 };
 let resume = ResumeState::from(row);
 assert!(resume.peer_id.is_none(), "truncated hex must yield None");
 assert!(resume.key.is_none());
 }

 #[test]
 fn resume_state_handles_none_peer_id_and_key() {
 // Old DB migrated to new schema: peer_id/key columns are NULL
 let row = crate::db::PeerStateRow {
 uploaded: 1_000_000, downloaded: 500_000, left: 0,
 lifecycle_phase: Some("seed".into()), completed_sent: true, elapsed_secs: 100,
 peer_id: None,
 key: None,
 };
 let resume = ResumeState::from(row);
 assert_eq!(resume.uploaded, 1_000_000, "counters must still load from old DB");
 assert!(resume.peer_id.is_none(), "NULL peer_id → None");
 assert!(resume.key.is_none(), "NULL key → None");
 }

 // config defaults

 #[test]
 fn from_defaults_provides_sensible_values() {
 let d = crate::config::test_helpers::defaults_cfg();
 let s = crate::config::test_helpers::swarm_defaults_cfg();
 let config = AuditConfig::from_defaults(&d, &s);
 assert_eq!(config.upload_bps, 524_288, "default upload 512 KiB/s");
 assert_eq!(config.jitter_pct, 20, "default jitter 20%");
 assert_eq!(config.ramp_up_secs, 120, "default ramp 120s");
 assert_eq!(config.mode, Mode::DownloadAndUpload, "default mode is download+upload");
 assert_eq!(config.speed_mode, SpeedMode::Dynamic, "default speed_mode is dynamic");
 assert_eq!(config.download_bps, 1_048_576, "default download 1 MiB/s");
 assert!(config.freeze_on_zero_leechers, "default freeze on");
 assert!(config.forced_client.is_none(), "default forced_client is None (auto-probe)");
 }

 #[test]
 fn forced_client_serializes_and_deserializes() {
 let d = crate::config::test_helpers::defaults_cfg();
 let s = crate::config::test_helpers::swarm_defaults_cfg();
 let config = AuditConfig {
 forced_client: Some("-qB5220-".into()),
 ..AuditConfig::from_defaults(&d, &s)
 };
 let json = serde_json::to_string(&config).unwrap();
 assert!(json.contains("\"forced_client\":\"-qB5220-\""), "forced_client must serialize; got: {json}");
 let restored: AuditConfig = serde_json::from_str(&json).unwrap();
 assert_eq!(restored.forced_client.as_deref(), Some("-qB5220-"));
 }

 #[test]
 fn forced_client_defaults_to_none_when_absent_from_json() {
 // A config JSON from before forced_client was added must still deserialize.
 let d = crate::config::test_helpers::defaults_cfg();
 let s = crate::config::test_helpers::swarm_defaults_cfg();
 let config = AuditConfig::from_defaults(&d, &s);
 let mut json = serde_json::to_value(&config).unwrap();
 let obj = json.as_object_mut().unwrap();
 obj.remove("forced_client");
 let restored: AuditConfig = serde_json::from_value(json).unwrap();
 assert!(restored.forced_client.is_none(), "missing forced_client must default to None");
 }

 #[test]
 fn upload_only_mode_never_downloads() {
 let mut config = default_config();
 config.mode = Mode::UploadOnly;
 let mut state = PeerState { uploaded: 0, downloaded: 0, left: config.torrent_size };
 let elapsed = Duration::from_secs(200);
 // In upload-only mode, tick should never grow downloaded or shrink left
 let dl_before = state.downloaded;
 let left_before = state.left;
 for _ in 0..100 {
 tick(&mut state, elapsed, &config, &default_engine_cfg(), "seed", &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 });
 }
 assert_eq!(state.downloaded, dl_before, "downloaded must not grow in upload-only");
 assert_eq!(state.left, left_before, "left must not change in upload-only");
 assert!(state.uploaded > 0, "uploaded should still grow");
 }

 // start_download_pct clamping (underflow defense)

 #[test]
 fn initial_download_state_pct_zero_starts_from_scratch() {
 let (dl, left) = initial_download_state(1_073_741_824, 0);
 assert_eq!(dl, 0);
 assert_eq!(left, 1_073_741_824);
 }

 #[test]
 fn initial_download_state_pct_full_starts_as_seeder() {
 let (dl, left) = initial_download_state(1_073_741_824, 100);
 assert_eq!(dl, 1_073_741_824);
 assert_eq!(left, 0);
 }

 #[test]
 fn initial_download_state_half() {
 let (dl, left) = initial_download_state(1_000_000_000, 50);
 assert_eq!(dl, 500_000_000);
 assert_eq!(left, 500_000_000);
 }

 #[test]
 fn initial_download_state_clamps_pct_150_to_100() {
 let (dl, left) = initial_download_state(1_073_741_824, 150);
 assert_eq!(dl, 1_073_741_824, "dl must clamp to torrent_size");
 assert_eq!(left, 0, "left must not underflow");
 }

 #[test]
 fn initial_download_state_clamps_pct_max_u8() {
 let (dl, left) = initial_download_state(1_073_741_824, u8::MAX);
 assert_eq!(dl, 1_073_741_824);
 assert_eq!(left, 0);
 }

 // AuditConfig::validate

 #[test]
 fn validate_accepts_default_test_config() {
 let config = default_config();
 assert!(config.validate().is_ok(), "default test config should be valid");
 }

 #[test]
 fn validate_rejects_start_download_pct_over_100() {
 let mut config = default_config();
 config.start_download_pct = 200;
 let err = config.validate().unwrap_err();
 assert!(
 err.contains("start_download_pct"),
 "error should name the field: {err}"
 );
 }

 #[test]
 fn validate_rejects_jitter_pct_over_100() {
 let mut config = default_config();
 config.jitter_pct = 150;
 let err = config.validate().unwrap_err();
 assert!(err.contains("jitter_pct"), "error should name the field: {err}");
 }

 #[test]
 fn validate_rejects_zero_torrent_size() {
 let mut config = default_config();
 config.torrent_size = 0;
 assert!(config.validate().is_err());
 }

 // jittered_interval

 #[test]
 fn jitter_zero_pct_returns_exact_interval() {
 // jitter_pct = 0.0 must produce exactly `interval` - no random add.
 assert_eq!(jittered_interval(1800, 0.0), 1800);
 assert_eq!(jittered_interval(1, 0.0), 1);
 assert_eq!(jittered_interval(60, 0.0), 60);
 }

 #[test]
 fn jitter_respects_bounds() {
 // With 5% jitter on interval=1800, jitter_secs=90.
 // Result must be in [1800-90, 1800+90] = [1710, 1890].
 for _ in 0..1000 {
 let v = jittered_interval(1800, 5.0);
 assert!((1710..=1890).contains(&v), "jittered value {v} out of [1710, 1890]");
 }
 }

 #[test]
 fn jitter_floors_at_one_second() {
 // A tiny interval with huge jitter could go negative - must floor at 1.
 // interval=2, jitter_pct=1000.0 → jitter_secs=20, delta in [-20, 20].
 // interval + delta can be as low as 2-20 = -18, but floored to 1.
 for _ in 0..1000 {
 let v = jittered_interval(2, 1000.0);
 assert!(v >= 1, "jittered value {v} must be floored at 1");
 }
 }

 #[test]
 fn jitter_never_exceeds_interval_plus_jitter() {
 // interval=100, jitter_pct=10.0 → jitter_secs=10.
 // Upper bound: 100+10 = 110.
 for _ in 0..1000 {
 let v = jittered_interval(100, 10.0);
 assert!(v <= 110, "jittered value {v} exceeds interval+jitter (110)");
 }
 }

 // goal_required_bps

 fn goal(enabled: bool, direction: GoalDirection, target_bytes: u64, target_secs: u64) -> GoalConfig {
 // Map the single-target convenience arg to the right per-direction
 // field based on direction. For DownloadAndUpload, set the upload
 // target (use goal_du for both targets).
 let (up_t, dl_t) = match direction {
 GoalDirection::Upload | GoalDirection::DownloadAndUpload => (target_bytes, 0),
 };
 GoalConfig { enabled, direction, upload_target: up_t, download_target: dl_t, target_secs, reached_action: GoalReachedAction::Stop, reached_bps: 0 }
 }

 fn goal_du(enabled: bool, upload_target: u64, download_target: u64, target_secs: u64) -> GoalConfig {
 GoalConfig { enabled, direction: GoalDirection::DownloadAndUpload, upload_target, download_target, target_secs, reached_action: GoalReachedAction::Stop, reached_bps: 0 }
 }

 fn goal_with_action(enabled: bool, direction: GoalDirection, target_bytes: u64, target_secs: u64, action: GoalReachedAction, reached_bps: u64) -> GoalConfig {
 let (up_t, dl_t) = match direction {
 GoalDirection::Upload | GoalDirection::DownloadAndUpload => (target_bytes, 0),
 };
 GoalConfig { enabled, direction, upload_target: up_t, download_target: dl_t, target_secs, reached_action: action, reached_bps }
 }

 // goal_upload_target / goal_download_target / goal_reached

 #[test]
 fn goal_upload_target_returns_field_for_upload_directions() {
 let g = goal(true, GoalDirection::Upload, 1_000, 0);
 assert_eq!(goal_upload_target(&g), 1_000);
 let g = goal_du(true, 2_000, 3_000, 0);
 assert_eq!(goal_upload_target(&g), 2_000);
 }

 #[test]
 fn goal_upload_target_returns_zero_when_upload_field_zero() {
 let g = goal_du(true, 0, 1_000, 0);
 assert_eq!(goal_upload_target(&g), 0);
 }

 #[test]
 fn goal_download_target_returns_field_for_du() {
 let g = goal_du(true, 2_000, 1_000, 0);
 assert_eq!(goal_download_target(&g), 1_000);
 let g = goal_du(true, 2_000, 3_000, 0);
 assert_eq!(goal_download_target(&g), 3_000);
 }

 #[test]
 fn goal_download_target_returns_zero_for_upload_only() {
 let g = goal(true, GoalDirection::Upload, 1_000, 0);
 assert_eq!(goal_download_target(&g), 0);
 }

 #[test]
 fn goal_reached_false_when_disabled() {
 let g = goal(false, GoalDirection::Upload, 1_000, 0);
 let st = PeerState { uploaded: 2_000, downloaded: 0, left: 0 };
 assert!(!goal_reached(&st, &g), "disabled goal never reached");
 }

 #[test]
 fn goal_reached_false_when_both_targets_zero() {
 let g = GoalConfig { enabled: true, direction: GoalDirection::Upload, upload_target: 0, download_target: 0, target_secs: 0, reached_action: GoalReachedAction::Stop, reached_bps: 0 };
 let st = PeerState { uploaded: 2_000, downloaded: 0, left: 0 };
 assert!(!goal_reached(&st, &g), "zero-target goal never reached");
 }

 #[test]
 fn goal_reached_true_at_exact_upload_target() {
 let g = goal(true, GoalDirection::Upload, 1_000, 0);
 let st = PeerState { uploaded: 1_000, downloaded: 0, left: 0 };
 assert!(goal_reached(&st, &g));
 }

 #[test]
 fn goal_reached_true_when_overshot() {
 let g = goal_du(true, 0, 1_000, 0);
 let st = PeerState { uploaded: 0, downloaded: 1_500, left: 0 };
 assert!(goal_reached(&st, &g));
 }

 #[test]
 fn goal_reached_du_requires_both_targets() {
 let g = goal_du(true, 1_000, 500, 0);
 let st = PeerState { uploaded: 1_000, downloaded: 500, left: 0 };
 assert!(goal_reached(&st, &g), "both met → reached");
 let st = PeerState { uploaded: 1_000, downloaded: 400, left: 0 };
 assert!(!goal_reached(&st, &g), "only upload met → not reached");
 let st = PeerState { uploaded: 900, downloaded: 500, left: 0 };
 assert!(!goal_reached(&st, &g), "only download met → not reached");
 }

 #[test]
 fn goal_required_bps_forward_mode_returns_zero() {
 // target_secs == 0 means forward/ETA-only - no speed override.
 assert_eq!(goal_required_bps(1_000_000, 0, 0, Duration::from_secs(10)), 0);
 }

 #[test]
 fn goal_required_bps_target_reached_returns_zero() {
 assert_eq!(goal_required_bps(1_000_000, 1_000_000, 100, Duration::from_secs(10)), 0);
 assert_eq!(goal_required_bps(1_000_000, 2_000_000, 100, Duration::from_secs(10)), 0);
 }

 #[test]
 fn goal_required_bps_deadline_passed_returns_zero() {
 assert_eq!(goal_required_bps(1_000_000, 0, 100, Duration::from_secs(100)), 0);
 assert_eq!(goal_required_bps(1_000_000, 500_000, 100, Duration::from_secs(200)), 0);
 }

 #[test]
 fn goal_required_bps_even_split() {
 // 1_000_000 bytes in 1000s, elapsed 0 → 1000 B/s.
 assert_eq!(goal_required_bps(1_000_000, 0, 1000, Duration::from_secs(0)), 1000);
 }

 #[test]
 fn goal_required_bps_ceil_division_no_undershoot() {
 // 1_000_000 bytes, 999s left → ceil(1000000/999) = 1002 B/s.
 assert_eq!(goal_required_bps(1_000_000, 0, 1000, Duration::from_secs(1)), 1002);
 }

 #[test]
 fn goal_required_bps_shrinks_as_progress_grows() {
 // Halfway: 500_000 left, 500s left → 1000 B/s (same avg).
 assert_eq!(goal_required_bps(1_000_000, 500_000, 1000, Duration::from_secs(500)), 1000);
 // Behind schedule: 800_000 left, 100s left → 8000 B/s (speed up).
 assert_eq!(goal_required_bps(1_000_000, 200_000, 1000, Duration::from_secs(900)), 8000);
 }

 #[test]
 fn goal_required_bps_works_for_download_target_too() {
 // Same helper for the download direction's target.
 assert_eq!(goal_required_bps(2_000_000, 1_000_000, 1000, Duration::from_secs(500)), 2000);
 }

 // tick: goal live feedback

 fn seed_tick_with_goal(state: &mut PeerState, elapsed: Duration, config: &AuditConfig) -> TickSpeeds {
 tick(state, elapsed, config, &default_engine_cfg(), "seed",
 &TickContext { leecher_count: 5, seeder_count: 10, dynamic_target_bps: 0, dynamic_download_bps: 0 })
 }

 #[test]
 fn goal_upload_override_raises_speed_in_seed() {
 // Huge target (1 TiB) over 1h, sampled at elapsed=500s (3100s left) →
 // required ≈ 354 MB/s, far above the default 512 KiB/s. The target is
 // never approached in 2000 ticks, so the override stays active throughout.
 let mut config = default_config();
 config.goal = goal(true, GoalDirection::Upload, 1_099_511_627_776, 3600);
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500); // past ramp-up, 3100s left
 for _ in 0..2000 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 // ~345 MB/s over ~1400 non-choke ticks ≈ 480 GiB; assert well over 50 GiB
 // (the default rate would only reach ~734 MiB).
 assert!(state.uploaded > 50_000_000_000, "goal override should raise upload speed: got {}", state.uploaded);
 }

 #[test]
 fn goal_max_upload_bps_caps_override() {
 // Same huge target, but max_upload_bps=600_000 caps the effective speed.
 let mut config = default_config();
 config.swarm.max_upload_bps = 600_000;
 config.goal = goal(true, GoalDirection::Upload, 1_099_511_627_776, 3600);
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500);
 for _ in 0..2000 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 // Capped at 600_000 over ~1400 ticks ≈ 840 MiB; the uncapped override
 // would be ~480 GiB, so assert the cap held (under 1.05 GiB).
 assert!(state.uploaded < 1_050_000_000, "max_upload_bps should cap goal override: got {}", state.uploaded);
 }

 #[test]
 fn goal_disabled_matches_default_speed() {
 // With goal disabled, 2000 seed ticks at the default 524288 B/s over
 // ~1400 non-choke ticks ≈ 734 MiB (±jitter). Clearly below the 2×
 // override (~1.47 GiB) from the test above.
 let config = default_config(); // goal.enabled = false
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500);
 for _ in 0..2000 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 assert!(state.uploaded > 500_000_000 && state.uploaded < 1_000_000_000,
 "disabled goal should match default speed: got {}", state.uploaded);
 }

 #[test]
 fn goal_target_reached_drops_override() {
 // Already uploaded the target → required 0 → no override → default speed
 // (same range as the disabled test, NOT the 2× override range).
 let mut config = default_config();
 config.goal = goal_with_action(true, GoalDirection::Upload, 1_000, 50, GoalReachedAction::ContinueInitial, 0);
 let mut state = PeerState { uploaded: 1_000, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500);
 let before = state.uploaded;
 for _ in 0..2000 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 let delta = state.uploaded - before;
 assert!(delta > 500_000_000 && delta < 1_000_000_000, "ContinueInitial resumes default speed: got {delta}");
 }

 #[test]
 fn goal_continue_custom_overrides_to_custom_speed() {
 // Target reached + ContinueCustom → grow at reached_bps (200 KiB/s),
 // not the default 512 KiB/s and not the reverse-mode required rate.
 let mut config = default_config();
 config.goal = goal_with_action(true, GoalDirection::Upload, 1_000, 50, GoalReachedAction::ContinueCustom, 204_800);
 let mut state = PeerState { uploaded: 1_000, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500);
 let before = state.uploaded;
 for _ in 0..2000 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 let delta = state.uploaded - before;
 // 204_800 B/s over ~1400 non-choke ticks ≈ 286 MiB. The default 512
 // KiB/s would reach ~734 MiB, so assert the custom rate held.
 assert!(delta > 150_000_000 && delta < 450_000_000, "ContinueCustom should grow at reached_bps: got {delta}");
 }

 #[test]
 fn goal_continue_custom_zero_freezes_counter() {
 // reached_bps = 0 → freeze: counter does not grow at all.
 let mut config = default_config();
 config.goal = goal_with_action(true, GoalDirection::Upload, 1_000, 50, GoalReachedAction::ContinueCustom, 0);
 let mut state = PeerState { uploaded: 1_000, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500);
 let before = state.uploaded;
 for _ in 0..2000 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 assert_eq!(state.uploaded, before, "reached_bps=0 must freeze the counter");
 }

 #[test]
 fn goal_continue_custom_respects_max_upload_cap() {
 // reached_bps = 600_000 but max_upload_bps = 300_000 → effective 300_000.
 let mut config = default_config();
 config.swarm.max_upload_bps = 300_000;
 config.goal = goal_with_action(true, GoalDirection::Upload, 1_000, 50, GoalReachedAction::ContinueCustom, 600_000);
 let mut state = PeerState { uploaded: 1_000, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500);
 for _ in 0..2000 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 // Capped at 300_000 over ~1400 ticks ≈ 420 MiB; uncapped 600_000 would
 // be ~840 MiB. Assert the cap held (under 500 MiB).
 assert!(state.uploaded < 500_000_000, "max_upload_bps should cap ContinueCustom: got {}", state.uploaded);
 }

 #[test]
 fn goal_stop_action_uses_required_rate_on_reaching_ticks() {
 // With Stop, the reached branch leaves the base speed as-is and the
 // loop breaks after the tick. Before reaching, the reverse-mode
 // required rate applies (1 TiB / 1h ≈ 345 MB/s). Run several ticks
 // to average out the 30% burst-choke; assert growth is large.
 let mut config = default_config();
 config.goal = goal_with_action(true, GoalDirection::Upload, 1_099_511_627_776, 3600, GoalReachedAction::Stop, 0);
 let mut state = PeerState { uploaded: 0, downloaded: config.torrent_size, left: 0 };
 let elapsed = Duration::from_secs(500);
 for _ in 0..20 {
 seed_tick_with_goal(&mut state, elapsed, &config);
 }
 assert!(state.uploaded > 1_000_000_000, "Stop pre-reach ticks should grow at required rate: got {}", state.uploaded);
 }

 // GoalConfig::validate

 #[test]
 fn goal_validate_rejects_upload_target_over_max() {
 let g = goal(true, GoalDirection::Upload, crate::data::units::GOAL_MAX_TARGET_BYTES + 1, 100);
 let err = g.validate().unwrap_err();
 assert!(err.contains("upload_target"), "error should name the field: {err}");
 }

 #[test]
 fn goal_validate_rejects_download_target_over_max() {
 let g = goal_du(true, 1_000, crate::data::units::GOAL_MAX_TARGET_BYTES + 1, 100);
 let err = g.validate().unwrap_err();
 assert!(err.contains("download_target"), "error should name the field: {err}");
 }

 #[test]
 fn goal_validate_accepts_forward_mode_zero_secs() {
 let g = goal(true, GoalDirection::DownloadAndUpload, 1_000, 0);
 assert!(g.validate().is_ok(), "forward mode (target_secs=0) should be valid");
 }

 #[test]
 fn audit_validate_propagates_goal_error() {
 let mut config = default_config();
 config.goal = goal(true, GoalDirection::Upload, crate::data::units::GOAL_MAX_TARGET_BYTES + 1, 100);
 let err = config.validate().unwrap_err();
 assert!(err.starts_with("goal."), "error should be prefixed with goal.: {err}");
 }

 // AuditConfig serde forward-compatibility
 // Regression: a config_json stored before the `goal` field existed has no
 // `goal` key. Without `#[serde(default)]` on `AuditConfig.goal`, the
 // deserialize falls back to `from_defaults` and silently resets the task's
 // mode/speed/etc. to `[defaults]` on restart. The fix: `goal` (and the
 // later-added `reached_action`/`reached_bps`) carry serde defaults so old
 // JSON deserializes with a disabled goal and the stored mode preserved.

 #[test]
 fn audit_config_deserializes_old_json_without_goal_preserving_mode() {
 // A pre-goal-feature config: UploadOnly + no `goal` key. Mirrors what
 // was stored before the goal feature shipped.
 let old_json = r#"{"announce_url":"","info_hash":"","torrent_size":1024,"upload_bps":524288,"jitter_pct":20,"ramp_up_secs":120,"mode":"upload_only","download_bps":1048576,"freeze_on_zero_leechers":true,"freeze_on_zero_seeders":true,"start_download_pct":0,"speed_mode":"fixed","swarm":{"avg_leecher_download_bps":3000000,"seed_share_factor":0.8,"fair_share_multiplier":1.0,"max_upload_bps":0,"max_download_bps":0}}"#;
 let config: AuditConfig = serde_json::from_str(old_json).expect("old config without goal must deserialize");
 assert_eq!(config.mode, Mode::UploadOnly, "stored mode must survive restart, not reset to DownloadAndUpload");
 assert!(!config.goal.enabled, "missing goal key → disabled goal");
 assert_eq!(config.goal.reached_action, GoalReachedAction::Stop, "missing reached_action → Stop default");
 assert_eq!(config.goal.reached_bps, 0, "missing reached_bps → 0 default");
 assert_eq!(config.goal.download_target, 0, "missing download_target → 0 default");
 }

 #[test]
 fn audit_config_deserializes_mid_json_without_reached_fields() {
 // A config stored between goal iterations: has `goal` with the old
 // single-target `target_bytes` field but no `reached_action`/
 // `reached_bps`/`download_target` keys. The old `target_bytes` is
 // silently dropped (renamed to `upload_target`) - serde ignores
 // unknown fields. The stored mode survives (the key invariant).
 let mid_json = r#"{"announce_url":"","info_hash":"","torrent_size":1024,"upload_bps":524288,"jitter_pct":20,"ramp_up_secs":120,"mode":"download_and_upload","download_bps":1048576,"freeze_on_zero_leechers":true,"freeze_on_zero_seeders":true,"start_download_pct":0,"speed_mode":"dynamic","swarm":{"avg_leecher_download_bps":3000000,"seed_share_factor":0.8,"fair_share_multiplier":1.0,"max_upload_bps":0,"max_download_bps":0},"goal":{"enabled":true,"direction":"upload","target_bytes":1048576,"target_secs":3600}}"#;
 let config: AuditConfig = serde_json::from_str(mid_json).expect("mid config without reached fields must deserialize");
 assert_eq!(config.mode, Mode::DownloadAndUpload, "stored mode must survive");
 assert!(config.goal.enabled, "goal.enabled preserved");
 // The old `target_bytes` field was renamed to `upload_target`; serde
 // ignores the unknown `target_bytes` and defaults `upload_target` to 0.
 assert_eq!(config.goal.upload_target, 0, "renamed field → 0 default");
 assert_eq!(config.goal.reached_action, GoalReachedAction::Stop, "missing reached_action → Stop default");
 assert_eq!(config.goal.reached_bps, 0, "missing reached_bps → 0 default");
 }

 #[test]
 fn audit_config_round_trips_full_goal_through_json() {
 let mut config = default_config();
 config.mode = Mode::UploadOnly;
 config.goal = goal_du(true, 2_097_152, 1_048_576, 7200);
 config.goal.reached_action = GoalReachedAction::ContinueCustom;
 config.goal.reached_bps = 524_288;
 let json = serde_json::to_string(&config).unwrap();
 let back: AuditConfig = serde_json::from_str(&json).unwrap();
 assert_eq!(back.mode, Mode::UploadOnly, "mode round-trips");
 assert_eq!(back.goal, config.goal, "goal round-trips exactly");
 }
}
