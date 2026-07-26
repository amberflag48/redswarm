//! Configuration for RedSwarm.
//!
//! All tunable values live in [`AppConfig`], grouped into focused sub-structs.
//! Values are loaded from a TOML file whose path comes from the
//! `REDSWARM_CONFIG` environment variable (default `config.toml`).
//!
//! There are **no built-in defaults**: every value must be present and valid in
//! the config file. A missing file, a parse error, or a failed validation is a
//! hard error - [`load()`] returns `Err` and the application refuses to start.
//!
//! `DefaultsConfig` reuses the `Mode`/`SpeedMode` enums from [`crate::engine`]
//! so the per-audit default values round-trip through TOML without any string
//! validation - serde rejects unknown enum variants at parse time.
//!
//! Binary byte units (KiB/MiB/GiB) are physical constants and live in
//! [`crate::data::units`], not here.

use std::collections::BTreeMap;

use rand::Rng;

/// Tokio broadcast channel capacity for audit event fan-out (internal plumbing).
pub const BROADCAST_CHANNEL_CAPACITY: usize = 128;

/// Per-client SSE channel capacity (internal plumbing).
pub const SSE_CHANNEL_CAPACITY: usize = 64;

// Server

/// HTTP server bind address, database URL, and log filter.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
 /// Socket address the web UI listens on.
 pub bind_addr: String,
 /// SQLx database URL (SQLite by default).
 pub db_url: String,
 /// `tracing` env-filter directive controlling log verbosity.
 pub log_filter: String,
 /// Seconds to wait before retrying an HTTP listener rebind after a bind
 /// failure (e.g. a hot-reloaded `bind_addr` is already in use). Prevents a
 /// hot spin loop; the loop falls back to the last known-good address
 /// meanwhile. Also used when the fallback bind itself fails.
 pub rebind_retry_secs: u64,
 /// Interval (seconds) between SSE keep-alive comment frames sent on the
 /// global `/api/events` stream. Keeps idle connections alive so
 /// intermediaries (proxies, load balancers, browsers) don't drop a quiet
 /// stream during periods with no running audits. Well under typical idle
 /// timeouts; a comment frame is ignored by the EventSource parser.
 pub sse_keepalive_secs: u64,
}

impl ServerConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(!self.bind_addr.trim().is_empty(), "server.bind_addr must not be empty");
 self.bind_addr
 .trim()
 .parse::<std::net::SocketAddr>()
 .map_err(|e| anyhow::anyhow!("server.bind_addr `{}` is not a valid socket address: {e}", self.bind_addr.trim()))?;
 anyhow::ensure!(!self.db_url.trim().is_empty(), "server.db_url must not be empty");
 anyhow::ensure!(!self.log_filter.trim().is_empty(), "server.log_filter must not be empty");
 anyhow::ensure!(self.rebind_retry_secs >= 1, "server.rebind_retry_secs must be >= 1");
 anyhow::ensure!(self.sse_keepalive_secs >= 1, "server.sse_keepalive_secs must be >= 1");
 Ok(())
 }
}

// HTTP

/// HTTP client tuning for the announce client.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HttpConfig {
 /// Per-request timeout, in seconds, for tracker announce calls.
 pub timeout_secs: u64,
}

impl HttpConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.timeout_secs >= 1, "http.timeout_secs must be >= 1");
 Ok(())
 }
}

// Tracker

/// Tracker-facing announce parameters.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackerConfig {
 /// TCP port advertised to the tracker for incoming peer connections.
 pub peer_port: u16,
 /// Announce interval (seconds) used when the tracker omits one.
 pub default_interval_secs: u32,
 /// Lower bound (seconds) clamping the tracker-supplied interval.
 pub min_interval_secs: u32,
 /// Upper bound (seconds) clamping any tracker-supplied interval.
 pub max_interval_secs: u32,
}

impl TrackerConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.peer_port > 0, "tracker.peer_port must be > 0");
 anyhow::ensure!(self.min_interval_secs >= 1, "tracker.min_interval_secs must be >= 1");
 anyhow::ensure!(
 self.default_interval_secs >= self.min_interval_secs,
 "tracker.default_interval_secs must be >= tracker.min_interval_secs"
 );
 anyhow::ensure!(
 self.max_interval_secs > self.min_interval_secs,
 "tracker.max_interval_secs must be > tracker.min_interval_secs"
 );
 Ok(())
 }
}

// Engine

/// Engine loop timing and behavioral knobs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EngineConfig {
 /// Seconds between engine ticks (event-loop wakeups).
 pub tick_interval_secs: u64,
 /// Seconds between persisted statistic snapshots.
 pub stat_interval_secs: u64,
 /// Random jitter applied to the announce interval, as a percent.
 pub announce_jitter_pct: f64,
 /// Fraction of a leecher's upload bandwidth a faked peer claims to serve.
 pub leech_upload_factor: f64,
 /// Probability of choking a peer during a burst to mimic client behavior.
 pub burst_choke_probability: f64,
 /// Seconds to wait for a running task to flush its `stopped` announce
 /// when an edit or delete request arrives. The task is cancelled first,
 /// then we wait for the engine to finish before applying the change.
 pub stop_grace_secs: u64,
}

impl EngineConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.tick_interval_secs >= 1, "engine.tick_interval_secs must be >= 1");
 anyhow::ensure!(self.stat_interval_secs >= 1, "engine.stat_interval_secs must be >= 1");
 anyhow::ensure!(self.announce_jitter_pct >= 0.0, "engine.announce_jitter_pct must be >= 0.0");
 anyhow::ensure!(self.announce_jitter_pct <= 100.0, "engine.announce_jitter_pct must be <= 100.0");
 anyhow::ensure!(
 self.leech_upload_factor >= 0.0 && self.leech_upload_factor <= 1.0,
 "engine.leech_upload_factor must be in [0.0, 1.0]"
 );
 anyhow::ensure!(
 self.burst_choke_probability >= 0.0 && self.burst_choke_probability <= 1.0,
 "engine.burst_choke_probability must be in [0.0, 1.0]"
 );
 anyhow::ensure!(self.stop_grace_secs >= 1, "engine.stop_grace_secs must be >= 1");
 Ok(())
 }
}

// Database

/// SQLite connection-pool sizing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DatabaseConfig {
 /// Maximum number of connections kept in the SQLx pool.
 pub max_connections: u32,
}

impl DatabaseConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.max_connections >= 1, "database.max_connections must be >= 1");
 Ok(())
 }
}

// UI

/// Web UI display limits.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UiConfig {
 /// Number of most-recent events retained for the log/SSE stream.
 pub event_log_limit: u32,
}

impl UiConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.event_log_limit >= 1, "ui.event_log_limit must be >= 1");
 Ok(())
 }
}

// Per-audit defaults

/// Per-audit defaults applied when creating a new audit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DefaultsConfig {
 /// Default transfer mode (download+upload lifecycle or upload-only ghost seed).
 pub mode: crate::engine::Mode,
 /// Default speed strategy (fixed manual or dynamic swarm-aware).
 pub speed_mode: crate::engine::SpeedMode,
 /// Default fake upload rate, in bytes per second.
 pub upload_bps: u64,
 /// Default fake download rate, in bytes per second.
 pub download_bps: u64,
 /// Default per-announce rate jitter, in percent.
 pub jitter_pct: u32,
 /// Seconds over which rates ramp from zero to their target.
 pub ramp_up_secs: u64,
    /// Starting download-progress percentage (0-100).
 pub start_download_pct: u32,
 /// Freeze the swarm when the last leecher disappears.
 pub freeze_on_zero_leechers: bool,
 /// Freeze the swarm when the last seeder disappears.
 pub freeze_on_zero_seeders: bool,
 /// Per-audit goal default: whether new tasks start with a goal active.
 pub goal_enabled: bool,
 /// Per-audit goal default: which counter(s) the goal tracks.
 pub goal_direction: crate::engine::GoalDirection,
 /// Per-audit goal default: upload target in bytes (0 = degenerate).
 pub goal_upload_target: u64,
 /// Per-audit goal default: download target in bytes (0 = degenerate).
 /// Only used when `goal_direction` is `download_and_upload`.
 #[serde(default)]
 pub goal_download_target: u64,
 /// Per-audit goal default: deadline in seconds from start. 0 = forward /
 /// ETA-only mode (no speed adjustment); > 0 = reverse mode (live feedback).
 pub goal_target_secs: u64,
 /// Per-audit goal default: what to do once the target is reached.
 pub goal_reached_action: crate::engine::GoalReachedAction,
 /// Per-audit goal default: custom speed (bytes/sec) for
 /// `GoalReachedAction::ContinueCustom`. 0 = freeze the counter.
 pub goal_reached_bps: u64,
}

impl DefaultsConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.upload_bps >= 1, "defaults.upload_bps must be >= 1");
 anyhow::ensure!(self.download_bps >= 1, "defaults.download_bps must be >= 1");
 anyhow::ensure!(self.jitter_pct <= crate::data::units::PERCENT, "defaults.jitter_pct must be <= 100");
 anyhow::ensure!(self.ramp_up_secs >= 1, "defaults.ramp_up_secs must be >= 1");
 anyhow::ensure!(self.ramp_up_secs <= crate::data::units::SECS_PER_DAY, "defaults.ramp_up_secs must be <= 86400 (24h)");
 anyhow::ensure!(self.start_download_pct <= crate::data::units::PERCENT, "defaults.start_download_pct must be <= 100");
 anyhow::ensure!(
 self.goal_upload_target <= crate::data::units::GOAL_MAX_TARGET_BYTES,
 "defaults.goal_upload_target must be <= {}",
 crate::data::units::GOAL_MAX_TARGET_BYTES
 );
 anyhow::ensure!(
 self.goal_download_target <= crate::data::units::GOAL_MAX_TARGET_BYTES,
 "defaults.goal_download_target must be <= {}",
 crate::data::units::GOAL_MAX_TARGET_BYTES
 );
 anyhow::ensure!(
 self.goal_target_secs <= crate::data::units::GOAL_MAX_TIME_SECS,
 "defaults.goal_target_secs must be <= {}",
 crate::data::units::GOAL_MAX_TIME_SECS
 );
 Ok(())
 }
}

// Swarm defaults

/// Swarm-simulation defaults shared across audits.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SwarmDefaultsConfig {
 /// Assumed average download rate of a real leecher, in bytes per second.
 pub avg_leecher_download_bps: u64,
 /// Share of downloaded data a faked seeder reports as uploaded.
 pub seed_share_factor: f64,
 /// Multiplier applied when distributing bandwidth fairly across peers.
 pub fair_share_multiplier: f64,
 /// Per-peer upload cap, in bytes per second; 0 means unlimited.
 pub max_upload_bps: u64,
 /// Per-peer download cap, in bytes per second; 0 means unlimited.
 pub max_download_bps: u64,
}

impl SwarmDefaultsConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(
 self.avg_leecher_download_bps >= 1,
 "swarm_defaults.avg_leecher_download_bps must be >= 1"
 );
 anyhow::ensure!(
 self.seed_share_factor > crate::data::units::SEED_SHARE_FACTOR_MIN
 && self.seed_share_factor <= crate::data::units::SEED_SHARE_FACTOR_MAX,
 "swarm_defaults.seed_share_factor must be in (0.0, 1.0]"
 );
 anyhow::ensure!(
 self.fair_share_multiplier >= 0.0,
 "swarm_defaults.fair_share_multiplier must be >= 0.0"
 );
 Ok(())
 }
}

// Client emulation

/// Per-client tracker `key` parameter format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyFormat {
 /// 8 lowercase hex chars (`%08x`) - rTorrent (libtorrent-rakshasa).
 LowerHex,
 /// 8 uppercase hex chars (`%08X`) - libtorrent-rasterbar (qBittorrent,
 /// Deluge), Transmission, uTorrent, BitTorrent.
 UpperHex,
 /// Decimal digits - no known major client uses this format, but BEP-3
 /// allows any value. Kept for completeness and custom client emulation.
 Decimal,
}

impl KeyFormat {
 /// Generate a random key string in this format.
 pub fn generate(&self) -> String {
 let mut rng = rand::rng();
 match self {
 Self::LowerHex => format!("{:08x}", rng.random::<u32>()),
 Self::UpperHex => format!("{:08X}", rng.random::<u32>()),
 Self::Decimal => format!("{}", rng.random::<u32>()),
 }
 }
}

/// A single emulated BitTorrent client definition.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClientSpecConfig {
 /// Client name, e.g. `"qBittorrent"`.
 pub label: String,
 /// Client version, e.g. `"5.2.2"`.
 pub version: String,
 /// Peer-ID prefix emitted by this client, e.g. `"-qB5220-"`. **This is the
 /// unique identity key** - no two clients may share the same prefix.
 pub peer_id_prefix: String,
 /// HTTP `User-Agent` header value, e.g. `"qBittorrent/5.2.2"`.
 pub user_agent: String,
 /// Announce URL query template with `{placeholder}` tokens.
 pub query: String,
 /// Number of peers requested per announce (`numwant`).
 pub numwant: u32,
 /// Alternative names accepted for client selection.
 pub aliases: Vec<String>,
 /// Peer-wire handshake reserved bytes (8 bytes, hex-encoded, e.g. `"0000000000100005"`).
 /// Determines which extensions the client advertises (DHT, LTEP, Fast Ext, etc.).
 pub reserved_bytes: String,
 /// Whether this client uses Fast Extension `have_all` (BEP-6) instead of a
 /// full bitfield when seeding. `true` for libtorrent-rasterbar, Transmission,
 /// µTorrent, Vuze. `false` for rTorrent (libtorrent-rakshasa, no Fast Ext).
 pub fast_extension: bool,
 /// Keepalive interval in seconds for the peer-wire connection.
 pub keepalive_secs: u64,
 /// BEP-10 extension handshake `v` field - the client name + version string
 /// sent in the LTEP handshake (e.g. `"qBittorrent/5.2.2"`, `"Transmission 4.1.2"`).
 /// BEP-10 calls this "a much more reliable way of identifying the client than
 /// relying on the peer id encoding."
 pub v_string: String,
 /// BEP-10 extension handshake `m` dict - extension name → local message ID.
 /// Keys are BEP-10 extension names (`ut_metadata`, `ut_pex`, `upload_only`,
 /// `lt_donthave`, `ut_holepunch`, …). Values are small positive integers the
 /// client assigns as local message IDs. An empty dict means no extensions
 /// are advertised (unusual for real clients).
 pub m_dict: BTreeMap<String, u32>,
 /// BEP-10 extension handshake `reqq` field - the maximum number of
 /// outstanding piece requests the client supports without dropping any
 /// (e.g. 2000 for libtorrent-rasterbar, 500 for Transmission, 2048 for
 /// libtorrent-rakshasa). `None` = omit the field entirely (Vuze doesn't
 /// send reqq).
 #[serde(default)]
 pub reqq: Option<u32>,
 /// BEP-10 `e` field - encryption preference. `None` = omit the field
 /// entirely (e.g. Transmission always sends `e=1`, qBittorrent sends when
 /// encryption is enabled, some clients never send it).
 #[serde(default)]
 pub encryption_preferred: Option<bool>,
 /// Whether to send `upload_only: 1` in the ext handshake. Real seeders
 /// always set this (BEP-21); redswarm emulates a seeder so this is `true`
 /// for every configured client.
 pub send_upload_only: bool,
 /// BEP-10 `complete_ago` field - seconds since torrent completed.
 /// `None` = omit (Transmission doesn't send this). `Some(-1)` = never seen
 /// complete (libtorrent default for a fresh seeder).
 #[serde(default)]
 pub send_complete_ago: Option<i64>,
 /// Whether to send `yourip` in the ext handshake. All BEP-10 clients send
 /// this; `true` for every configured client.
 pub send_yourip: bool,
 /// Tracker `key` parameter format - per-client encoding.
 pub key_format: KeyFormat,
}

impl ClientSpecConfig {
 /// Human-readable display name: `"{label} - {version} ({peer_id_prefix})"`.
 /// Used in logs, SSE events, the task list, and the settings card title.
 pub fn display_name(&self) -> String {
 format!("{} - {} ({})", self.label, self.version, self.peer_id_prefix)
 }

 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(!self.label.trim().is_empty(), "clients[].label must not be empty");
 anyhow::ensure!(!self.version.trim().is_empty(), "clients[].version must not be empty (client `{}`)", self.display_name());
 anyhow::ensure!(!self.peer_id_prefix.trim().is_empty(), "clients[].peer_id_prefix must not be empty (client `{}`)", self.display_name());
 anyhow::ensure!(
 self.peer_id_prefix.len() <= crate::data::protocol::PEER_ID_PREFIX_MAX_LEN,
 "clients[].peer_id_prefix must be at most {} chars (client `{}`)",
 crate::data::protocol::PEER_ID_PREFIX_MAX_LEN, self.display_name()
 );
 anyhow::ensure!(!self.user_agent.trim().is_empty(), "clients[].user_agent must not be empty (client `{}`)", self.display_name());
 anyhow::ensure!(!self.query.trim().is_empty(), "clients[].query must not be empty (client `{}`)", self.display_name());
 anyhow::ensure!(self.query.contains(crate::data::protocol::Q_INFO_HASH), "clients[].query must contain `{{info_hash}}` (client `{}`)", self.display_name());
 anyhow::ensure!(self.query.contains(crate::data::protocol::Q_PEER_ID), "clients[].query must contain `{{peer_id}}` (client `{}`)", self.display_name());
 anyhow::ensure!(self.numwant > 0, "clients[].numwant must be > 0 (client `{}`)", self.display_name());
 let reserved = crate::bencode::hex_decode(&self.reserved_bytes)
 .map_err(|e| anyhow::anyhow!("clients[].reserved_bytes must be hex (client `{}`): {e}", self.display_name()))?;
 anyhow::ensure!(reserved.len() == crate::data::protocol::RESERVED_LEN, "clients[].reserved_bytes must decode to {} bytes (client `{}`)", crate::data::protocol::RESERVED_LEN, self.display_name());
 let fast_ext_bit = reserved[crate::data::protocol::FAST_EXT_BYTE_INDEX] & crate::data::protocol::FAST_EXT_BIT_MASK != 0;
 anyhow::ensure!(
 fast_ext_bit == self.fast_extension,
 "clients[].fast_extension ({}) contradicts reserved_bytes Fast Extension bit ({}) (client `{}`)",
 self.fast_extension, fast_ext_bit, self.display_name()
 );
 anyhow::ensure!(self.keepalive_secs >= 1, "clients[].keepalive_secs must be >= 1 (client `{}`)", self.display_name());
 anyhow::ensure!(!self.v_string.trim().is_empty(), "clients[].v_string must not be empty (client `{}`)", self.display_name());
 if let Some(reqq) = self.reqq {
 anyhow::ensure!(reqq > 0, "clients[].reqq must be > 0 (client `{}`)", self.display_name());
 }
 for (name, &id) in &self.m_dict {
 anyhow::ensure!(id > 0, "clients[].m_dict `{name}` must be > 0 (client `{}`)", self.display_name());
 }
 if let Some(ago) = self.send_complete_ago {
 anyhow::ensure!(ago >= -1, "clients[].send_complete_ago must be >= -1 (client `{}`)", self.display_name());
 }
 Ok(())
 }
}

/// The full set of emulated clients (`[[clients]]` array of tables in TOML).
pub type ClientsConfig = Vec<ClientSpecConfig>;

// Peer server

/// Peer-wire server configuration - listens on `peer_port` to accept inbound
/// BitTorrent peer connections, complete the handshake, and keep connections
/// alive without serving data. Makes the emulated peer "connectable".
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerServerConfig {
 /// Whether to enable the peer-wire listener. When disabled, the advertised
 /// `peer_port` won't accept connections (non-connectable - a detection signal).
 pub enabled: bool,
 /// Maximum concurrent peer connections. Each connection uses ~2-5 KB.
 /// 10,000 connections ≈ 130 MB. Must be ≤ `ulimit -n` minus ~50 overhead.
 pub max_connections: usize,
 /// Maximum concurrent connections per IP address (DoS flood defense).
 pub max_per_ip: u32,
 /// Timeout for receiving the 68-byte handshake after TCP connect (seconds).
 /// Defends against slow-loris attacks.
 pub handshake_timeout_secs: u64,
 /// Timeout for writing our handshake/bitfield/unchoke/keepalive (seconds).
 pub write_timeout_secs: u64,
 /// Drop connection if no data received for this long (seconds).
 pub idle_timeout_secs: u64,
 /// Timeout for reading a single message body (seconds). A peer that sends
 /// a header then stalls is a slow-loris variant.
 pub body_read_timeout_secs: u64,
 /// Backoff after an accept error (milliseconds). Prevents a hot spin loop
 /// when the OS returns EMFILE (fd exhaustion) or similar.
 pub accept_error_backoff_ms: u64,
 /// Keepalive interval for capture-mode peer connections (seconds). Used
 /// when the peer_server accepts a connection for a fingerprint capture
 /// session, which doesn't have a `ClientSpecConfig` to provide the
 /// interval.
 pub capture_keepalive_secs: u64,
}

impl PeerServerConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.max_connections >= 1, "peer_server.max_connections must be >= 1");
 anyhow::ensure!(self.max_per_ip >= 1, "peer_server.max_per_ip must be >= 1");
 anyhow::ensure!(self.handshake_timeout_secs >= 1, "peer_server.handshake_timeout_secs must be >= 1");
 anyhow::ensure!(self.write_timeout_secs >= 1, "peer_server.write_timeout_secs must be >= 1");
 anyhow::ensure!(self.idle_timeout_secs >= 1, "peer_server.idle_timeout_secs must be >= 1");
 anyhow::ensure!(self.body_read_timeout_secs >= 1, "peer_server.body_read_timeout_secs must be >= 1");
 anyhow::ensure!(self.capture_keepalive_secs >= 1, "peer_server.capture_keepalive_secs must be >= 1");
 anyhow::ensure!(self.accept_error_backoff_ms >= 1, "peer_server.accept_error_backoff_ms must be >= 1");
 Ok(())
 }
}

// NAT

/// NAT-PMP port forwarding settings for VPN setups (e.g. ProtonVPN WireGuard).
///
/// When `gateway_ip` is non-empty, the app queries the NAT-PMP gateway at
/// startup to discover the public IP + public port, then uses them for
/// tracker announces and capture-mode peer advertising. The `[tracker]
/// peer_port` becomes the internal listening port (what the peer-wire
/// server binds). When `gateway_ip` is empty, NAT-PMP is disabled and
/// `[tracker] peer_port` is used for both listening and announcing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NatConfig {
 /// NAT-PMP gateway IP (e.g. ProtonVPN WireGuard = `"10.2.0.1"`).
 /// Empty = disabled (use `[tracker] peer_port` directly).
 pub gateway_ip: String,
 /// Lease lifetime (seconds) requested from the gateway per RFC 6886.
 /// ProtonVPN grants 60 s; the gateway may grant less.
 pub lease_lifetime_secs: u32,
 /// Seconds between lease-renewal attempts. Must be less than
 /// `lease_lifetime_secs` so the lease is refreshed before it lapses.
 /// ProtonVPN's official `natpmpc` loop uses 45 s.
 pub renew_interval_secs: u64,
}

impl NatConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.lease_lifetime_secs >= 1, "nat.lease_lifetime_secs must be >= 1");
 anyhow::ensure!(self.renew_interval_secs >= 1, "nat.renew_interval_secs must be >= 1");
 anyhow::ensure!(
 self.renew_interval_secs < u64::from(self.lease_lifetime_secs),
 "nat.renew_interval_secs must be less than nat.lease_lifetime_secs (renew before the lease lapses)"
 );
 if self.gateway_ip.trim().is_empty() {
 return Ok(());
 }
 self.gateway_ip
 .trim()
 .parse::<std::net::IpAddr>()
 .map_err(|e| anyhow::anyhow!("nat.gateway_ip must be a valid IP address: {e}"))?;
 Ok(())
 }
}

// Hot-reload watcher

/// File-watcher tuning for hot-reloading `config.toml`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WatcherConfig {
 /// Quiet period (milliseconds) after the last filesystem event before a
 /// reload is triggered. Editors often write in several quick steps
 /// (write, fsync, rename); this collapses a burst into a single reload.
 /// Raise it on slow filesystems or editors that write in many passes;
 /// lower it for snappier reloads.
 pub debounce_ms: u64,
}

impl WatcherConfig {
 pub fn validate(&self) -> anyhow::Result<()> {
 anyhow::ensure!(self.debounce_ms >= 1, "watcher.debounce_ms must be >= 1");
 anyhow::ensure!(
 self.debounce_ms <= crate::data::units::DEBOUNCE_MS_MAX,
 "watcher.debounce_ms must be <= {}",
 crate::data::units::DEBOUNCE_MS_MAX
 );
 Ok(())
 }
}

// Top-level

/// Top-level configuration aggregating every section.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
 pub server: ServerConfig,
 pub http: HttpConfig,
 pub tracker: TrackerConfig,
 pub engine: EngineConfig,
 pub database: DatabaseConfig,
 pub ui: UiConfig,
 pub defaults: DefaultsConfig,
 pub swarm_defaults: SwarmDefaultsConfig,
 pub peer_server: PeerServerConfig,
 pub nat: NatConfig,
 pub watcher: WatcherConfig,
 pub clients: ClientsConfig,
}

impl AppConfig {
 /// Validate every sub-section plus cross-section invariants.
 pub fn validate(&self) -> anyhow::Result<()> {
 self.server.validate()?;
 self.http.validate()?;
 self.tracker.validate()?;
 self.engine.validate()?;
 self.database.validate()?;
 self.ui.validate()?;
 self.defaults.validate()?;
 self.swarm_defaults.validate()?;
 self.peer_server.validate()?;
 self.nat.validate()?;
 self.watcher.validate()?;
 anyhow::ensure!(!self.clients.is_empty(), "clients must contain at least one entry");
 for client in &self.clients {
 client.validate()?;
 }
 let mut seen = std::collections::HashSet::new();
 for client in &self.clients {
 anyhow::ensure!(seen.insert(client.peer_id_prefix.clone()), "clients[].peer_id_prefix `{}` is duplicated", client.peer_id_prefix);
 }
 Ok(())
 }
}

/// The path to the config file: the value of the `REDSWARM_CONFIG`
/// environment variable, or `config.toml` if unset. Shared by [`load()`] and
/// the hot-reload watcher so they always read the same file.
pub fn path() -> String {
 std::env::var("REDSWARM_CONFIG").unwrap_or_else(|_| "config.toml".into())
}

/// Load and validate configuration from a specific file path. A missing or
/// unreadable file, a TOML parse error, or a validation failure all propagate
/// as `Err`. Used by [`load()`] (startup) and the hot-reload watcher (runtime).
pub fn load_from_path(path: &str) -> anyhow::Result<AppConfig> {
 let contents = std::fs::read_to_string(path)
 .map_err(|e| anyhow::anyhow!("could not read config file `{path}`: {e}"))?;
 let config: AppConfig = toml::from_str(&contents)
 .map_err(|e| anyhow::anyhow!("failed to parse config file `{path}`: {e}"))?;
 config
 .validate()
 .map_err(|e| anyhow::anyhow!("invalid config in `{path}`: {e}"))?;
 Ok(config)
}

/// Load configuration from disk.
///
/// The path is taken from [`path()`] (the `REDSWARM_CONFIG` environment
/// variable, default `config.toml`). A missing/unreadable file, a TOML parse
/// error, or a validation failure all propagate as `Err` - the application
/// will not start with an invalid configuration.
pub fn load() -> anyhow::Result<AppConfig> {
 let path = path();
 let config = load_from_path(&path)?;
 tracing::info!("loaded config from {path}");
 Ok(config)
}

/// Serialize `config` to pretty TOML and write it to `path`, replacing the
/// file. Every field is written explicitly - `#[serde(default)]` fields that
/// were implicit in the original file become explicit on round-trip. Comments
/// from the original file are not preserved (TOML serialization cannot
/// round-trip comments); a short generated header is prepended. The caller is
/// responsible for validating `config` before calling this - [`save_to_path`]
/// does not re-validate.
pub fn save_to_path(path: &str, config: &AppConfig) -> anyhow::Result<()> {
 let body = toml::to_string_pretty(config)
 .map_err(|e| anyhow::anyhow!("failed to serialize config to TOML: {e}"))?;
 let header = "# RedSwarm configuration.\n\
# Generated by the settings UI - edits via the web UI or a text editor are both supported.\n\
# Override this file's path by setting the REDSWARM_CONFIG env var.\n\n";
 std::fs::write(path, header.to_string() + &body)
 .map_err(|e| anyhow::anyhow!("could not write config file `{path}`: {e}"))?;
 Ok(())
}

#[cfg(test)]
pub mod test_helpers {
 use super::*;

 pub fn engine_cfg() -> EngineConfig {
 EngineConfig {
 tick_interval_secs: 1,
 stat_interval_secs: 5,
 announce_jitter_pct: 5.0,
 leech_upload_factor: 0.5,
 burst_choke_probability: 0.3,
 stop_grace_secs: 15,
 }
 }

 pub fn defaults_cfg() -> DefaultsConfig {
 DefaultsConfig {
 mode: crate::engine::Mode::DownloadAndUpload,
 speed_mode: crate::engine::SpeedMode::Dynamic,
 upload_bps: 524_288,
 download_bps: 1_048_576,
 jitter_pct: 20,
 ramp_up_secs: 120,
 start_download_pct: 0,
 freeze_on_zero_leechers: true,
 freeze_on_zero_seeders: true,
 goal_enabled: false,
 goal_direction: crate::engine::GoalDirection::Upload,
 goal_upload_target: 0,
 goal_download_target: 0,
 goal_target_secs: 0,
 goal_reached_action: crate::engine::GoalReachedAction::Stop,
 goal_reached_bps: 0,
 }
 }

 pub fn swarm_defaults_cfg() -> SwarmDefaultsConfig {
 SwarmDefaultsConfig {
 avg_leecher_download_bps: 3_000_000,
 seed_share_factor: 0.8,
 fair_share_multiplier: 1.0,
 max_upload_bps: 0,
 max_download_bps: 0,
 }
 }

 /// A full `AppConfig` satisfying every `validate()` invariant, for tests
 /// that need a complete `AppState`. Test-only - not a production default.
 pub fn app_config() -> AppConfig {
 AppConfig {
 server: ServerConfig {
 bind_addr: "127.0.0.1:0".into(),
 db_url: "sqlite::memory:".into(),
 log_filter: "off".into(),
 rebind_retry_secs: 2,
 sse_keepalive_secs: 15,
 },
 http: HttpConfig { timeout_secs: 5 },
 tracker: TrackerConfig {
 peer_port: 6881,
 default_interval_secs: 1800,
 min_interval_secs: 1,
 max_interval_secs: 3600,
 },
 engine: engine_cfg(),
 database: DatabaseConfig { max_connections: 2 },
 ui: UiConfig { event_log_limit: 100 },
 defaults: defaults_cfg(),
 swarm_defaults: swarm_defaults_cfg(),
 peer_server: PeerServerConfig {
 enabled: false,
 max_connections: 100,
 max_per_ip: 5,
 handshake_timeout_secs: 5,
 write_timeout_secs: 5,
 idle_timeout_secs: 240,
 body_read_timeout_secs: 15,
 accept_error_backoff_ms: 100,
 capture_keepalive_secs: 90,
 },
 nat: NatConfig {
 gateway_ip: String::new(),
 lease_lifetime_secs: 60,
 renew_interval_secs: 45,
 },
 watcher: WatcherConfig { debounce_ms: 300 },
 clients: vec![ClientSpecConfig {
 label: "Test Client".into(),
 version: "1.0".into(),
 peer_id_prefix: "-TC0000-".into(),
 user_agent: "TestClient/1.0".into(),
 query: "info_hash={info_hash}&peer_id={peer_id}".into(),
 numwant: 50,
 aliases: vec![],
 reserved_bytes: "0000000000100005".into(),
 fast_extension: true,
 keepalive_secs: 90,
 v_string: "TestClient/1.0".into(),
 m_dict: BTreeMap::new(),
 reqq: Some(500),
 encryption_preferred: None,
 send_upload_only: true,
 send_complete_ago: None,
 send_yourip: true,
 key_format: KeyFormat::UpperHex,
 }],
 }
 }
}

#[cfg(test)]
mod validation_tests {
 use super::*;

 /// Build a `NatConfig` with the given gateway IP and valid default intervals
 /// (lease 60 s, renew 45 s) so gateway-IP-focused tests don't repeat the
 /// interval boilerplate.
 fn nat_config(gateway_ip: String) -> NatConfig {
 NatConfig {
 gateway_ip,
 lease_lifetime_secs: 60,
 renew_interval_secs: 45,
 }
 }

 fn valid_client() -> ClientSpecConfig {
 ClientSpecConfig {
 label: "Test".into(),
 version: "1.0".into(),
 peer_id_prefix: "-TC0000-".into(),
 user_agent: "Test/1.0".into(),
 query: "info_hash={info_hash}&peer_id={peer_id}".into(),
 numwant: 50,
 aliases: vec![],
 reserved_bytes: "0000000000100005".into(),
 fast_extension: true,
 keepalive_secs: 90,
 v_string: "Test/1.0".into(),
 m_dict: BTreeMap::new(),
 reqq: Some(500),
 encryption_preferred: None,
 send_upload_only: true,
 send_complete_ago: None,
 send_yourip: true,
 key_format: KeyFormat::UpperHex,
 }
 }

 #[test]
 fn validate_accepts_valid_client() {
 assert!(valid_client().validate().is_ok());
 }

 #[test]
 fn validate_rejects_empty_v_string() {
 let mut c = valid_client();
 c.v_string = "".into();
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("v_string"), "got: {err}");
 }

 #[test]
 fn validate_rejects_whitespace_only_v_string() {
 let mut c = valid_client();
 c.v_string = " ".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_zero_reqq() {
 let mut c = valid_client();
 c.reqq = Some(0);
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("reqq"), "got: {err}");
 }

 #[test]
 fn validate_accepts_empty_m_dict() {
 let mut c = valid_client();
 c.m_dict = BTreeMap::new();
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_accepts_populated_m_dict() {
 let mut c = valid_client();
 c.m_dict.insert("ut_metadata".into(), 2);
 c.m_dict.insert("ut_pex".into(), 1);
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_rejects_empty_label() {
 let mut c = valid_client();
 c.label = "".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_empty_version() {
 let mut c = valid_client();
 c.version = "".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_empty_peer_id_prefix() {
 let mut c = valid_client();
 c.peer_id_prefix = "".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_query_without_info_hash_placeholder() {
 let mut c = valid_client();
 c.query = "peer_id={peer_id}".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_query_without_peer_id_placeholder() {
 let mut c = valid_client();
 c.query = "info_hash={info_hash}".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_zero_numwant() {
 let mut c = valid_client();
 c.numwant = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_bad_reserved_bytes_hex() {
 let mut c = valid_client();
 c.reserved_bytes = "not-hex".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_short_reserved_bytes() {
 let mut c = valid_client();
 c.reserved_bytes = "000000".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_zero_keepalive_secs() {
 let mut c = valid_client();
 c.keepalive_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_fast_extension_mismatch() {
 // fast_extension = true but reserved bytes DON'T have Fast Ext bit
 let mut c = valid_client();
 c.fast_extension = true;
 c.reserved_bytes = "0000000000100001".into(); // no 0x04
 assert!(c.validate().is_err(), "should reject fast_extension=true without reserved bit");
 }

 #[test]
 fn validate_rejects_fast_extension_mismatch_reverse() {
 // fast_extension = false but reserved bytes HAVE Fast Ext bit
 let mut c = valid_client();
 c.fast_extension = false;
 c.reserved_bytes = "0000000000100005".into(); // has 0x04
 assert!(c.validate().is_err(), "should reject fast_extension=false with reserved bit");
 }

 #[test]
 fn validate_accepts_consistent_fast_extension_true() {
 let mut c = valid_client();
 c.fast_extension = true;
 c.reserved_bytes = "0000000000100005".into(); // has 0x04
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_accepts_consistent_fast_extension_false() {
 let mut c = valid_client();
 c.fast_extension = false;
 c.reserved_bytes = "0000000000100001".into(); // no 0x04
 assert!(c.validate().is_ok());
 }

 // New validation rules

 #[test]
 fn validate_rejects_whitespace_only_label() {
 let mut c = valid_client();
 c.label = " ".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_whitespace_only_version() {
 let mut c = valid_client();
 c.version = " ".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_whitespace_only_peer_id_prefix() {
 let mut c = valid_client();
 c.peer_id_prefix = " ".into();
 assert!(c.validate().is_err(), "whitespace-only prefix should fail after trim");
 }

 #[test]
 fn validate_rejects_whitespace_only_user_agent() {
 let mut c = valid_client();
 c.user_agent = " ".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_empty_query() {
 let mut c = valid_client();
 c.query = "".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_whitespace_only_query() {
 let mut c = valid_client();
 c.query = " ".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_long_peer_id_prefix() {
 let mut c = valid_client();
 c.peer_id_prefix = "A".repeat(crate::data::protocol::PEER_ID_PREFIX_MAX_LEN + 1);
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("peer_id_prefix"), "got: {err}");
 }

 #[test]
 fn validate_accepts_max_length_peer_id_prefix() {
 let mut c = valid_client();
 c.peer_id_prefix = "A".repeat(crate::data::protocol::PEER_ID_PREFIX_MAX_LEN);
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_rejects_odd_length_reserved_bytes() {
 let mut c = valid_client();
 c.reserved_bytes = "000000000010000".into(); // 15 chars
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_empty_reserved_bytes() {
 let mut c = valid_client();
 c.reserved_bytes = "".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_rejects_long_reserved_bytes() {
 let mut c = valid_client();
 c.reserved_bytes = "0000000000100005FF".into(); // 18 chars
 assert!(c.validate().is_err());
 }

 #[test]
 fn validate_accepts_none_reqq() {
 let mut c = valid_client();
 c.reqq = None;
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_rejects_zero_m_dict_value() {
 let mut c = valid_client();
 c.m_dict.insert("ut_pex".into(), 0);
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("m_dict") && err.contains("ut_pex"), "got: {err}");
 }

 #[test]
 fn validate_accepts_positive_m_dict_values() {
 let mut c = valid_client();
 c.m_dict.insert("ut_pex".into(), 1);
 c.m_dict.insert("ut_metadata".into(), 2);
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_rejects_send_complete_ago_below_minus_one() {
 let mut c = valid_client();
 c.send_complete_ago = Some(-2);
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("complete_ago"), "got: {err}");
 }

 #[test]
 fn validate_accepts_send_complete_ago_minus_one() {
 let mut c = valid_client();
 c.send_complete_ago = Some(-1);
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_accepts_send_complete_ago_zero() {
 let mut c = valid_client();
 c.send_complete_ago = Some(0);
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_accepts_send_complete_ago_large() {
 let mut c = valid_client();
 c.send_complete_ago = Some(999999999);
 assert!(c.validate().is_ok());
 }

 #[test]
 fn validate_accepts_none_send_complete_ago() {
 let mut c = valid_client();
 c.send_complete_ago = None;
 assert!(c.validate().is_ok());
 }

 // AppConfig cross-section validation

 #[test]
 fn app_config_rejects_empty_clients() {
 let mut c = test_helpers::app_config();
 c.clients.clear();
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("at least one"), "got: {err}");
 }

 #[test]
 fn app_config_rejects_duplicate_peer_id_prefix() {
 let mut c = test_helpers::app_config();
 let dup = c.clients[0].clone();
 c.clients.push(dup);
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("duplicated"), "got: {err}");
 }

 // Section validation: tracker

 #[test]
 fn tracker_validate_rejects_zero_peer_port() {
 let mut c = test_helpers::app_config();
 c.tracker.peer_port = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn tracker_validate_rejects_default_below_min() {
 let mut c = test_helpers::app_config();
 c.tracker.default_interval_secs = c.tracker.min_interval_secs - 1;
 assert!(c.validate().is_err());
 }

 #[test]
 fn tracker_validate_rejects_max_not_above_min() {
 let mut c = test_helpers::app_config();
 c.tracker.max_interval_secs = c.tracker.min_interval_secs;
 assert!(c.validate().is_err());
 }

 #[test]
 fn tracker_validate_rejects_zero_min_interval() {
 let mut c = test_helpers::app_config();
 c.tracker.min_interval_secs = 0;
 assert!(c.validate().is_err());
 }

 // Section validation: engine

 #[test]
 fn engine_validate_rejects_zero_tick_interval() {
 let mut c = test_helpers::app_config();
 c.engine.tick_interval_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_zero_stat_interval() {
 let mut c = test_helpers::app_config();
 c.engine.stat_interval_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_negative_jitter() {
 let mut c = test_helpers::app_config();
 c.engine.announce_jitter_pct = -0.1;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_jitter_over_100() {
 let mut c = test_helpers::app_config();
 c.engine.announce_jitter_pct = 100.1;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_leech_factor_below_zero() {
 let mut c = test_helpers::app_config();
 c.engine.leech_upload_factor = -0.01;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_leech_factor_above_one() {
 let mut c = test_helpers::app_config();
 c.engine.leech_upload_factor = 1.01;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_burst_choke_below_zero() {
 let mut c = test_helpers::app_config();
 c.engine.burst_choke_probability = -0.01;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_burst_choke_above_one() {
 let mut c = test_helpers::app_config();
 c.engine.burst_choke_probability = 1.01;
 assert!(c.validate().is_err());
 }

 #[test]
 fn engine_validate_rejects_zero_stop_grace() {
 let mut c = test_helpers::app_config();
 c.engine.stop_grace_secs = 0;
 assert!(c.validate().is_err());
 }

 // Section validation: http

 #[test]
 fn http_validate_rejects_zero_timeout() {
 let mut c = test_helpers::app_config();
 c.http.timeout_secs = 0;
 assert!(c.validate().is_err());
 }

 // Section validation: database

 #[test]
 fn database_validate_rejects_zero_max_connections() {
 let mut c = test_helpers::app_config();
 c.database.max_connections = 0;
 assert!(c.validate().is_err());
 }

 // Section validation: ui

 #[test]
 fn ui_validate_rejects_zero_event_log_limit() {
 let mut c = test_helpers::app_config();
 c.ui.event_log_limit = 0;
 assert!(c.validate().is_err());
 }

 // Section validation: defaults

 #[test]
 fn defaults_validate_rejects_zero_upload_bps() {
 let mut c = test_helpers::app_config();
 c.defaults.upload_bps = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn defaults_validate_rejects_zero_download_bps() {
 let mut c = test_helpers::app_config();
 c.defaults.download_bps = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn defaults_validate_rejects_jitter_over_100() {
 let mut c = test_helpers::app_config();
 c.defaults.jitter_pct = 101;
 assert!(c.validate().is_err());
 }

 #[test]
 fn defaults_validate_rejects_start_download_pct_over_100() {
 let mut c = test_helpers::app_config();
 c.defaults.start_download_pct = 101;
 assert!(c.validate().is_err());
 }

 #[test]
 fn defaults_validate_rejects_zero_ramp_up_secs() {
 let mut c = test_helpers::app_config();
 c.defaults.ramp_up_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn defaults_validate_rejects_excessive_ramp_up_secs() {
 let mut c = test_helpers::app_config();
 c.defaults.ramp_up_secs = 86401;
 assert!(c.validate().is_err());
 }

 #[test]
 fn defaults_validate_rejects_goal_upload_target_over_max() {
 let mut c = test_helpers::app_config();
 c.defaults.goal_upload_target = crate::data::units::GOAL_MAX_TARGET_BYTES + 1;
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("goal_upload_target"), "error should name the field: {err}");
 }

 #[test]
 fn defaults_validate_rejects_goal_download_target_over_max() {
 let mut c = test_helpers::app_config();
 c.defaults.goal_download_target = crate::data::units::GOAL_MAX_TARGET_BYTES + 1;
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("goal_download_target"), "error should name the field: {err}");
 }

 #[test]
 fn defaults_validate_rejects_goal_target_secs_over_max() {
 let mut c = test_helpers::app_config();
 c.defaults.goal_target_secs = crate::data::units::GOAL_MAX_TIME_SECS + 1;
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("goal_target_secs"), "error should name the field: {err}");
 }

 #[test]
 fn defaults_validate_accepts_goal_forward_mode_zero_secs() {
 let mut c = test_helpers::app_config();
 c.defaults.goal_enabled = true;
 c.defaults.goal_upload_target = 1_073_741_824;
 c.defaults.goal_target_secs = 0;
 assert!(c.validate().is_ok(), "forward mode (target_secs=0) should be valid");
 }

 #[test]
 fn defaults_validate_accepts_goal_reverse_mode() {
 let mut c = test_helpers::app_config();
 c.defaults.goal_enabled = true;
 c.defaults.goal_upload_target = 1_073_741_824;
 c.defaults.goal_target_secs = 3600;
 assert!(c.validate().is_ok(), "reverse mode (target_secs>0) should be valid");
 }

 #[test]
 fn defaults_validate_accepts_goal_download_and_upload_both_targets() {
 let mut c = test_helpers::app_config();
 c.defaults.goal_enabled = true;
 c.defaults.goal_direction = crate::engine::GoalDirection::DownloadAndUpload;
 c.defaults.goal_upload_target = 5_368_709_120;
 c.defaults.goal_download_target = 1_073_741_824;
 c.defaults.goal_target_secs = 7200;
 assert!(c.validate().is_ok(), "D+U with both targets should be valid");
 }

 // Section validation: swarm

 #[test]
 fn swarm_validate_rejects_zero_avg_leecher_download() {
 let mut c = test_helpers::app_config();
 c.swarm_defaults.avg_leecher_download_bps = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn swarm_validate_rejects_zero_seed_share_factor() {
 let mut c = test_helpers::app_config();
 c.swarm_defaults.seed_share_factor = 0.0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn swarm_validate_rejects_seed_share_factor_above_one() {
 let mut c = test_helpers::app_config();
 c.swarm_defaults.seed_share_factor = 1.01;
 assert!(c.validate().is_err());
 }

 #[test]
 fn swarm_validate_rejects_negative_seed_share_factor() {
 let mut c = test_helpers::app_config();
 c.swarm_defaults.seed_share_factor = -0.01;
 assert!(c.validate().is_err());
 }

 #[test]
 fn swarm_validate_rejects_negative_fair_share_multiplier() {
 let mut c = test_helpers::app_config();
 c.swarm_defaults.fair_share_multiplier = -0.01;
 assert!(c.validate().is_err());
 }

 // Section validation: peer_server

 #[test]
 fn peer_server_validate_rejects_zero_max_connections() {
 let mut c = test_helpers::app_config();
 c.peer_server.max_connections = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn peer_server_validate_rejects_zero_max_per_ip() {
 let mut c = test_helpers::app_config();
 c.peer_server.max_per_ip = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn peer_server_validate_rejects_zero_handshake_timeout() {
 let mut c = test_helpers::app_config();
 c.peer_server.handshake_timeout_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn peer_server_validate_rejects_zero_write_timeout() {
 let mut c = test_helpers::app_config();
 c.peer_server.write_timeout_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn peer_server_validate_rejects_zero_idle_timeout() {
 let mut c = test_helpers::app_config();
 c.peer_server.idle_timeout_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn peer_server_validate_rejects_zero_body_read_timeout() {
 let mut c = test_helpers::app_config();
 c.peer_server.body_read_timeout_secs = 0;
 assert!(c.validate().is_err());
 }

 #[test]
 fn peer_server_validate_rejects_zero_capture_keepalive() {
 let mut c = test_helpers::app_config();
 c.peer_server.capture_keepalive_secs = 0;
 assert!(c.validate().is_err());
 }

 // KeyFormat

 #[test]
 fn key_format_lower_hex_produces_lowercase() {
 let key = KeyFormat::LowerHex.generate();
 assert_eq!(key.len(), 8);
 assert!(key.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()));
 }

 #[test]
 fn key_format_upper_hex_produces_uppercase() {
 let key = KeyFormat::UpperHex.generate();
 assert_eq!(key.len(), 8);
 assert!(key.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
 }

 #[test]
 fn key_format_decimal_produces_digits_only() {
 let key = KeyFormat::Decimal.generate();
 assert!(key.chars().all(|c| c.is_ascii_digit()));
 }

 #[test]
 fn key_format_round_trips_through_toml() {
 #[derive(serde::Serialize, serde::Deserialize)]
 struct Wrapper {
 key_format: KeyFormat,
 }
 for fmt in [KeyFormat::LowerHex, KeyFormat::UpperHex, KeyFormat::Decimal] {
 let w = Wrapper { key_format: fmt };
 let toml_str = toml::to_string(&w).unwrap();
 let parsed: Wrapper = toml::from_str(&toml_str).unwrap();
 assert_eq!(fmt, parsed.key_format, "round-trip failed for {:?}", fmt);
 }
 }

 #[test]
 fn app_config_with_new_fields_validates() {
 // The test_helpers::app_config() must produce a valid config with the
 // new fields - catches struct-construction drift.
 assert!(test_helpers::app_config().validate().is_ok());
 }

 #[test]
 fn load_rejects_client_missing_required_keepalive_secs() {
 // Regression: the per-client serde defaults were removed, so
 // keepalive_secs is now a required [[clients]] field. A config that
 // omits it must fail to load (previously it silently defaulted to 90).
 let cfg = test_helpers::app_config();
 let mut toml_str = toml::to_string(&cfg).unwrap();
 // Anchor to the line start so this matches the [[clients]] field, not
 // peer_server.capture_keepalive_secs (which also ends in "keepalive_secs").
 toml_str = toml_str.replace("\nkeepalive_secs = 90", "");
 let tmp = std::env::temp_dir()
 .join(format!("rf_missing_keepalive_{}.toml", std::process::id()));
 std::fs::write(&tmp, &toml_str).unwrap();
 let err = load_from_path(tmp.to_str().unwrap()).unwrap_err().to_string();
 assert!(
 err.contains("keepalive_secs"),
 "expected an error mentioning keepalive_secs, got: {err}"
 );
 let _ = std::fs::remove_file(&tmp);
 }

 #[test]
 fn load_rejects_invalid_defaults_mode() {
 let cfg = test_helpers::app_config();
 let mut toml_str = toml::to_string(&cfg).unwrap();
 toml_str = toml_str.replace("mode = \"download_and_upload\"", "mode = \"nonsense\"");
 let tmp = std::env::temp_dir()
 .join(format!("rf_bad_mode_{}.toml", std::process::id()));
 std::fs::write(&tmp, &toml_str).unwrap();
 let err = load_from_path(tmp.to_str().unwrap()).unwrap_err().to_string();
 assert!(err.contains("mode") || err.contains("invalid"), "got: {err}");
 let _ = std::fs::remove_file(&tmp);
 }

 #[test]
 fn load_rejects_invalid_defaults_speed_mode() {
 let cfg = test_helpers::app_config();
 let mut toml_str = toml::to_string(&cfg).unwrap();
 toml_str = toml_str.replace("speed_mode = \"dynamic\"", "speed_mode = \"fast\"");
 let tmp = std::env::temp_dir()
 .join(format!("rf_bad_speed_{}.toml", std::process::id()));
 std::fs::write(&tmp, &toml_str).unwrap();
 assert!(load_from_path(tmp.to_str().unwrap()).is_err());
 let _ = std::fs::remove_file(&tmp);
 }

 #[test]
 fn server_validate_rejects_zero_rebind_retry_secs() {
 let mut c = test_helpers::app_config();
 c.server.rebind_retry_secs = 0;
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("rebind_retry_secs"), "got: {err}");
 }

 #[test]
 fn server_validate_rejects_bind_addr_without_port() {
 let mut c = test_helpers::app_config();
 c.server.bind_addr = "0.0.0.0".into();
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("bind_addr"), "got: {err}");
 }

 #[test]
 fn server_validate_rejects_bind_addr_garbage() {
 let mut c = test_helpers::app_config();
 c.server.bind_addr = "not an address".into();
 assert!(c.validate().is_err());
 }

 #[test]
 fn server_validate_accepts_valid_ipv6_bind_addr() {
 let mut c = test_helpers::app_config();
 c.server.bind_addr = "[::1]:3000".into();
 assert!(c.validate().is_ok());
 }

 #[test]
 fn watcher_validate_rejects_zero_debounce_ms() {
 let mut c = test_helpers::app_config();
 c.watcher.debounce_ms = 0;
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("debounce_ms"), "got: {err}");
 }

 #[test]
 fn watcher_validate_rejects_excessive_debounce_ms() {
 let mut c = test_helpers::app_config();
 c.watcher.debounce_ms = 10_001;
 assert!(c.validate().is_err());
 }

 #[test]
 fn nat_validate_accepts_empty_gateway_ip() {
 let c = nat_config(String::new());
 assert!(c.validate().is_ok());
 }

 #[test]
 fn nat_validate_accepts_valid_ipv4() {
 let c = nat_config("10.2.0.1".into());
 assert!(c.validate().is_ok());
 }

 #[test]
 fn nat_validate_accepts_valid_ipv6() {
 let c = nat_config("::1".into());
 assert!(c.validate().is_ok());
 }

 #[test]
 fn nat_validate_rejects_invalid_ip() {
 let c = nat_config("not-an-ip".into());
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("nat.gateway_ip"), "got: {err}");
 }

 #[test]
 fn nat_validate_trims_whitespace() {
 let c = nat_config(" 10.2.0.1 ".into());
 assert!(c.validate().is_ok());
 }

 #[test]
 fn nat_validate_rejects_empty_string_after_trim() {
 let c = nat_config(" ".into());
 assert!(c.validate().is_ok());
 }

 #[test]
 fn nat_validate_rejects_zero_lease_lifetime() {
 let mut c = nat_config("10.2.0.1".into());
 c.lease_lifetime_secs = 0;
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("nat.lease_lifetime_secs"), "got: {err}");
 }

 #[test]
 fn nat_validate_rejects_zero_renew_interval() {
 let mut c = nat_config("10.2.0.1".into());
 c.renew_interval_secs = 0;
 let err = c.validate().unwrap_err().to_string();
 assert!(err.contains("nat.renew_interval_secs"), "got: {err}");
 }

 #[test]
 fn nat_validate_rejects_renew_not_less_than_lifetime() {
 let mut c = nat_config("10.2.0.1".into());
 c.renew_interval_secs = 60;
 c.lease_lifetime_secs = 60;
 let err = c.validate().unwrap_err().to_string();
 assert!(
 err.contains("must be less than"),
 "got: {err}"
 );
 }

 #[test]
 fn nat_validate_accepts_renew_just_below_lifetime() {
 let mut c = nat_config("10.2.0.1".into());
 c.renew_interval_secs = 59;
 c.lease_lifetime_secs = 60;
 assert!(c.validate().is_ok());
 }

 #[test]
 fn save_to_path_round_trips_full_config() {
 let cfg = test_helpers::app_config();
 let tmp = std::env::temp_dir()
 .join(format!("rf_save_roundtrip_{}.toml", std::process::id()));
 save_to_path(tmp.to_str().unwrap(), &cfg).expect("save should succeed");
 let reloaded = load_from_path(tmp.to_str().unwrap()).expect("load should succeed");
 assert_eq!(cfg, reloaded, "config must round-trip through TOML");
 let _ = std::fs::remove_file(&tmp);
 }

 #[test]
 fn save_to_path_round_trips_clients_with_m_dict() {
 let mut cfg = test_helpers::app_config();
 cfg.clients[0].m_dict.insert("ut_pex".into(), 1);
 cfg.clients[0].m_dict.insert("ut_metadata".into(), 2);
 cfg.clients[0].aliases = vec!["Alias1".into(), "Alias2".into()];
 cfg.clients.push(ClientSpecConfig {
 label: "Second Client".into(),
 version: "1.0".into(),
 peer_id_prefix: "-SC0000-".into(),
 user_agent: "SecondClient/1.0".into(),
 query: "info_hash={info_hash}&peer_id={peer_id}".into(),
 numwant: 80,
 aliases: vec![],
 reserved_bytes: "0000000000100001".into(),
 fast_extension: false,
 keepalive_secs: 120,
 v_string: "SecondClient/1.0".into(),
 m_dict: BTreeMap::from([("ut_pex".into(), 1)]),
 reqq: Some(300),
 encryption_preferred: None,
 send_upload_only: true,
 send_complete_ago: None,
 send_yourip: true,
 key_format: KeyFormat::UpperHex,
 });
 assert!(cfg.validate().is_ok());
 let tmp = std::env::temp_dir()
 .join(format!("rf_save_clients_{}.toml", std::process::id()));
 save_to_path(tmp.to_str().unwrap(), &cfg).expect("save should succeed");
 let reloaded = load_from_path(tmp.to_str().unwrap()).expect("load should succeed");
 assert_eq!(cfg, reloaded, "clients with m_dict must round-trip");
 assert_eq!(reloaded.clients.len(), 2);
 assert_eq!(reloaded.clients[0].m_dict.get("ut_pex"), Some(&1));
 assert_eq!(reloaded.clients[1].m_dict.get("ut_pex"), Some(&1));
 let _ = std::fs::remove_file(&tmp);
 }
}
