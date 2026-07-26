//! Peer-wire server - listens on `peer_port` and accepts inbound BitTorrent
//! peer connections to make the emulated peer "connectable".
//!
//! The server completes the BT handshake, sends a bitfield (or `have_all`
//! for Fast Extension clients), unchokes the peer, and keeps the connection
//! alive with keepalives. It **never serves piece data** - piece requests
//! are silently ignored. This avoids triggering hash-mismatch bans while
//! still appearing as a real, connectable, protocol-participating seeder.
//!
//! Architecture: one global listener (started in `main.rs`), shared by all
//! audits. Each audit registers its `info_hash` + `peer_id` + client fingerprint
//! via `register()`. The handshake handler routes connections by `info_hash`.
//! This avoids port conflicts - only one listener binds `peer_port`.
//!
//! Security: all I/O is timeout-bounded; per-IP and global connection caps
//! prevent DoS; message bodies are drained into a fixed discard buffer
//! (never stored); unknown info hashes are rejected before any state is
//! allocated.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::data::protocol;

/// Known BEP-3 peer-wire message IDs we expect from connected peers. Used to
/// validate incoming messages - unknown IDs are drained but not acted on.
const KNOWN_PEER_MSG_IDS: &[u8] = &[
 protocol::MSG_CHOKE, protocol::MSG_UNCHOKE,
 protocol::MSG_INTERESTED, protocol::MSG_NOT_INTERESTED,
 protocol::MSG_HAVE, protocol::MSG_BITFIELD,
 protocol::MSG_REQUEST, protocol::MSG_PIECE, protocol::MSG_CANCEL,
 protocol::MSG_PORT,
 protocol::MSG_SUGGEST_PIECE, protocol::MSG_HAVE_ALL, protocol::MSG_HAVE_NONE,
 protocol::MSG_REJECT_REQUEST, protocol::MSG_ALLOWED_FAST,
 protocol::MSG_EXTENDED,
];

/// Per-audit registration: the info_hash we serve, our peer_id, and the
/// client fingerprint (reserved bytes, Fast Extension, keepalive interval,
/// BEP-10 extension handshake fields).
#[derive(Clone)]
struct AuditRegistration {
 peer_id: [u8; protocol::PEER_ID_LEN],
 reserved: [u8; protocol::RESERVED_LEN],
 fast_extension: bool,
 keepalive: Duration,
 /// BEP-10 `v` field - client name + version string.
 v_string: String,
 /// BEP-10 `m` dict - extension name → local message ID.
 m_dict: BTreeMap<String, u32>,
 /// BEP-10 `reqq` field - max outstanding request queue. `None` = omit.
 reqq: Option<u32>,
 /// True for capture sessions - sends `have_none` instead of `have_all`
 /// so the client stays connected long enough to send its ext handshake.
 capture_mode: bool,
 /// BEP-10 `e` field - encryption preference. `None` = omit.
 encryption_preferred: Option<bool>,
 /// Whether to send `upload_only: 1` in the ext handshake (seeders).
 send_upload_only: bool,
 /// BEP-10 `complete_ago` field. `None` = omit. `Some(-1)` = never complete.
 send_complete_ago: Option<i64>,
 /// Whether to send `yourip` in the ext handshake.
 send_yourip: bool,
}

/// Global peer server - one listener, shared by all audits.
pub struct PeerServer {
 inner: Arc<PeerServerInner>,
 cancel: CancellationToken,
 /// `Some` when the accept loop is running; `None` for the disabled variant.
 /// Stored so [`PeerServer::stop`] can await the accept loop's exit, which
 /// drops the `TcpListener` and releases the bound port. The hot-reloader
 /// uses this to free the port for a new server before rebinding. Guarded
 /// by a `Mutex` so `stop` can `take()` it from `&self` (the server is
 /// shared via `Arc`, and `JoinHandle::await` consumes the handle).
 accept_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct PeerServerInner {
 /// Active audits keyed by info_hash. Each audit registers its peer_id +
 /// client fingerprint so the handshake handler can respond correctly.
 audits: Mutex<HashMap<[u8; protocol::INFO_HASH_LEN], AuditRegistration>>,
 /// Fingerprint capture sessions - shared with the axum handlers.
 /// When a peer connects with a capture session's info_hash, the handler
 /// records the handshake + ext handshake fields.
 capture: crate::capture::CaptureStore,
 /// Keepalive interval for capture-mode connections (from config).
 capture_keepalive: Duration,
 semaphore: Arc<Semaphore>,
 per_ip: Arc<Mutex<HashMap<IpAddr, u32>>>,
 handshake_timeout: Duration,
 write_timeout: Duration,
 idle_timeout: Duration,
 body_read_timeout: Duration,
 accept_backoff: Duration,
 max_per_ip: u32,
}

impl PeerServer {
 /// Create a disabled (no-op) peer server - used when `enabled = false`.
 pub fn disabled(capture: crate::capture::CaptureStore) -> Self {
 // Use zero-capacity semaphore - no connections can be acquired.
 // The durations are never used since no connections are accepted
 // (no listener is bound and the audits map is empty).
 Self {
 inner: Arc::new(PeerServerInner {
 audits: Mutex::new(HashMap::new()),
 capture,
 capture_keepalive: Duration::from_secs(0),
 semaphore: Arc::new(Semaphore::new(0)),
 per_ip: Arc::new(Mutex::new(HashMap::new())),
 handshake_timeout: Duration::from_secs(0),
 write_timeout: Duration::from_secs(0),
 idle_timeout: Duration::from_secs(0),
 body_read_timeout: Duration::from_secs(0),
 accept_backoff: Duration::from_secs(0),
 max_per_ip: 0,
 }),
 cancel: CancellationToken::new(),
 accept_task: std::sync::Mutex::new(None),
 }
 }

 /// Start the global peer server. Binds once and shares the listener
 /// across all audits. Call from `main.rs` before starting any audit.
 pub fn start(
 bind_addr: String,
 cfg: &crate::config::PeerServerConfig,
 capture: crate::capture::CaptureStore,
 ) -> anyhow::Result<Self> {
 if !cfg.enabled {
 return Ok(Self::disabled(capture));
 }

 let listener = std::net::TcpListener::bind(&bind_addr)
 .map_err(|e| anyhow::anyhow!("peer server bind {bind_addr}: {e}"))?;
 listener.set_nonblocking(true)?;
 let listener = TcpListener::from_std(listener)?;

 let cancel = CancellationToken::new();
 let cancel_clone = cancel.clone();

 let inner = Arc::new(PeerServerInner {
 audits: Mutex::new(HashMap::new()),
 capture,
 capture_keepalive: Duration::from_secs(cfg.capture_keepalive_secs),
 semaphore: Arc::new(Semaphore::new(cfg.max_connections)),
 per_ip: Arc::new(Mutex::new(HashMap::new())),
 handshake_timeout: Duration::from_secs(cfg.handshake_timeout_secs),
 write_timeout: Duration::from_secs(cfg.write_timeout_secs),
 idle_timeout: Duration::from_secs(cfg.idle_timeout_secs),
 body_read_timeout: Duration::from_secs(cfg.body_read_timeout_secs),
 accept_backoff: Duration::from_millis(cfg.accept_error_backoff_ms),
 max_per_ip: cfg.max_per_ip,
 });

 tracing::info!(addr = %bind_addr, "peer server listening");

 let inner_clone = Arc::clone(&inner);
 let handle = tokio::spawn(async move {
 run_accept_loop(listener, inner_clone, cancel_clone).await;
 });

 Ok(Self { inner, cancel, accept_task: std::sync::Mutex::new(Some(handle)) })
 }

 /// Stop the peer server - cancels the accept loop and awaits its exit,
 /// which drops the `TcpListener` and releases the bound port. Used by
 /// the hot-reloader to free the port for a new server before rebinding.
 /// A no-op for the disabled variant (no accept task). Safe to call from
 /// `&self` (the `JoinHandle` is `take`n from behind a `Mutex`).
 pub async fn stop(&self) {
 self.cancel.cancel();
 let handle = self.accept_task.lock().unwrap().take();
 if let Some(h) = handle {
 let _ = h.await;
 }
 }

 /// Register an audit's info_hash + client fingerprint. The peer server
 /// will accept connections for this info_hash and respond with the
 /// correct client's reserved bytes, bitfield/have_all, and keepalive.
 pub fn register(
 &self,
 info_hash: [u8; protocol::INFO_HASH_LEN],
 peer_id: [u8; protocol::PEER_ID_LEN],
 client: &crate::config::ClientSpecConfig,
 ) -> anyhow::Result<()> {
 let reserved_bytes = crate::bencode::hex_decode(&client.reserved_bytes)
 .map_err(|e| anyhow::anyhow!("invalid reserved_bytes for {}: {e}", client.display_name()))?;
 anyhow::ensure!(
 reserved_bytes.len() == protocol::RESERVED_LEN,
 "reserved_bytes for {} must be {} bytes",
 client.display_name(),
 protocol::RESERVED_LEN
 );
 let mut reserved = [0u8; protocol::RESERVED_LEN];
 reserved.copy_from_slice(&reserved_bytes);

 let reg = AuditRegistration {
 peer_id,
 reserved,
 fast_extension: client.fast_extension,
 keepalive: Duration::from_secs(client.keepalive_secs),
 v_string: client.v_string.clone(),
 m_dict: client.m_dict.clone(),
 reqq: client.reqq,
 capture_mode: false,
 encryption_preferred: client.encryption_preferred,
 send_upload_only: client.send_upload_only,
 send_complete_ago: client.send_complete_ago,
 send_yourip: client.send_yourip,
 };
 self.inner.audits.lock().unwrap().insert(info_hash, reg);
 tracing::info!(info_hash = ?info_hash, "peer server: registered audit");
 Ok(())
 }

 /// Deregister an audit's info_hash. The peer server will stop accepting
 /// connections for this torrent.
 pub fn deregister(&self, info_hash: &[u8; protocol::INFO_HASH_LEN]) {
 if self.inner.audits.lock().unwrap().remove(info_hash).is_some() {
 tracing::info!(info_hash = ?info_hash, "peer server: deregistered audit");
 }
 }
}

impl Drop for PeerServer {
 fn drop(&mut self) {
 // Cancel the accept loop so the bound port is released as soon as the
 // last holder of this `PeerServer` drops it. This matters for hot
 // reload: when the peer server is swapped out, running audits keep
 // their old `Arc<PeerServer>` snapshot (frozen); the old listener
 // stays alive for them and is freed only once they all end. The accept
 // loop observes this cancel via its cloned token.
 self.cancel.cancel();
 }
}

/// RAII guard that decrements the per-IP connection counter on drop.
struct IpGuard {
 ip: IpAddr,
 per_ip: Arc<Mutex<HashMap<IpAddr, u32>>>,
}

impl Drop for IpGuard {
 fn drop(&mut self) {
 let mut per_ip = self.per_ip.lock().unwrap();
 if let Some(count) = per_ip.get_mut(&self.ip) {
 *count = count.saturating_sub(1);
 if *count == 0 {
 per_ip.remove(&self.ip);
 }
 }
 }
}

async fn run_accept_loop(listener: TcpListener, inner: Arc<PeerServerInner>, cancel: CancellationToken) {
 loop {
 tokio::select! {
 _ = cancel.cancelled() => break,
 res = listener.accept() => {
 let (mut socket, addr) = match res {
 Ok(s) => s,
 Err(e) => {
 tracing::warn!(error = %e, "peer server accept error");
 tokio::time::sleep(inner.accept_backoff).await;
 continue;
 }
 };
 let _ = socket.set_nodelay(true);

 let ip = addr.ip();
 {
 let mut per_ip = inner.per_ip.lock().unwrap();
 let count = per_ip.entry(ip).or_insert(0);
 if *count >= inner.max_per_ip {
 tracing::debug!("peer server: rejecting {addr}, too many connections from {ip}");
 continue;
 }
 *count += 1;
 }
 let per_ip = Arc::clone(&inner.per_ip);
 let ip_for_drop = ip;

 let permit = match Arc::clone(&inner.semaphore).try_acquire_owned() {
 Ok(p) => p,
 Err(_) => {
 let mut per_ip = per_ip.lock().unwrap();
 if let Some(c) = per_ip.get_mut(&ip_for_drop) { *c = c.saturating_sub(1); }
 tracing::debug!("peer server full, rejecting connection from {addr}");
 continue;
 }
 };

 let inner = Arc::clone(&inner);
 let cancel = cancel.clone();
 tokio::spawn(async move {
 let _permit = permit;
 let _ip_guard = IpGuard { ip: ip_for_drop, per_ip };
 let _ = handle_connection(&mut socket, &inner, &cancel).await;
 });
 }
 }
 }
 tracing::info!("peer server stopped");
}

/// Handle a single peer connection: handshake → bitfield → unchoke → keepalive loop.
/// Never serves piece data. All I/O is timeout-bounded.
async fn handle_connection(
 socket: &mut TcpStream,
 inner: &PeerServerInner,
 cancel: &CancellationToken,
) -> std::io::Result<()> {
 // 1. Read handshake (68 bytes) under timeout - slow-loris defense
 let mut hs = [0u8; protocol::HANDSHAKE_LEN];
 let peer_addr = socket.peer_addr().map(|a| a.to_string()).unwrap_or_default();
 tracing::debug!(peer = %peer_addr, "peer server: TCP connection accepted, waiting for handshake");
 tokio::time::timeout(inner.handshake_timeout, socket.read_exact(&mut hs))
 .await
 .map_err(|e| {
 tracing::debug!(peer = %peer_addr, error = %e, "peer server: handshake read failed/timeout");
 std::io::Error::new(std::io::ErrorKind::TimedOut, "handshake read")
 })??;

 // Validate handshake
 if hs[0] != protocol::PSTRLEN || &hs[1..protocol::RESERVED_OFFSET] != protocol::PSTR {
 tracing::debug!(peer = %peer_addr, first_byte = hs[0], "peer server: not a BitTorrent handshake - dropping");
 return Ok(()); // not BitTorrent - silently drop
 }
 let peer_info_hash: &[u8; protocol::INFO_HASH_LEN] = hs[protocol::INFO_HASH_OFFSET..protocol::PEER_ID_OFFSET]
 .try_into().unwrap();
 let peer_reserved: [u8; protocol::RESERVED_LEN] =
 hs[protocol::RESERVED_OFFSET..protocol::INFO_HASH_OFFSET]
 .try_into()
 .unwrap();
 let peer_supports_fast_ext = peer_reserved[protocol::FAST_EXT_BYTE_INDEX] & protocol::FAST_EXT_BIT_MASK != 0;
 let peer_supports_ltep = peer_reserved[protocol::LTEP_BYTE_INDEX] & protocol::LTEP_BIT_MASK != 0;
 tracing::debug!(
 peer = %peer_addr,
 info_hash = %crate::bencode::hex_encode(peer_info_hash),
 fast_ext = peer_supports_fast_ext,
 ltep = peer_supports_ltep,
 "peer server: connection received"
 );

 // Look up the registration for this info_hash
 let (reg, capture_token) = {
 let audits = inner.audits.lock().unwrap();
 match audits.get(peer_info_hash) {
 Some(r) => (r.clone(), None),
 None => {
 // Not a registered audit - check if it's a capture session
 match inner.capture.find_by_info_hash(peer_info_hash) {
 Some(token) => {
 // Record the peer's handshake fields (first client only)
 let peer_peer_id: [u8; protocol::PEER_ID_LEN] =
 hs[protocol::PEER_ID_OFFSET..protocol::HANDSHAKE_LEN]
 .try_into()
 .unwrap();
 let recorded = inner.capture.record_handshake(&token, peer_reserved, peer_peer_id);
 if recorded {
 tracing::info!(
 token = %token,
 reserved = ?peer_reserved,
 "capture: peer handshake recorded"
 );
 } else {
 tracing::debug!(token = %token, "capture: handshake rejected - session already locked");
 return Ok(()); // another client already captured - drop
 }

 // Create a synthetic registration using the peer_id
 // stored at capture-start time (same one used for the
 // torrent filename) so the client sees a consistent
 // identity across tracker announce + wire handshake.
 let our_peer_id = inner.capture.get_our_peer_id(&token)
 .unwrap_or_else(|| crate::peer_id::generate_peer_id(""));
 let mut m = BTreeMap::new();
 m.insert(protocol::EXT_UT_PEX.to_string(), 1u32);
 m.insert(protocol::EXT_UT_METADATA.to_string(), 2u32);
 let mut capture_reserved = [0u8; protocol::RESERVED_LEN];
 capture_reserved[protocol::LTEP_BYTE_INDEX] |= protocol::LTEP_BIT_MASK;
 capture_reserved[protocol::DHT_BYTE_INDEX] |= protocol::DHT_BIT_MASK;
 capture_reserved[protocol::FAST_EXT_BYTE_INDEX] |= protocol::FAST_EXT_BIT_MASK;
 let reg = AuditRegistration {
 peer_id: our_peer_id,
 reserved: capture_reserved,
 fast_extension: true,
 keepalive: inner.capture_keepalive,
 v_string: "redswarm/capture".into(),
 m_dict: m,
 reqq: Some(500),
 capture_mode: true,
 encryption_preferred: None,
 send_upload_only: false,
 send_complete_ago: None,
 send_yourip: true,
 };
 (reg, Some(token))
 }
 None => {
 tracing::debug!(
 peer = %peer_addr,
 info_hash = %crate::bencode::hex_encode(peer_info_hash),
 "peer server: unknown info_hash - no audit or capture session matches, dropping"
 );
 return Ok(());
 }
 }
 }
 }
 };

 // 2. Send our handshake
 let mut our_hs = [0u8; protocol::HANDSHAKE_LEN];
 our_hs[0] = protocol::PSTRLEN;
 our_hs[1..protocol::RESERVED_OFFSET].copy_from_slice(protocol::PSTR);
 our_hs[protocol::RESERVED_OFFSET..protocol::INFO_HASH_OFFSET].copy_from_slice(&reg.reserved);
 our_hs[protocol::INFO_HASH_OFFSET..protocol::PEER_ID_OFFSET].copy_from_slice(peer_info_hash);
 our_hs[protocol::PEER_ID_OFFSET..protocol::HANDSHAKE_LEN].copy_from_slice(&reg.peer_id);
 write_timeout(socket, &our_hs, inner.write_timeout).await?;

 // 3. Send bitfield (or have_all/have_none for Fast Extension clients).
 // Fast Extension is only active if BOTH peers set the bit (BEP-6).
 if reg.capture_mode {
 if peer_supports_fast_ext {
 write_msg(socket, protocol::MSG_HAVE_NONE, &[], inner.write_timeout).await?;
 } else {
 write_msg(socket, protocol::MSG_BITFIELD, &[0x00], inner.write_timeout).await?;
 }
 } else if reg.fast_extension && peer_supports_fast_ext {
 write_msg(socket, protocol::MSG_HAVE_ALL, &[], inner.write_timeout).await?;
 } else {
 write_msg(socket, protocol::MSG_BITFIELD, &protocol::SEEDER_BITFIELD, inner.write_timeout).await?;
 }

 // 4. Send unchoke (emulation) or interested (capture).
 // In capture mode we're a leecher - `interested` tells the seeder we want
 // its data, keeping it connected. `unchoke` would be meaningless (we have
 // nothing to offer).
 if reg.capture_mode {
 write_msg(socket, protocol::MSG_INTERESTED, &[], inner.write_timeout).await?;
 } else {
 write_msg(socket, protocol::MSG_UNCHOKE, &[], inner.write_timeout).await?;
 }

 // 4.5. Send BEP-10 extension handshake if both we and the peer support LTEP.
 // BEP-10: "This message should be sent immediately after the standard
 // bittorrent handshake to any peer that supports this extension protocol."
 if peer_supports_ltep && reg.reserved[protocol::LTEP_BYTE_INDEX] & protocol::LTEP_BIT_MASK != 0 {
 let peer_ip = socket.peer_addr().ok().map(|addr| match addr.ip() {
 std::net::IpAddr::V4(v4) => v4.octets().to_vec(),
 std::net::IpAddr::V6(v6) => v6.octets().to_vec(),
 });
 let ext_hs = build_ext_handshake(&reg, peer_ip.as_deref());
 tracing::debug!(
 v = %reg.v_string,
 reqq = reg.reqq,
 m_dict_keys = ?reg.m_dict.keys().collect::<Vec<_>>(),
 upload_only = reg.send_upload_only,
 complete_ago = ?reg.send_complete_ago,
 e = ?reg.encryption_preferred,
 yourip = ?peer_ip.is_some(),
 "sending ext handshake"
 );
 write_msg(socket, protocol::MSG_EXTENDED, &ext_hs, inner.write_timeout).await?;
 }

 // 5. Keepalive loop - send keepalives, drain incoming messages, never serve data
 let mut last_recv = tokio::time::Instant::now();
 let mut keepalive_interval = tokio::time::interval(reg.keepalive);
 keepalive_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
 // Skip the first (immediate) tick - real clients wait for the full
 // interval before sending the first keepalive, not immediately after handshake.
 let mut first_keepalive_tick = true;

 // Capture-mode state for behavioral fingerprinting
 let mut first_keepalive_at: Option<tokio::time::Instant> = None;
 let mut second_keepalive_at: Option<tokio::time::Instant> = None;

 let mut header = [0u8; protocol::MSG_HEADER_LEN];
 let mut discard = [0u8; protocol::DISCARD_BUF_LEN];

 loop {
 tokio::select! {
 _ = cancel.cancelled() => {
 let _ = socket.shutdown().await;
 return Ok(());
 }
 r = socket.read_exact(&mut header) => {
 match r {
 Ok(_) => {
 last_recv = tokio::time::Instant::now();
 let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
 if len == 0 {
 // Keepalive - measure cadence for capture sessions
 if capture_token.is_some() {
 let now = tokio::time::Instant::now();
 if first_keepalive_at.is_none() {
 first_keepalive_at = Some(now);
 } else if second_keepalive_at.is_none() {
 second_keepalive_at = Some(now);
 if let (Some(first), Some(second), Some(token)) =
 (first_keepalive_at, second_keepalive_at, capture_token.as_ref())
 {
 let secs = second.duration_since(first).as_secs();
 if secs > 0 {
 inner.capture.record_keepalive_secs(token, secs);
 tracing::info!(token = %token, keepalive_secs = secs, "capture: keepalive cadence measured");
 }
 }
 }
 if let Some(token) = capture_token.as_ref() {
 inner.capture.record_peer_message(token, "keepalive");
 }
 }
 continue;
 }
 if len > protocol::MAX_PEER_MSG_LEN { return Ok(()); } // oversized - drop
 let id = header[4];
 // Security: we never serve data. A piece message means
 // the peer thinks we requested data - impossible, so drop.
 if id == protocol::MSG_PIECE { return Ok(()); }
 // Record message name for behavioral fingerprint
 if let Some(token) = capture_token.as_ref() {
 inner.capture.record_peer_message(token, msg_name(id));
 }
 // For capture sessions, parse MSG_EXTENDED instead of draining.
 // This captures the BEP-10 extension handshake (v, m, reqq, e, upload_only, complete_ago).
 if capture_token.is_some() && id == protocol::MSG_EXTENDED {
 let body_len = len - 1;
 let mut body = vec![0u8; body_len];
 tokio::time::timeout(
 inner.body_read_timeout,
 socket.read_exact(&mut body),
 )
 .await
 .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "body read"))??;
 // First byte is the ext sub-ID; 0 = handshake.
 if body_len == 0 || body[0] != protocol::EXT_HANDSHAKE_ID {
 continue;
 }
 let Ok(dict) = crate::bencode::decode(&body[1..]) else {
 tracing::warn!(body_len, "capture: ext handshake bencode decode failed");
 continue;
 };
 let v_string = dict.get(protocol::K_V)
 .and_then(|v| v.as_str())
 .map(String::from);
 let reqq = dict.get(protocol::K_REQQ)
 .and_then(|v| v.as_int())
 .map(|i| i as u32);
 let m_dict: Option<BTreeMap<String, u32>> = dict.get(protocol::K_M)
 .and_then(|m| m.as_dict())
 .map(|d| {
 d.iter()
 .filter_map(|(k, v)| {
 let key = std::str::from_utf8(k).ok()?.to_string();
 let val = v.as_int()? as u32;
 Some((key, val))
 })
 .collect()
 });
 let encryption_preferred = dict.get(protocol::K_E)
 .and_then(|v| v.as_int())
 .map(|i| i != 0);
 let upload_only = dict.get(protocol::K_UPLOAD_ONLY)
 .and_then(|v| v.as_int())
 .map(|i| i != 0);
 let complete_ago = dict.get(protocol::K_COMPLETE_AGO)
 .and_then(|v| v.as_int());
 let yourip = dict.get(protocol::K_YOURIP)
 .and_then(|v| v.as_bytes())
 .map(|b| b.to_vec());
 let listen_port = dict.get(protocol::K_P)
 .and_then(|v| v.as_int())
 .filter(|&v| v > 0 && v <= crate::data::protocol::MAX_PORT as i64)
 .map(|v| v as u16);
 let metadata_size = dict.get(protocol::K_METADATA_SIZE)
 .and_then(|v| v.as_int())
 .filter(|&v| v > 0)
 .map(|v| v as u64);
 let ipv4 = dict.get(protocol::K_IPV4)
 .and_then(|v| v.as_bytes())
 .filter(|b| b.len() == 4)
 .map(|b| b.to_vec());
 let ipv6 = dict.get(protocol::K_IPV6)
 .and_then(|v| v.as_bytes())
 .filter(|b| b.len() == 16)
 .map(|b| b.to_vec());
 let share_mode = dict.get(protocol::K_SHARE_MODE)
 .and_then(|v| v.as_int())
 .map(|i| i != 0);
 tracing::debug!(
 has_v = v_string.is_some(),
 has_m = m_dict.is_some(),
 has_reqq = reqq.is_some(),
 has_e = encryption_preferred.is_some(),
 has_upload_only = upload_only.is_some(),
 has_complete_ago = complete_ago.is_some(),
 has_yourip = yourip.is_some(),
 has_p = listen_port.is_some(),
 has_metadata_size = metadata_size.is_some(),
 has_ipv4 = ipv4.is_some(),
 has_ipv6 = ipv6.is_some(),
 has_share_mode = share_mode.is_some(),
 "capture: ext handshake fields parsed"
 );
 if let Some(token) = capture_token.as_ref() {
 let recorded = inner.capture.record_ext_handshake(
 token,
 crate::capture::ExtHandshakeData {
 v_string,
 m_dict,
 reqq,
 encryption_preferred,
 upload_only,
 complete_ago,
 yourip,
 listen_port,
 metadata_size,
 ipv4,
 ipv6,
 share_mode,
 },
 );
 if recorded {
 tracing::info!(token = %token, "capture: ext handshake recorded");
 } else {
 tracing::debug!(token = %token, "capture: ext handshake rejected - session already locked");
 }
 }
 continue;
 }
 // Validate against known BEP-3 message IDs.
 if !KNOWN_PEER_MSG_IDS.contains(&id) {
 tracing::debug!(msg_id = id, "peer: unknown message ID, draining");
 }
 // Drain message body (len - 1 bytes for the message ID)
 let body_len = len - 1;
 let mut remaining = body_len;
 while remaining > 0 {
 let take = remaining.min(discard.len());
 tokio::time::timeout(
 inner.body_read_timeout,
 socket.read_exact(&mut discard[..take]),
 )
 .await
 .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "body read"))??;
 remaining -= take;
 }
 }
 Err(e) => {
 if let Some(token) = capture_token.as_ref() {
 tracing::debug!(
 token = %token,
 error = %e,
 "capture: peer disconnected"
 );
 inner.capture.mark_connection_ended(token);
 }
 return Ok(()); // EOF/reset - drop
 }
 }
 }
 _ = keepalive_interval.tick() => {
 if first_keepalive_tick {
 first_keepalive_tick = false;
 continue; // skip the immediate first tick
 }
 if last_recv.elapsed() > inner.idle_timeout {
 if let Some(token) = capture_token.as_ref() {
 tracing::debug!(
 token = %token,
 idle_secs = last_recv.elapsed().as_secs(),
 "capture: peer idle timeout"
 );
 inner.capture.mark_connection_ended(token);
 }
 return Ok(()); // idle - drop
 }
 let _ = write_timeout(socket, &protocol::KEEPALIVE_MSG, inner.write_timeout).await;
 }
 }
 }
}

/// Write a length-prefixed message: 4-byte BE length + 1-byte id + payload.
async fn write_msg(
 socket: &mut TcpStream,
 id: u8,
 payload: &[u8],
 timeout: Duration,
) -> std::io::Result<()> {
 let len = (1 + payload.len()) as u32;
 let mut buf = Vec::with_capacity(4 + 1 + payload.len());
 buf.extend_from_slice(&len.to_be_bytes());
 buf.push(id);
 buf.extend_from_slice(payload);
 write_timeout(socket, &buf, timeout).await
}

/// Build the BEP-10 extension handshake payload.
///
/// The payload starts with a 1-byte extended message ID (0 = handshake),
/// followed by a bencoded dict containing:
/// - `m`: dict mapping extension names to local message IDs
/// - `v`: client name + version string
/// - `reqq`: max outstanding request queue size
///
/// Keys are sorted by `BTreeMap`'s natural byte order, which is canonical
/// bencode ordering.
fn build_ext_handshake(reg: &AuditRegistration, peer_ip: Option<&[u8]>) -> Vec<u8> {
 use crate::bencode::Value;

 // Build the m dict: extension name (String) → local ID (int)
 let mut m = BTreeMap::new();
 for (name, id) in &reg.m_dict {
 m.insert(name.as_bytes().to_vec(), Value::Int(*id as i64));
 }

 // Build the top-level handshake dict
 let mut top = BTreeMap::new();
 top.insert(protocol::K_M.to_vec(), Value::Dict(m));
 top.insert(
 protocol::K_V.to_vec(),
 Value::Bytes(reg.v_string.as_bytes().to_vec()),
 );
 if let Some(reqq) = reg.reqq {
 top.insert(protocol::K_REQQ.to_vec(), Value::Int(reqq as i64));
 }
 if reg.send_upload_only {
 top.insert(protocol::K_UPLOAD_ONLY.to_vec(), Value::Int(1));
 }
 if let Some(ago) = reg.send_complete_ago {
 top.insert(protocol::K_COMPLETE_AGO.to_vec(), Value::Int(ago));
 }
 if let Some(enc) = reg.encryption_preferred {
 top.insert(protocol::K_E.to_vec(), Value::Int(if enc { 1 } else { 0 }));
 }
 if let Some(ip) = peer_ip.filter(|_| reg.send_yourip) {
 top.insert(protocol::K_YOURIP.to_vec(), Value::Bytes(ip.to_vec()));
 }

 // Payload: 1-byte ext-handshake ID + bencoded dict
 let mut buf = vec![protocol::EXT_HANDSHAKE_ID];
 buf.extend_from_slice(&crate::bencode::encode(&Value::Dict(top)));
 buf
}

/// Map a peer-wire message ID to a human-readable name for the behavioral
/// fingerprint. Used only in capture mode.
fn msg_name(id: u8) -> &'static str {
 match id {
 protocol::MSG_CHOKE => "choke",
 protocol::MSG_UNCHOKE => "unchoke",
 protocol::MSG_INTERESTED => "interested",
 protocol::MSG_NOT_INTERESTED => "not_interested",
 protocol::MSG_HAVE => "have",
 protocol::MSG_BITFIELD => "bitfield",
 protocol::MSG_REQUEST => "request",
 protocol::MSG_PIECE => "piece",
 protocol::MSG_CANCEL => "cancel",
 protocol::MSG_PORT => "dht_port",
 protocol::MSG_SUGGEST_PIECE => "suggest_piece",
 protocol::MSG_HAVE_ALL => "have_all",
 protocol::MSG_HAVE_NONE => "have_none",
 protocol::MSG_REJECT_REQUEST => "reject_request",
 protocol::MSG_ALLOWED_FAST => "allowed_fast",
 protocol::MSG_EXTENDED => "ext_handshake",
 _ => "unknown",
 }
}

/// Write data with a timeout.
async fn write_timeout(
 socket: &mut TcpStream,
 data: &[u8],
 timeout: Duration,
) -> std::io::Result<()> {
 tokio::time::timeout(timeout, socket.write_all(data))
 .await
 .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "write"))??;
 Ok(())
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::config::{ClientSpecConfig, PeerServerConfig, KeyFormat};
 use tokio::io::{AsyncReadExt, AsyncWriteExt};

 /// Find an ephemeral port that's free on the system.
 fn free_port() -> u16 {
 let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
 listener.local_addr().unwrap().port()
 }

 fn test_client() -> ClientSpecConfig {
 ClientSpecConfig {
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
 }
 }

 fn fast_client() -> ClientSpecConfig { test_client() }

 fn non_fast_client() -> ClientSpecConfig {
 let mut c = test_client();
 c.fast_extension = false;
 c.reserved_bytes = "0000000000100001".into();
 c
 }

 fn test_peer_server_cfg() -> PeerServerConfig {
 PeerServerConfig {
 enabled: true, max_connections: 10, max_per_ip: 5,
 handshake_timeout_secs: 2, write_timeout_secs: 2, idle_timeout_secs: 5,
 body_read_timeout_secs: 2, accept_error_backoff_ms: 100,
 capture_keepalive_secs: 90,
 }
 }

 const TEST_INFO_HASH: [u8; protocol::INFO_HASH_LEN] = [0xAA; protocol::INFO_HASH_LEN];
 const TEST_PEER_ID: [u8; protocol::PEER_ID_LEN] = [0xBB; protocol::PEER_ID_LEN];

 fn build_handshake(info_hash: &[u8; protocol::INFO_HASH_LEN]) -> [u8; protocol::HANDSHAKE_LEN] {
 let mut hs = [0u8; protocol::HANDSHAKE_LEN];
 hs[0] = protocol::PSTRLEN;
 hs[1..protocol::RESERVED_OFFSET].copy_from_slice(protocol::PSTR);
 // Set LTEP + DHT + Fast Extension bits - simulates a real modern client.
 hs[protocol::RESERVED_OFFSET + protocol::LTEP_BYTE_INDEX] |= protocol::LTEP_BIT_MASK;
 hs[protocol::RESERVED_OFFSET + protocol::DHT_BYTE_INDEX] |= protocol::DHT_BIT_MASK;
 hs[protocol::RESERVED_OFFSET + protocol::FAST_EXT_BYTE_INDEX] |= protocol::FAST_EXT_BIT_MASK;
 hs[protocol::INFO_HASH_OFFSET..protocol::PEER_ID_OFFSET].copy_from_slice(info_hash);
 hs[protocol::PEER_ID_OFFSET..protocol::HANDSHAKE_LEN].copy_from_slice(&TEST_PEER_ID);
 hs
 }

 /// Handshake with LTEP + DHT but NO Fast Extension (simulates rTorrent).
 fn build_handshake_no_fast(info_hash: &[u8; protocol::INFO_HASH_LEN]) -> [u8; protocol::HANDSHAKE_LEN] {
 let mut hs = [0u8; protocol::HANDSHAKE_LEN];
 hs[0] = protocol::PSTRLEN;
 hs[1..protocol::RESERVED_OFFSET].copy_from_slice(protocol::PSTR);
 hs[protocol::RESERVED_OFFSET + protocol::LTEP_BYTE_INDEX] |= protocol::LTEP_BIT_MASK;
 hs[protocol::RESERVED_OFFSET + protocol::DHT_BYTE_INDEX] |= protocol::DHT_BIT_MASK;
 hs[protocol::INFO_HASH_OFFSET..protocol::PEER_ID_OFFSET].copy_from_slice(info_hash);
 hs[protocol::PEER_ID_OFFSET..protocol::HANDSHAKE_LEN].copy_from_slice(&TEST_PEER_ID);
 hs
 }

 /// Handshake with NO LTEP - ext handshake should not be sent.
 fn build_handshake_no_ltep(info_hash: &[u8; protocol::INFO_HASH_LEN]) -> [u8; protocol::HANDSHAKE_LEN] {
 let mut hs = [0u8; protocol::HANDSHAKE_LEN];
 hs[0] = protocol::PSTRLEN;
 hs[1..protocol::RESERVED_OFFSET].copy_from_slice(protocol::PSTR);
 hs[protocol::RESERVED_OFFSET + protocol::FAST_EXT_BYTE_INDEX] |= protocol::FAST_EXT_BIT_MASK;
 hs[protocol::INFO_HASH_OFFSET..protocol::PEER_ID_OFFSET].copy_from_slice(info_hash);
 hs[protocol::PEER_ID_OFFSET..protocol::HANDSHAKE_LEN].copy_from_slice(&TEST_PEER_ID);
 hs
 }

 async fn read_msg(socket: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
 let mut len_buf = [0u8; 4];
 socket.read_exact(&mut len_buf).await?;
 let len = u32::from_be_bytes(len_buf) as usize;
 if len == 0 { return Ok((0xFF, vec![])); }
 let mut msg = vec![0u8; len];
 socket.read_exact(&mut msg).await?;
 Ok((msg[0], msg[1..].to_vec()))
 }

 // Protocol constants

 #[test]
 fn handshake_constants_correct() {
 assert_eq!(protocol::HANDSHAKE_LEN, 68);
 assert_eq!(protocol::PSTRLEN, 19);
 assert_eq!(protocol::PSTR, b"BitTorrent protocol");
 }

 #[test]
 fn message_ids_match_bep3() {
 assert_eq!(protocol::MSG_CHOKE, 0);
 assert_eq!(protocol::MSG_UNCHOKE, 1);
 assert_eq!(protocol::MSG_BITFIELD, 5);
 assert_eq!(protocol::MSG_REQUEST, 6);
 assert_eq!(protocol::MSG_PIECE, 7);
 assert_eq!(protocol::MSG_HAVE_ALL, 14);
 assert_eq!(protocol::MSG_HAVE_NONE, 15);
 assert_eq!(protocol::MSG_EXTENDED, 20);
 }

 #[test]
 fn max_message_len_caps_at_64k() {
 assert_eq!(protocol::MAX_PEER_MSG_LEN, 65536);
 }

 // Integration: full handshake + bitfield + unchoke

 #[tokio::test]
 async fn fast_extension_handshake_responds_with_have_all_and_unchoke() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();

 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 assert_eq!(resp[0], protocol::PSTRLEN);
 assert_eq!(&resp[protocol::INFO_HASH_OFFSET..protocol::PEER_ID_OFFSET], &TEST_INFO_HASH);

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_HAVE_ALL);
 assert!(payload.is_empty());

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_UNCHOKE);
 assert!(payload.is_empty());

 drop(ps);
 }

 #[tokio::test]
 async fn non_fast_extension_handshake_responds_with_bitfield_and_unchoke() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &non_fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake_no_fast(&TEST_INFO_HASH)).await.unwrap();

 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 assert_eq!(resp[0], protocol::PSTRLEN);

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_BITFIELD);
 assert_eq!(payload, protocol::SEEDER_BITFIELD.to_vec());

 let (id, _) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_UNCHOKE);

 drop(ps);
 }

 #[tokio::test]
 async fn wrong_info_hash_silently_dropped() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 let wrong_hash = [0xCC; protocol::INFO_HASH_LEN];
 sock.write_all(&build_handshake(&wrong_hash)).await.unwrap();

 let mut buf = [0u8; 1];
 let result = tokio::time::timeout(Duration::from_secs(3), sock.read(&mut buf)).await;
 match result {
 Err(_) | Ok(Ok(0)) | Ok(Err(_)) => {}
 Ok(Ok(_)) => panic!("server responded to wrong info_hash"),
 }
 drop(ps);
 }

 #[tokio::test]
 async fn unregistered_info_hash_silently_dropped() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 // Don't register any audit - all info hashes should be rejected

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();

 let mut buf = [0u8; 1];
 let result = tokio::time::timeout(Duration::from_secs(3), sock.read(&mut buf)).await;
 match result {
 Err(_) | Ok(Ok(0)) | Ok(Err(_)) => {}
 Ok(Ok(_)) => panic!("server responded to unregistered info_hash"),
 }
 drop(ps);
 }

 #[tokio::test]
 async fn non_bittorrent_handshake_dropped() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();

 let mut buf = [0u8; 1];
 let result = tokio::time::timeout(Duration::from_secs(3), sock.read(&mut buf)).await;
 match result {
 Err(_) | Ok(Ok(0)) | Ok(Err(_)) => {}
 Ok(Ok(_)) => panic!("server responded to non-BitTorrent data"),
 }
 drop(ps);
 }

 #[tokio::test]
 async fn slow_handshake_dropped_after_timeout() {
 let port = free_port();
 let mut cfg = test_peer_server_cfg();
 cfg.handshake_timeout_secs = 1;
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &cfg, crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 let mut buf = [0u8; 1];
 let result = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await;
 match result {
 Err(_) => panic!("timeout waiting for server to drop"),
 Ok(Ok(0)) | Ok(Err(_)) => {}
 Ok(Ok(_)) => panic!("server sent data without receiving a handshake"),
 }
 drop(ps);
 }

 #[tokio::test]
 async fn keepalive_sent_to_idle_peer() {
 let port = free_port();
 let cfg = test_peer_server_cfg();
 let mut client = fast_client();
 client.keepalive_secs = 1;
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &cfg, crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke
 let _ = read_msg(&mut sock).await.unwrap(); // ext handshake

 let result = tokio::time::timeout(Duration::from_secs(5), read_msg(&mut sock)).await;
 match result {
 Err(_) => panic!("no keepalive received"),
 Ok(Ok((0xFF, _))) => {}
 Ok(Ok((id, _))) => panic!("expected keepalive, got id={id}"),
 Ok(Err(_)) => panic!("error reading keepalive"),
 }
 drop(ps);
 }

 #[tokio::test]
 async fn disabled_server_rejects_connections() {
 let port = free_port();
 let mut cfg = test_peer_server_cfg();
 cfg.enabled = false;
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &cfg, crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 let result = TcpStream::connect(format!("127.0.0.1:{port}")).await;
 assert!(result.is_err(), "disabled server should not accept connections");
 drop(ps);
 }

 #[tokio::test]
 async fn piece_message_from_peer_drops_connection() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke
 let _ = read_msg(&mut sock).await.unwrap(); // ext handshake

 // Drain any buffered keepalive (the keepalive `interval` fires an
 // immediate first tick, so one may be queued alongside the ext
 // handshake depending on scheduling). Without draining, it would be
 // mistaken for a response to the piece message below.
 while let Ok(Ok(_)) =
 tokio::time::timeout(Duration::from_millis(150), read_msg(&mut sock)).await
 {}

 let piece_msg = {
 let mut buf = Vec::new();
 buf.extend_from_slice(&13u32.to_be_bytes());
 buf.push(protocol::MSG_PIECE);
 buf.extend_from_slice(&0u32.to_be_bytes());
 buf.extend_from_slice(&0u32.to_be_bytes());
 buf.extend_from_slice(&[0u8; 4]);
 buf
 };
 sock.write_all(&piece_msg).await.unwrap();

 let mut buf = [0u8; 1];
 let result = tokio::time::timeout(Duration::from_secs(3), sock.read(&mut buf)).await;
 match result {
 Err(_) => panic!("server didn't drop after piece message"),
 Ok(Ok(0)) | Ok(Err(_)) => {}
 Ok(Ok(_)) => panic!("server responded to piece message"),
 }
 drop(ps);
 }

 #[tokio::test]
 async fn multiple_audits_share_one_listener() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();

 // Register two audits with different info hashes
 let hash1 = [0xAA; protocol::INFO_HASH_LEN];
 let hash2 = [0xBB; protocol::INFO_HASH_LEN];
 let peer_id1 = [0x11; protocol::PEER_ID_LEN];
 let peer_id2 = [0x22; protocol::PEER_ID_LEN];

 ps.register(hash1, peer_id1, &fast_client()).unwrap();
 ps.register(hash2, peer_id2, &fast_client()).unwrap();

 // Connect with hash1 - should get peer_id1
 let mut sock1 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock1.write_all(&build_handshake(&hash1)).await.unwrap();
 let mut resp1 = [0u8; protocol::HANDSHAKE_LEN];
 sock1.read_exact(&mut resp1).await.unwrap();
 assert_eq!(&resp1[protocol::PEER_ID_OFFSET..protocol::HANDSHAKE_LEN], &peer_id1);

 // Connect with hash2 - should get peer_id2
 let mut sock2 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock2.write_all(&build_handshake(&hash2)).await.unwrap();
 let mut resp2 = [0u8; protocol::HANDSHAKE_LEN];
 sock2.read_exact(&mut resp2).await.unwrap();
 assert_eq!(&resp2[protocol::PEER_ID_OFFSET..protocol::HANDSHAKE_LEN], &peer_id2);

 // Deregister hash1 - new connections for hash1 should be dropped
 ps.deregister(&hash1);
 let mut sock3 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock3.write_all(&build_handshake(&hash1)).await.unwrap();
 let mut buf = [0u8; 1];
 let result = tokio::time::timeout(Duration::from_secs(3), sock3.read(&mut buf)).await;
 match result {
 Err(_) | Ok(Ok(0)) | Ok(Err(_)) => {}
 Ok(Ok(_)) => panic!("server responded to deregistered info_hash"),
 }

 drop(ps);
 }

 // BEP-10 extension handshake

 #[tokio::test]
 async fn ext_handshake_sent_when_ltep_bit_set() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();

 let mut client = test_client();
 client.v_string = "qBittorrent/5.2.2".into();
 client.m_dict.insert("ut_pex".into(), 1);
 client.m_dict.insert("ut_metadata".into(), 2);
 client.reqq = Some(2000);

 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_EXTENDED, "expected MSG_EXTENDED");
 assert!(!payload.is_empty());

 // First byte is the ext message sub-ID (0 = handshake)
 assert_eq!(payload[0], protocol::EXT_HANDSHAKE_ID);

 // Rest is a bencoded dict - decode and check keys
 let dict = crate::bencode::decode(&payload[1..]).unwrap();
 assert_eq!(dict.get(b"v").unwrap().as_str(), Some("qBittorrent/5.2.2"));
 assert_eq!(dict.get(b"reqq").unwrap().as_int(), Some(2000));

 let m = dict.get(b"m").unwrap();
 let m_dict = m.as_dict().unwrap();
 assert_eq!(m_dict.get(b"ut_pex".as_slice()).unwrap().as_int(), Some(1));
 assert_eq!(m_dict.get(b"ut_metadata".as_slice()).unwrap().as_int(), Some(2));

 drop(ps);
 }

 #[tokio::test]
 async fn ext_handshake_not_sent_when_ltep_bit_clear() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();

 let mut client = test_client();
 // Clear the LTEP bit (byte 5) while keeping DHT + Fast Ext (byte 7)
 client.reserved_bytes = "0000000000000005".into();

 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke

 // Next message should be a keepalive, NOT an ext handshake.
 // Wait briefly - if an ext handshake arrives, it's a bug.
 let result = tokio::time::timeout(Duration::from_millis(500), read_msg(&mut sock)).await;
 match result {
 Err(_) => {} // timeout = no more messages = correct (no ext handshake)
 Ok(Ok((0xFF, _))) => {} // keepalive = correct
 Ok(Ok((id, _))) => panic!("expected no ext handshake, got id={id}"),
 Ok(Err(_)) => {}
 }
 drop(ps);
 }

 #[tokio::test]
 async fn ext_handshake_not_sent_when_peer_ltep_bit_clear() {
 // When the PEER doesn't support LTEP, we should NOT send an ext handshake
 // even if our config has the LTEP bit set.
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake_no_ltep(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all (peer has Fast Ext)
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke

 // No ext handshake should follow - peer doesn't support LTEP.
 let result = tokio::time::timeout(Duration::from_millis(500), read_msg(&mut sock)).await;
 match result {
 Err(_) => {} // timeout = no more messages = correct
 Ok(Ok((0xFF, _))) => {} // keepalive = correct
 Ok(Ok((id, _))) => panic!("expected no ext handshake (peer has no LTEP), got id={id}"),
 Ok(Err(_)) => {}
 }
 drop(ps);
 }

 #[tokio::test]
 async fn ext_handshake_empty_m_dict() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 let client = test_client(); // m_dict is empty in test_client
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap();

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_EXTENDED);

 let dict = crate::bencode::decode(&payload[1..]).unwrap();
 // m dict should be present but empty
 let m = dict.get(b"m").unwrap();
 assert!(matches!(m, crate::bencode::Value::Dict(d) if d.is_empty()));
 assert_eq!(dict.get(b"v").unwrap().as_str(), Some("TestClient/1.0"));
 assert_eq!(dict.get(b"reqq").unwrap().as_int(), Some(500));

 drop(ps);
 }

 #[tokio::test]
 async fn ext_handshake_includes_upload_only_and_complete_ago() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 let mut client = test_client();
 client.send_upload_only = true;
 client.send_complete_ago = Some(-1);
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_EXTENDED);
 let dict = crate::bencode::decode(&payload[1..]).unwrap();
 assert_eq!(dict.get(b"upload_only").and_then(|v| v.as_int()), Some(1), "upload_only should be 1");
 assert_eq!(dict.get(b"complete_ago").and_then(|v| v.as_int()), Some(-1), "complete_ago should be -1");
 drop(ps);
 }

 #[tokio::test]
 async fn ext_handshake_includes_encryption_preferred() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 let mut client = test_client();
 client.encryption_preferred = Some(true);
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_EXTENDED);
 let dict = crate::bencode::decode(&payload[1..]).unwrap();
 assert_eq!(dict.get(b"e").and_then(|v| v.as_int()), Some(1), "e should be 1");
 drop(ps);
 }

 #[tokio::test]
 async fn ext_handshake_includes_yourip() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 let client = test_client(); // send_yourip defaults to true
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap(); // have_all
 let _ = read_msg(&mut sock).await.unwrap(); // unchoke

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_EXTENDED);
 let dict = crate::bencode::decode(&payload[1..]).unwrap();
 // yourip should be present and contain 4 bytes (127.0.0.1) or 16 bytes (IPv6)
 let yourip = dict.get(b"yourip").and_then(|v| v.as_bytes());
 assert!(yourip.is_some(), "yourip should be present");
 assert!(yourip.unwrap().len() == 4 || yourip.unwrap().len() == 16, "yourip should be 4 or 16 bytes");
 drop(ps);
 }

 #[tokio::test]
 async fn ext_handshake_omits_upload_only_when_disabled() {
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 let mut client = test_client();
 client.send_upload_only = false;
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &client).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 sock.write_all(&build_handshake(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap();
 let _ = read_msg(&mut sock).await.unwrap();

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_EXTENDED);
 let dict = crate::bencode::decode(&payload[1..]).unwrap();
 assert!(dict.get(b"upload_only").is_none(), "upload_only should be omitted when send_upload_only=false");
 drop(ps);
 }

 #[tokio::test]
 async fn fast_ext_sends_bitfield_to_non_fast_ext_peer() {
 // When WE support Fast Ext but the PEER doesn't, we should send a
 // bitfield (not have_all), since Fast Extension requires both peers to set the bit.
 let port = free_port();
 let ps = PeerServer::start(format!("127.0.0.1:{port}"), &test_peer_server_cfg(), crate::capture::CaptureStore::new(tokio::sync::broadcast::channel(1).0)).unwrap();
 ps.register(TEST_INFO_HASH, TEST_PEER_ID, &fast_client()).unwrap();

 let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
 // Peer has LTEP but NO Fast Extension
 sock.write_all(&build_handshake_no_fast(&TEST_INFO_HASH)).await.unwrap();
 let mut resp = [0u8; protocol::HANDSHAKE_LEN];
 sock.read_exact(&mut resp).await.unwrap();

 let (id, payload) = read_msg(&mut sock).await.unwrap();
 assert_eq!(id, protocol::MSG_BITFIELD, "should send bitfield (not have_all) when peer lacks Fast Ext");
 assert_eq!(payload, protocol::SEEDER_BITFIELD.to_vec());
 drop(ps);
 }
}
