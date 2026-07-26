//! Fingerprint capture - accepts tracker announces and peer-wire handshakes
//! from real BitTorrent clients to extract their exact fingerprint.
//!
//! The capture flow:
//! 1. `start()` mints a token, generates a dummy `.torrent` with our tracker
//!    URL, and stores a `CaptureSession`.
//! 2. The client announces to `GET /capture/{token}/announce` - we record
//!    `peer_id`, `key`, `numwant`, raw query-param order, and `User-Agent`.
//! 3. The tracker responds with our IP + peer_port as the only peer.
//! 4. The client connects to our peer_server - we record `reserved_bytes`,
//!    `peer_id` (cross-check), and the BEP-10 extension handshake (`v`, `m`,
//!    `reqq`).
//! 5. The UI shows the full fingerprint and a TOML snippet.
//!
//! Session state is in-memory (`Mutex<HashMap>`) - lost on restart. This is
//! intentional: captures are short-lived interactive sessions, not persistent
//! data.

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::bencode;
use crate::data::protocol;
use crate::engine::AppEvent;

// Azureus-style peer_id prefix decoder
//
// The peer_id prefix `-XXYYYY-` is the most reliable client identity channel:
// it's always present (BEP-3 mandate), structured (2-char code + version
// block), and decoded per-client. The 2-char code maps to a client name via a
// lookup table (it's a mnemonic, not derivable). Version decoding varies per
// client - see BEP-20 and Transmission's clients.cc for the reference.
//
// This decoder covers the 7 emulated clients in config.toml plus the common
// ones a capture session might encounter. Unknown codes return None.

/// Decode a single base62 character to its numeric value (0-61).
/// `0-9` = 0-9, `A-Z` = 10-35, `a-z` = 36-61.
fn base62(c: u8) -> Option<u32> {
 match c {
 b'0'..=b'9' => Some((c - b'0') as u32),
 b'A'..=b'Z' => Some((c - b'A' + 10) as u32),
 b'a'..=b'z' => Some((c - b'a' + 36) as u32),
 _ => None,
 }
}

/// Decode an Azureus-style peer_id prefix (`-XXYYYY-`, 8 bytes) into
/// `(client_name, version)`. Returns `None` for unknown codes or malformed
/// prefixes.
///
/// The version block (bytes 3-6) is decoded per-client:
/// - 3-digit base62 + trailing `0` (qBittorrent): `5220` → `5.2.2`
/// - 4-digit (Transmission ≥4.0): `4120` → `4.1.2`, suffix `0`=stable
/// - 3-digit + release char (Deluge): `220s` → `2.2.0`
/// - Hex (µTorrent/BitTorrent): `3550` → `3.5.5`
/// - 3-digit base62 (rTorrent/libTorrent): `098` → `0.9.8`
/// - 4-digit base62 (Vuze/Azureus): `5750` → `5.7.5.0`
pub fn decode_peer_id_prefix(prefix: &str) -> Option<(String, String)> {
 let bytes = prefix.as_bytes();
 if bytes.len() < 7 || bytes[0] != b'-' {
 return None;
 }
 // Extract the 2-char code (bytes 1-2) and the version block (bytes 3+).
 let code = &prefix[1..3];
 let ver_block = &prefix[3..];

 let (name, version) = match code {
 "qB" => {
 // qBittorrent: 3-digit base62, byte 6 is always '0' (ignored).
 // -qB5220- → 5.2.2
 let b = ver_block.as_bytes();
 if b.len() < 4 { return None; }
 let maj = base62(b[0])?;
 let min = base62(b[1])?;
 let pat = base62(b[2])?;
 (protocol::BEP20_CLIENT_QBITTORRENT.to_string(), format!("{maj}.{min}.{pat}"))
 }
 "TR" => {
 // Transmission ≥4.0: -TRXYZR- (base62, R=suffix)
 // -TR4120- → 4.1.2 stable
 let b = ver_block.as_bytes();
 if b.len() < 4 { return None; }
 let maj = base62(b[0])?;
 let min = base62(b[1])?;
 let pat = base62(b[2])?;
 (protocol::BEP20_CLIENT_TRANSMISSION.to_string(), format!("{maj}.{min}.{pat}"))
 }
 "DE" => {
 // Deluge: 3-digit + release char (s=stable, D=dev, a=alpha, b=beta)
 // -DE220s- → 2.2.0
 let b = ver_block.as_bytes();
 if b.len() < 4 { return None; }
 let maj = base62(b[0])?;
 let min = base62(b[1])?;
 let pat = base62(b[2])?;
 (protocol::BEP20_CLIENT_DELUGE.to_string(), format!("{maj}.{min}.{pat}"))
 }
 "UT" | "UM" | "UE" | "UW" => {
 // µTorrent family: hex encoding
 // -UT3550- → 3.5.5
 let b = ver_block.as_bytes();
 if b.len() < 4 { return None; }
 let maj = bencode::hex_nibble(b[0]).ok()?;
 let min = bencode::hex_nibble(b[1]).ok()?;
 let pat = bencode::hex_nibble(b[2]).ok()?;
 (protocol::BEP20_CLIENT_UTORRENT.to_string(), format!("{maj}.{min}.{pat}"))
 }
 "BT" | "BW" => {
 // BitTorrent Mainline: hex encoding (same as µTorrent)
 // -BT7B00- → 7.11.0 (B=11 in hex), -BT7110- → 7.1.1
 let b = ver_block.as_bytes();
 if b.len() < 4 { return None; }
 let maj = bencode::hex_nibble(b[0]).ok()?;
 let min = bencode::hex_nibble(b[1]).ok()?;
 let pat = bencode::hex_nibble(b[2]).ok()?;
 (protocol::BEP20_CLIENT_BITTORRENT.to_string(), format!("{maj}.{min}.{pat}"))
 }
 "lt" => {
 // libTorrent (Rakshasa) / rTorrent: 3-digit base62
 // -lt098- → 0.9.8
 let b = ver_block.as_bytes();
 if b.len() < 3 { return None; }
 let maj = base62(b[0])?;
 let min = base62(b[1])?;
 let pat = base62(b[2])?;
 (protocol::BEP20_CLIENT_RTORRENT.to_string(), format!("{maj}.{min}.{pat}"))
 }
 "AZ" => {
 // Vuze/Azureus: 4-digit base62
 // -AZ5750- → 5.7.5.0
 let b = ver_block.as_bytes();
 if b.len() < 4 { return None; }
 let maj = base62(b[0])?;
 let min = base62(b[1])?;
 let pat = base62(b[2])?;
 let bld = base62(b[3])?;
 (protocol::BEP20_CLIENT_VUZE.to_string(), format!("{maj}.{min}.{pat}.{bld}"))
 }
 "LT" => {
 // libtorrent (Rasterbar): 3-digit base62
 let b = ver_block.as_bytes();
 if b.len() < 3 { return None; }
 let maj = base62(b[0])?;
 let min = base62(b[1])?;
 let pat = base62(b[2])?;
 (protocol::BEP20_CLIENT_LIBTORRENT.to_string(), format!("{maj}.{min}.{pat}"))
 }
 _ => return None,
 };

 Some((name, version))
}

/// Fields extracted from the tracker announce request. Grouped to keep
/// `record_announce` under clippy's argument limit.
#[derive(Debug, Clone, Default)]
pub struct AnnounceData {
 pub peer_id: [u8; protocol::PEER_ID_LEN],
 pub user_agent: String,
 pub query_param_order: Vec<String>,
 pub raw_query: String,
 pub numwant: Option<u32>,
 pub http_headers: Vec<(String, String)>,
}

/// Fields extracted from the BEP-10 extension handshake. Grouped to keep
/// `record_ext_handshake` under clippy's argument limit. All fields are
/// optional per BEP-10.
#[derive(Debug, Clone, Default)]
pub struct ExtHandshakeData {
 pub v_string: Option<String>,
 pub m_dict: Option<BTreeMap<String, u32>>,
 pub reqq: Option<u32>,
 pub encryption_preferred: Option<bool>,
 pub upload_only: Option<bool>,
 pub complete_ago: Option<i64>,
 /// Peer's compact IP as seen by us (4 or 16 bytes).
 pub yourip: Option<Vec<u8>>,
 /// Listen port (BEP-10 `p` field, outgoing connections only).
 pub listen_port: Option<u16>,
 /// Info-dict size in bytes (BEP-9, sent when ut_metadata is advertised).
 pub metadata_size: Option<u64>,
 /// Compact IPv4 bind address (4 bytes).
 pub ipv4: Option<Vec<u8>>,
 /// Compact IPv6 bind address (16 bytes).
 pub ipv6: Option<Vec<u8>>,
 /// Share-mode flag (libtorrent-specific).
 pub share_mode: Option<bool>,
}

/// Maximum number of peer-wire messages to record for the behavioral
/// fingerprint (message ordering). Enough to capture the initial handshake
/// sequence without unbounded memory.
const MAX_CAPTURE_MESSAGES: usize = 10;

/// Capture progress status - advances as each phase completes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStatus {
 WaitingForAnnounce,
 AnnounceCaptured,
 HandshakeCaptured,
 ExtHandshakeCaptured,
}

/// Fields captured from the tracker announce and peer-wire handshake.
/// All fields are `Option` - filled in as each phase completes.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CapturedFingerprint {
 // From HTTP tracker announce:
 pub peer_id: Option<[u8; protocol::PEER_ID_LEN]>,
 pub user_agent: Option<String>,
 /// Ordered list of query parameter names (e.g. ["info_hash", "peer_id", "port", ...]).
 pub query_param_order: Option<Vec<String>>,
 /// The raw query string from the announce request (e.g. "info_hash=...&peer_id=...&port=...").
 /// Used to reconstruct the `query` template for config.toml.
 pub raw_query: Option<String>,
 pub numwant: Option<u32>,
 /// All HTTP headers from the announce request, in the order received.
 /// Trackers may fingerprint on Accept-Encoding, Connection, etc.
 pub http_headers: Option<Vec<(String, String)>>,
 // From peer-wire handshake:
 pub reserved_bytes: Option<[u8; protocol::RESERVED_LEN]>,
 pub handshake_peer_id: Option<[u8; protocol::PEER_ID_LEN]>,
 // From BEP-10 extension handshake:
 pub v_string: Option<String>,
 pub m_dict: Option<BTreeMap<String, u32>>,
 pub reqq: Option<u32>,
 /// Encryption preference (BEP-10 `e` field, 0/1).
 pub encryption_preferred: Option<bool>,
 /// Upload-only flag (BEP-21, 0/1) - whether the client is a partial seed.
 pub upload_only: Option<bool>,
 /// Seconds since the torrent completed (libtorrent-specific `complete_ago`).
 pub complete_ago: Option<i64>,
 /// Peer's compact IP as seen by us (4 or 16 bytes).
 pub yourip: Option<Vec<u8>>,
 /// Listen port (BEP-10 `p` field).
 pub listen_port: Option<u16>,
 /// Info-dict size in bytes (BEP-9 `metadata_size`).
 pub metadata_size: Option<u64>,
 /// Compact IPv4 bind address (4 bytes).
 pub ipv4: Option<Vec<u8>>,
 /// Compact IPv6 bind address (16 bytes).
 pub ipv6: Option<Vec<u8>>,
 /// Share-mode flag (libtorrent-specific).
 pub share_mode: Option<bool>,
 // From peer-wire message stream (behavioral fingerprint):
 /// Ordered list of the first N message names after the handshake
 /// (e.g. ["ext_handshake", "interested", "have_none"]).
 pub message_order: Option<Vec<String>>,
 /// Measured keepalive interval in seconds (from peer's keepalive cadence).
 pub keepalive_secs: Option<u64>,
}

/// Serializable view of [`CapturedFingerprint`] - converts raw byte arrays to
/// the string representations the frontend expects: `peer_id_prefix` (first 8
/// bytes as lossy UTF-8), `reserved_bytes` (16-char hex), `http_headers`
/// (joined `"Key: Value"` strings). This mirrors the conversion the old
/// polling endpoint did inline; the SSE `capture_progress` event carries this
/// view so `serde_json::to_string` produces the same JSON shape.
///
/// `label` and `version` are decoded from the `peer_id_prefix` (the most
/// reliable identity channel) via [`decode_peer_id_prefix`]. The frontend
/// uses these directly - no client-side decoding needed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CaptureFingerprintView {
 pub label: String,
 pub version: String,
 pub peer_id_prefix: Option<String>,
 pub user_agent: Option<String>,
 pub query_param_order: Option<Vec<String>>,
 pub raw_query: Option<String>,
 pub numwant: Option<u32>,
 pub http_headers: Option<Vec<String>>,
 pub reserved_bytes: Option<String>,
 pub v_string: Option<String>,
 pub m_dict: Option<BTreeMap<String, u32>>,
 pub reqq: Option<u32>,
 pub encryption_preferred: Option<bool>,
 pub upload_only: Option<bool>,
 pub complete_ago: Option<i64>,
 pub yourip: Option<String>,
 pub listen_port: Option<u16>,
 pub metadata_size: Option<u64>,
 pub ipv4: Option<String>,
 pub ipv6: Option<String>,
 pub share_mode: Option<bool>,
 pub message_order: Option<Vec<String>>,
 pub keepalive_secs: Option<u64>,
}

impl CapturedFingerprint {
 /// Convert to the serializable view the frontend expects.
 pub fn to_view(&self) -> CaptureFingerprintView {
 let peer_id_prefix = self.peer_id.map(|id| String::from_utf8_lossy(&id[..8]).to_string());
 let (label, version) = peer_id_prefix
 .as_ref()
 .and_then(|p| decode_peer_id_prefix(p))
 .unwrap_or_else(|| {
 // Fall back to v_string or user_agent if the prefix is unknown.
 let raw = self.v_string.as_deref().or(self.user_agent.as_deref()).unwrap_or("Captured Client");
 let (l, v) = split_identity_string(raw);
 (l.to_string(), v.to_string())
 });
 CaptureFingerprintView {
 label,
 version,
 peer_id_prefix,
 user_agent: self.user_agent.clone(),
 query_param_order: self.query_param_order.clone(),
 raw_query: self.raw_query.clone(),
 numwant: self.numwant,
 http_headers: self.http_headers.as_ref().map(|h| h.iter().map(|(k, v)| format!("{k}: {v}")).collect()),
 reserved_bytes: self.reserved_bytes.map(|rb| crate::bencode::hex_encode(&rb)),
 v_string: self.v_string.clone(),
 m_dict: self.m_dict.clone(),
 reqq: self.reqq,
 encryption_preferred: self.encryption_preferred,
 upload_only: self.upload_only,
 complete_ago: self.complete_ago,
 yourip: self.yourip.as_ref().and_then(|v| compact_ip_to_string(v.as_slice())),
 listen_port: self.listen_port,
 metadata_size: self.metadata_size,
 ipv4: self.ipv4.as_ref().filter(|b| b.len() == 4).map(|b| ipv4_to_string([b[0], b[1], b[2], b[3]])),
 ipv6: self.ipv6.as_ref().filter(|b| b.len() == 16).map(|b| {
 let mut arr = [0u8; 16];
 arr.copy_from_slice(b);
 ipv6_to_string(arr)
 }),
 share_mode: self.share_mode,
 message_order: self.message_order.clone(),
 keepalive_secs: self.keepalive_secs,
 }
 }
}

/// Convert a compact IP byte slice (4 bytes IPv4 or 16 bytes IPv6) to a string.
/// Delegates to the typed `ipv4_to_string`/`ipv6_to_string` helpers to avoid
/// duplicating the `Ipv4Addr`/`Ipv6Addr` formatting logic.
fn compact_ip_to_string(bytes: &[u8]) -> Option<String> {
 match bytes.len() {
 4 => Some(ipv4_to_string([bytes[0], bytes[1], bytes[2], bytes[3]])),
 16 => {
 let mut arr = [0u8; 16];
 arr.copy_from_slice(bytes);
 Some(ipv6_to_string(arr))
 }
 _ => None,
 }
}

fn ipv4_to_string(b: [u8; 4]) -> String {
 std::net::Ipv4Addr::from(b).to_string()
}

fn ipv6_to_string(b: [u8; 16]) -> String {
 std::net::Ipv6Addr::from(b).to_string()
}

/// Split a free-form identity string ("qBittorrent/5.2.2", "Transmission 4.1.2",
/// "curl/7.81.0") into `(label, version)`. Tries `/` then space as separator.
/// Returns `(raw, "unknown")` if no version-like token is found.
fn split_identity_string(raw: &str) -> (&str, &str) {
 let sep = raw.find('/').or_else(|| raw.find(' '));
 match sep {
 Some(idx) => {
 let label = &raw[..idx];
 let after = &raw[idx + 1..];
 let ver_end = after.find(|c: char| c.is_whitespace() || c == ';').unwrap_or(after.len());
 let version = &after[..ver_end];
 if version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
 (label, version)
 } else {
 (raw, "unknown")
 }
 }
 None => (raw, "unknown"),
 }
}

/// A single capture session - one real client being fingerprinted.
pub struct CaptureSession {
 pub info_hash: [u8; protocol::INFO_HASH_LEN],
 /// IP advertised in the tracker response (peer IP). Should be a non-loopback
 /// IP so BT clients don't skip it as a self-connection.
 pub our_ip: Ipv4Addr,
 pub peer_port: u16,
 /// The peer_id we advertise to the real BT client during capture.
 /// Generated at session start so it matches the torrent filename.
 pub our_peer_id: [u8; protocol::PEER_ID_LEN],
 pub fingerprint: Mutex<CapturedFingerprint>,
 pub status: Mutex<CaptureStatus>,
}

/// Lightweight view of a session for the tracker handler (no locks needed).
#[derive(Debug, Clone)]
pub struct CaptureSessionRef {
 /// IP advertised in the tracker response (peer IP for the BT client to connect to).
 pub peer_ip: Ipv4Addr,
 pub peer_port: u16,
}

/// Shared capture session store - one `Arc` cloned between the axum handlers
/// and the peer_server. Uses `std::sync::Mutex` (not `tokio::sync`) because
/// operations are quick HashMap lookups/inserts, not async I/O.
///
/// Holds a `broadcast::Sender<AppEvent>` so state-machine transitions (announce
/// captured, handshake captured, ext-handshake captured, keepalive measured,
/// connection ended) can push live updates through the global SSE stream - no
/// polling.
#[derive(Clone)]
pub struct CaptureStore {
 sessions: Arc<Mutex<HashMap<String, CaptureSession>>>,
 events_tx: broadcast::Sender<AppEvent>,
}

impl CaptureStore {
 pub fn new(events_tx: broadcast::Sender<AppEvent>) -> Self {
 Self {
 sessions: Arc::new(Mutex::new(HashMap::new())),
 events_tx,
 }
 }

 /// Broadcast a capture-progress event over the global SSE stream. Called
 /// after each state-machine transition. The `let _ =` discards the
 /// "no subscribers" error - the capture works even with no SSE listeners.
 fn emit(&self, token: &str, status: CaptureStatus, fingerprint: &CapturedFingerprint) {
 let _ = self.events_tx.send(AppEvent::CaptureProgress {
 token: token.to_string(),
 status,
 fingerprint: Box::new(fingerprint.to_view()),
 });
 }

 /// `announce_port` is the web-server port for the capture torrent's announce
 /// URL - the port the BT client must connect back to. The caller resolves it
 /// from the request `Host` header, falling back to the configured
 /// `server.bind_addr` port.
 ///
 /// `announce_ip` is used for the torrent's announce URL (must be reachable
 /// from the BT client - e.g. 127.0.0.1 when testing locally, or the NAT
 /// public IP / detected LAN IP across the LAN).
 ///
 /// `peer_ip` is advertised in the tracker response (must be non-loopback so
 /// BT clients don't skip it as a self-connection - e.g. the LAN IP).
 ///
 /// Returns `(token, torrent_bytes)`.
 pub fn start(&self, announce_port: &str, announce_ip: Ipv4Addr, peer_ip: Ipv4Addr, peer_port: u16, torrent_name: &str, our_peer_id: [u8; protocol::PEER_ID_LEN]) -> (String, Vec<u8>) {
 let token = mint_token();
 let announce_path = protocol::CAPTURE_ANNOUNCE_PATH.replace("{token}", &token);
 let announce_url = format!("http://{announce_ip}:{announce_port}{announce_path}");
 let (torrent_bytes, info_hash) = crate::torrent::generate(&announce_url, torrent_name);
 let session = CaptureSession {
 info_hash,
 our_ip: peer_ip,
 peer_port,
 our_peer_id,
 fingerprint: Mutex::new(CapturedFingerprint::default()),
 status: Mutex::new(CaptureStatus::WaitingForAnnounce),
 };
 self.sessions
 .lock()
 .unwrap()
 .insert(token.clone(), session);
 (token, torrent_bytes)
 }

 /// Look up a session by token (for the tracker announce handler).
 /// Returns a lightweight ref with the fields needed to build the response.
 pub fn get_by_token(&self, token: &str) -> Option<CaptureSessionRef> {
 let sessions = self.sessions.lock().unwrap();
 sessions.get(token).map(|s| CaptureSessionRef {
 peer_ip: s.our_ip,
 peer_port: s.peer_port,
 })
 }

 /// Look up a session by info_hash (for the peer_server capture mode).
 /// Returns the token if found, so the peer_server can record handshake data.
 pub fn find_by_info_hash(&self, info_hash: &[u8; protocol::INFO_HASH_LEN]) -> Option<String> {
 let sessions = self.sessions.lock().unwrap();
 sessions
 .iter()
 .find(|(_, s)| &s.info_hash == info_hash)
 .map(|(token, _)| token.clone())
 }

 /// Get the peer_id we advertise for a capture session (for the peer_server
 /// to use in the wire handshake, so it matches the torrent filename).
 pub fn get_our_peer_id(&self, token: &str) -> Option<[u8; protocol::PEER_ID_LEN]> {
 let sessions = self.sessions.lock().unwrap();
 sessions.get(token).map(|s| s.our_peer_id)
 }

 /// Record announce fields (called by the tracker announce handler).
 ///
 /// Only the **first** announce is recorded - subsequent announces from
 /// other clients using the same torrent are rejected. This prevents
 /// fingerprint mixing when the same `.torrent` is loaded on multiple
 /// clients simultaneously. Returns `true` if recorded, `false` if a
 /// previous announce already locked the session.
 pub fn record_announce(&self, token: &str, data: AnnounceData) -> bool {
 let result = {
 let sessions = self.sessions.lock().unwrap();
 let Some(session) = sessions.get(token) else { return false; };
 let mut fp = session.fingerprint.lock().unwrap();
 if fp.peer_id.is_some() {
 return false; // already locked to another client
 }
 fp.peer_id = Some(data.peer_id);
 fp.user_agent = Some(data.user_agent);
 fp.query_param_order = Some(data.query_param_order);
 fp.raw_query = Some(data.raw_query);
 fp.numwant = data.numwant;
 fp.http_headers = Some(data.http_headers);
 let mut status = session.status.lock().unwrap();
 *status = CaptureStatus::AnnounceCaptured;
 (fp.clone(), CaptureStatus::AnnounceCaptured)
 };
 self.emit(token, result.1, &result.0);
 true
 }

 /// Record peer-wire handshake fields (called by the peer_server).
 ///
 /// Only the **first** handshake is recorded - subsequent connections from
 /// other clients are rejected. Returns `true` if recorded.
 pub fn record_handshake(&self, token: &str, reserved_bytes: [u8; protocol::RESERVED_LEN], peer_id: [u8; protocol::PEER_ID_LEN]) -> bool {
 let result = {
 let sessions = self.sessions.lock().unwrap();
 let Some(session) = sessions.get(token) else { return false; };
 let mut fp = session.fingerprint.lock().unwrap();
 if fp.reserved_bytes.is_some() {
 return false; // already locked to another client
 }
 fp.reserved_bytes = Some(reserved_bytes);
 fp.handshake_peer_id = Some(peer_id);
 let mut status = session.status.lock().unwrap();
 *status = CaptureStatus::HandshakeCaptured;
 (fp.clone(), CaptureStatus::HandshakeCaptured)
 };
 self.emit(token, result.1, &result.0);
 true
 }

 /// Record BEP-10 extension handshake fields (called by the peer_server).
 ///
 /// All fields are optional per BEP-10 - we record whatever the client
 /// sends and leave the rest as `None`. Only the **first** ext handshake
 /// is recorded. Returns `true` if recorded.
 pub fn record_ext_handshake(&self, token: &str, data: ExtHandshakeData) -> bool {
 let result = {
 let sessions = self.sessions.lock().unwrap();
 let Some(session) = sessions.get(token) else { return false; };
 let mut fp = session.fingerprint.lock().unwrap();
 if fp.v_string.is_some() || fp.m_dict.is_some() || fp.reqq.is_some() {
 return false; // already locked to another client
 }
 if data.v_string.is_some() { fp.v_string = data.v_string; }
 if data.m_dict.is_some() { fp.m_dict = data.m_dict; }
 if data.reqq.is_some() { fp.reqq = data.reqq; }
 if data.encryption_preferred.is_some() { fp.encryption_preferred = data.encryption_preferred; }
 if data.upload_only.is_some() { fp.upload_only = data.upload_only; }
 if data.complete_ago.is_some() { fp.complete_ago = data.complete_ago; }
 if data.yourip.is_some() { fp.yourip = data.yourip; }
 if data.listen_port.is_some() { fp.listen_port = data.listen_port; }
 if data.metadata_size.is_some() { fp.metadata_size = data.metadata_size; }
 if data.ipv4.is_some() { fp.ipv4 = data.ipv4; }
 if data.ipv6.is_some() { fp.ipv6 = data.ipv6; }
 if data.share_mode.is_some() { fp.share_mode = data.share_mode; }
 let mut status = session.status.lock().unwrap();
 *status = CaptureStatus::ExtHandshakeCaptured;
 (fp.clone(), CaptureStatus::ExtHandshakeCaptured)
 };
 self.emit(token, result.1, &result.0);
 true
 }

 /// Record a peer-wire message for the behavioral fingerprint (message
 /// ordering). Called by the peer_server for each message received during
 /// a capture session. Only the first `MAX_CAPTURE_MESSAGES` are stored.
 pub fn record_peer_message(&self, token: &str, msg_name: &str) {
 let sessions = self.sessions.lock().unwrap();
 let Some(session) = sessions.get(token) else { return; };
 let mut fp = session.fingerprint.lock().unwrap();
 let msgs = fp.message_order.get_or_insert_with(Vec::new);
 if msgs.len() < MAX_CAPTURE_MESSAGES {
 msgs.push(msg_name.to_string());
 }
 }

 /// Record the measured keepalive interval (seconds). Called by the
 /// peer_server when it detects the gap between two keepalives from the
 /// peer.
 pub fn record_keepalive_secs(&self, token: &str, secs: u64) {
 let result = {
 let sessions = self.sessions.lock().unwrap();
 let Some(session) = sessions.get(token) else { return; };
 let mut fp = session.fingerprint.lock().unwrap();
 fp.keepalive_secs = Some(secs);
 let status = session.status.lock().unwrap().clone();
 (fp.clone(), status)
 };
 self.emit(token, result.1, &result.0);
 }

 /// Mark the peer connection as ended (called when the peer_server detects
 /// disconnect or idle timeout). This lets the UI show "not measured"
 /// instead of "measuring..." when the connection was too short to measure
 /// keepalive cadence.
 pub fn mark_connection_ended(&self, token: &str) {
 let result = {
 let sessions = self.sessions.lock().unwrap();
 let Some(session) = sessions.get(token) else { return; };
 let mut fp = session.fingerprint.lock().unwrap();
 // If keepalive wasn't measured, set it to 0 to signal "not measured"
 // (distinct from None = "still measuring").
 if fp.keepalive_secs.is_none() {
 fp.keepalive_secs = Some(0);
 }
 let status = session.status.lock().unwrap().clone();
 (fp.clone(), status)
 };
 self.emit(token, result.1, &result.0);
 }

 /// Get the current status of a capture session (for the UI to poll).
 pub fn get_status(&self, token: &str) -> Option<CaptureStatus> {
 let sessions = self.sessions.lock().unwrap();
 sessions.get(token).map(|s| s.status.lock().unwrap().clone())
 }

 /// Get the captured fingerprint (for the UI to display / build TOML).
 pub fn get_fingerprint(&self, token: &str) -> Option<CapturedFingerprint> {
 let sessions = self.sessions.lock().unwrap();
 sessions.get(token).map(|s| s.fingerprint.lock().unwrap().clone())
 }

 /// Remove a session (cleanup after capture is done or abandoned).
 pub fn remove(&self, token: &str) {
 self.sessions.lock().unwrap().remove(token);
 }
}

/// Build the bencoded tracker announce response for a capture session.
///
/// Returns a compact peers response containing exactly one peer - us -
/// so the client connects to our peer_server for the wire handshake.
///
/// Format: `d8:completei1e10:incompletei1e8:intervali<N>e5:peers6:<ip><port>e`
pub fn build_tracker_response(session: &CaptureSessionRef, interval_secs: u64) -> Vec<u8> {
 let mut top = BTreeMap::new();
 top.insert(protocol::K_COMPLETE.to_vec(), bencode::Value::Int(1));
 top.insert(protocol::K_INCOMPLETE.to_vec(), bencode::Value::Int(1));
 top.insert(protocol::K_INTERVAL.to_vec(), bencode::Value::Int(interval_secs as i64));

 // Compact peers: 4-byte IPv4 (big-endian) + 2-byte port (big-endian)
 let ip_bytes = session.peer_ip.octets();
 let port_bytes = session.peer_port.to_be_bytes();
 let peers_blob: Vec<u8> = ip_bytes.iter().chain(port_bytes.iter()).copied().collect();
 top.insert(protocol::K_PEERS.to_vec(), bencode::Value::Bytes(peers_blob));

 bencode::encode(&bencode::Value::Dict(top))
}

/// Parse the raw query string into ordered (key, value) pairs.
///
/// Unlike `axum::extract::Query<T>`, this preserves the exact parameter order
/// and returns raw URL-encoded values (the caller decodes as needed).
pub fn parse_query_params(raw_query: &str) -> Vec<(String, String)> {
 raw_query
 .split('&')
 .filter_map(|pair| {
 let mut parts = pair.splitn(2, '=');
 let key = parts.next()?;
 if key.is_empty() {
 return None;
 }
 let value = parts.next().unwrap_or("").to_string();
 Some((key.to_string(), value))
 })
 .collect()
}

/// URL-decode a query parameter value into raw bytes (for binary fields like
/// `info_hash` and `peer_id`). Delegates to the shared `data::protocol` helper
/// so percent-decoding has one implementation.
pub fn url_decode(value: &str) -> Vec<u8> {
 crate::data::protocol::percent_decode_raw(value)
}

/// Mint a random 32-char hex token (16 bytes of entropy).
pub fn mint_token() -> String {
 use rand::Rng;
 let bytes: [u8; 16] = rand::rng().random();
 bencode::hex_encode(&bytes)
}

/// Parse an IPv4 address from a `host:port` string.
/// Returns `None` if the host part is not a valid IPv4 address.
pub fn parse_ip_from_host(host: &str) -> Option<Ipv4Addr> {
 // Handle "ip:port" - take everything before the last colon.
 let ip_str = host.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(host);
 ip_str.parse().ok()
}

/// Detect the machine's LAN IPv4 address using the connected-UDP trick: open
/// a UDP socket, `connect()` it to a non-loopback address (the NAT gateway
/// when configured, otherwise a public anycast address), then read
/// `local_addr()`. The OS picks the source IP from the routing table - the
/// machine's LAN IP on the interface that reaches that destination. UDP
/// `connect()` sends no packets, so this works without network connectivity
/// as long as a route exists.
///
/// Returns `None` if detection fails (e.g. no non-loopback route, loopback-only
/// environment). Filters out loopback, link-local, and multicast addresses.
pub fn detect_lan_ipv4(gateway: Option<std::net::IpAddr>) -> Option<Ipv4Addr> {
 // Prefer the NAT gateway (we know it's routable and non-loopback); fall
 // back to a well-known public address so the OS resolves the route.
 let target: std::net::SocketAddr = match gateway {
 Some(std::net::IpAddr::V4(ip)) if !ip.is_loopback() => {
 std::net::SocketAddr::new(std::net::IpAddr::V4(ip), 1)
 }
 _ => std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(1, 1, 1, 1)), 1),
 };
 let sock = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).ok()?;
 sock.connect(target).ok()?;
 match sock.local_addr() {
 Ok(std::net::SocketAddr::V4(s4)) => {
 let ip = *s4.ip();
 if ip.is_loopback() || ip.is_link_local() || ip.is_multicast() || ip.is_unspecified() {
 None
 } else {
 Some(ip)
 }
 }
 _ => None,
 }
}

#[cfg(test)]
mod tests {
 use super::*;

 // peer_id prefix decoding

 #[test]
 fn decode_qbittorrent() {
 let (name, ver) = decode_peer_id_prefix("-qB5220-").unwrap();
 assert_eq!(name, "qBittorrent");
 assert_eq!(ver, "5.2.2");
 }

 #[test]
 fn decode_qbittorrent_521() {
 let (name, ver) = decode_peer_id_prefix("-qB5210-").unwrap();
 assert_eq!(name, "qBittorrent");
 assert_eq!(ver, "5.2.1");
 }

 #[test]
 fn decode_transmission() {
 let (name, ver) = decode_peer_id_prefix("-TR4120-").unwrap();
 assert_eq!(name, "Transmission");
 assert_eq!(ver, "4.1.2");
 }

 #[test]
 fn decode_deluge() {
 let (name, ver) = decode_peer_id_prefix("-DE220s-").unwrap();
 assert_eq!(name, "Deluge");
 assert_eq!(ver, "2.2.0");
 }

 #[test]
 fn decode_utorrent() {
 let (name, ver) = decode_peer_id_prefix("-UT3550-").unwrap();
 assert_eq!(name, "µTorrent");
 assert_eq!(ver, "3.5.5");
 }

 #[test]
 fn decode_utorrent_mac() {
 let (name, ver) = decode_peer_id_prefix("-UM3550-").unwrap();
 assert_eq!(name, "µTorrent");
 assert_eq!(ver, "3.5.5");
 }

 #[test]
 fn decode_utorrent_embedded() {
 let (name, ver) = decode_peer_id_prefix("-UE3550-").unwrap();
 assert_eq!(name, "µTorrent");
 assert_eq!(ver, "3.5.5");
 }

 #[test]
 fn decode_utorrent_web() {
 let (name, ver) = decode_peer_id_prefix("-UW3550-").unwrap();
 assert_eq!(name, "µTorrent");
 assert_eq!(ver, "3.5.5");
 }

 #[test]
 fn decode_bittorrent() {
 let (name, ver) = decode_peer_id_prefix("-BT7B00-").unwrap();
 assert_eq!(name, "BitTorrent");
 assert_eq!(ver, "7.11.0");
 }

 #[test]
 fn decode_bittorrent_web() {
 let (name, ver) = decode_peer_id_prefix("-BW7B00-").unwrap();
 assert_eq!(name, "BitTorrent");
 assert_eq!(ver, "7.11.0");
 }

 #[test]
 fn decode_bittorrent_711() {
 let (name, ver) = decode_peer_id_prefix("-BT7110-").unwrap();
 assert_eq!(name, "BitTorrent");
 assert_eq!(ver, "7.1.1");
 }

 #[test]
 fn decode_rtorrent() {
 let (name, ver) = decode_peer_id_prefix("-lt098-").unwrap();
 assert_eq!(name, "rTorrent");
 assert_eq!(ver, "0.9.8");
 }

 #[test]
 fn decode_vuze() {
 let (name, ver) = decode_peer_id_prefix("-AZ5750-").unwrap();
 assert_eq!(name, "Vuze");
 assert_eq!(ver, "5.7.5.0");
 }

 #[test]
 fn decode_libtorrent_rasterbar() {
 let (name, ver) = decode_peer_id_prefix("-LT210-").unwrap();
 assert_eq!(name, "libtorrent");
 assert_eq!(ver, "2.1.0");
 }

 #[test]
 fn decode_utorrent_hex_lowercase() {
 // Hex: 'a'=10, 'f'=15 - proves hex (not base62, where 'a'=36)
 let (name, ver) = decode_peer_id_prefix("-UTaf0-").unwrap();
 assert_eq!(name, "µTorrent");
 assert_eq!(ver, "10.15.0");
 }

 #[test]
 fn decode_bittorrent_hex_lowercase() {
 // Hex: 'b'=11 - proves hex (not base62, where 'b'=37)
 let (name, ver) = decode_peer_id_prefix("-BT7b00-").unwrap();
 assert_eq!(name, "BitTorrent");
 assert_eq!(ver, "7.11.0");
 }

 #[test]
 fn decode_vuze_base62_letters() {
 // Base62: 'A' = 10, 'a' = 36
 let (name, ver) = decode_peer_id_prefix("-AZaB00-").unwrap();
 assert_eq!(name, "Vuze");
 assert_eq!(ver, "36.11.0.0");
 }

 #[test]
 fn decode_unknown_code_returns_none() {
 assert!(decode_peer_id_prefix("-XX0000-").is_none());
 }

 #[test]
 fn decode_no_dash_returns_none() {
 assert!(decode_peer_id_prefix("qB5220-").is_none());
 }

 #[test]
 fn decode_too_short_returns_none() {
 assert!(decode_peer_id_prefix("-qB").is_none());
 assert!(decode_peer_id_prefix("-qB52").is_none());
 assert!(decode_peer_id_prefix("").is_none());
 }

 #[test]
 fn decode_invalid_base62_char_returns_none() {
 // '!' is not a valid base62 character - base62('!') returns None
 assert!(decode_peer_id_prefix("-qB!220-").is_none());
 assert!(decode_peer_id_prefix("-TR4!20-").is_none());
 assert!(decode_peer_id_prefix("-DE2!0s-").is_none());
 assert!(decode_peer_id_prefix("-lt!98-").is_none());
 assert!(decode_peer_id_prefix("-AZ5!50-").is_none());
 assert!(decode_peer_id_prefix("-LT!10-").is_none());
 }

 #[test]
 fn decode_invalid_hex_char_returns_none() {
 // 'G' is not a valid hex character - hex_digit('G') returns None
 assert!(decode_peer_id_prefix("-UT3G50-").is_none());
 assert!(decode_peer_id_prefix("-BT7G00-").is_none());
 }

 #[test]
 fn decode_short_version_block_returns_none() {
 // Valid code but version block too short for the arm's guard.
 // qB requires b.len() >= 4 (ver_block after prefix[3..]); -qB52- has
 // ver_block = "52-" (3 bytes < 4).
 assert!(decode_peer_id_prefix("-qB52-").is_none());
 assert!(decode_peer_id_prefix("-TR12-").is_none());
 assert!(decode_peer_id_prefix("-DE20-").is_none());
 assert!(decode_peer_id_prefix("-UT35-").is_none());
 assert!(decode_peer_id_prefix("-BT7B-").is_none());
 assert!(decode_peer_id_prefix("-AZ57-").is_none());
 }

 // split_identity_string

 #[test]
 fn split_identity_slash_separator() {
 assert_eq!(split_identity_string("qBittorrent/5.2.2"), ("qBittorrent", "5.2.2"));
 }

 #[test]
 fn split_identity_space_separator() {
 assert_eq!(split_identity_string("Transmission 4.1.2"), ("Transmission", "4.1.2"));
 }

 #[test]
 fn split_identity_strips_trailing_lib_version() {
 assert_eq!(split_identity_string("Deluge/2.2.0 libtorrent/2.0.11.0"), ("Deluge", "2.2.0"));
 }

 #[test]
 fn split_identity_strips_semicolon_suffix() {
 assert_eq!(split_identity_string("Vuze 5.7.5.0;Windows 10;Java 1.8.0_301"), ("Vuze", "5.7.5.0"));
 }

 #[test]
 fn split_identity_non_digit_version_returns_unknown() {
 assert_eq!(split_identity_string("Foo/bar"), ("Foo/bar", "unknown"));
 }

 #[test]
 fn split_identity_no_separator_returns_unknown() {
 assert_eq!(split_identity_string("unknown"), ("unknown", "unknown"));
 }

 #[test]
 fn split_identity_empty_string() {
 assert_eq!(split_identity_string(""), ("", "unknown"));
 }

 #[test]
 fn split_identity_bare_slash() {
 // "/" → sep='/' at 0, label="", after="", version="" → non-digit → ("/", "unknown")
 assert_eq!(split_identity_string("/"), ("/", "unknown"));
 }

 #[test]
 fn split_identity_bare_space() {
 // " " → sep=' ' at 0, label="", after="", version="" → non-digit → (" ", "unknown")
 assert_eq!(split_identity_string(" "), (" ", "unknown"));
 }

 #[test]
 fn split_identity_slash_with_empty_version() {
 assert_eq!(split_identity_string("qBittorrent/"), ("qBittorrent/", "unknown"));
 }

 #[test]
 fn split_identity_slash_priority_over_space() {
 // Both separators present - `/` should win (it's checked first).
 assert_eq!(split_identity_string("Deluge/2.0.0 libtorrent/1.0.0"), ("Deluge", "2.0.0"));
 }

 // token minting

 #[test]
 fn mint_token_is_32_hex_chars() {
 let token = mint_token();
 assert_eq!(token.len(), 32, "token must be 32 hex chars");
 assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
 }

 #[test]
 fn mint_token_is_unique() {
 let t1 = mint_token();
 let t2 = mint_token();
 assert_ne!(t1, t2, "tokens must be unique");
 }

 // IP parsing

 #[test]
 fn parse_ip_from_host_with_port() {
 let ip = parse_ip_from_host("127.0.0.1:3000");
 assert_eq!(ip, Some(Ipv4Addr::new(127, 0, 0, 1)));
 }

 #[test]
 fn parse_ip_from_host_without_port() {
 let ip = parse_ip_from_host("192.168.1.100");
 assert_eq!(ip, Some(Ipv4Addr::new(192, 168, 1, 100)));
 }

 #[test]
 fn parse_ip_from_host_invalid_returns_none() {
 assert!(parse_ip_from_host("not-an-ip:3000").is_none());
 assert!(parse_ip_from_host("example.com:3000").is_none());
 }

 // query param parsing

 #[test]
 fn parse_query_params_preserves_order() {
 let params = parse_query_params("info_hash=abc&peer_id=def&port=12345");
 assert_eq!(params.len(), 3);
 assert_eq!(params[0], ("info_hash".to_string(), "abc".to_string()));
 assert_eq!(params[1], ("peer_id".to_string(), "def".to_string()));
 assert_eq!(params[2], ("port".to_string(), "12345".to_string()));
 }

 #[test]
 fn parse_query_params_handles_empty_value() {
 let params = parse_query_params("key=&next=val");
 assert_eq!(params[0], ("key".to_string(), "".to_string()));
 assert_eq!(params[1], ("next".to_string(), "val".to_string()));
 }

 #[test]
 fn parse_query_params_handles_value_with_equals() {
 // The value can contain '=' (e.g. base64 data) - only split on first '='
 let params = parse_query_params("key=val=ue");
 assert_eq!(params[0], ("key".to_string(), "val=ue".to_string()));
 }

 #[test]
 fn parse_query_params_empty_string() {
 let params = parse_query_params("");
 assert!(params.is_empty());
 }

 #[test]
 fn url_decode_plain_ascii() {
 assert_eq!(url_decode("hello"), b"hello");
 }

 #[test]
 fn url_decode_percent_encoded() {
 assert_eq!(url_decode("%20"), b" ");
 assert_eq!(url_decode("%41%42%43"), b"ABC");
 }

 #[test]
 fn url_decode_binary_info_hash() {
 // A typical info_hash percent-encoded value
 let decoded = url_decode("%AB%CD%EF%01%23%45%67%89%AB%CD%EF%01%23%45%67%89%AB%CD%EF%01");
 assert_eq!(decoded.len(), protocol::INFO_HASH_LEN);
 assert_eq!(decoded[0], 0xAB);
 assert_eq!(decoded[19], 0x01);
 }

 // tracker response

 #[test]
 fn build_tracker_response_contains_one_peer() {
 let session = CaptureSessionRef {
 peer_ip: Ipv4Addr::new(127, 0, 0, 1),
 peer_port: 6881,
 };
 let response = build_tracker_response(&session, 1800);

 // Must be valid bencode
 let dict = bencode::decode(&response).unwrap();
 assert_eq!(dict.get(b"interval").unwrap().as_int(), Some(1800));
 assert_eq!(dict.get(b"complete").unwrap().as_int(), Some(1));
 assert_eq!(dict.get(b"incomplete").unwrap().as_int(), Some(1));

 // peers blob: 6 bytes (4 IP + 2 port)
 let peers = dict.get(b"peers").unwrap();
 let peers_bytes = peers.as_bytes().unwrap();
 assert_eq!(peers_bytes.len(), protocol::COMPACT_IPV4_PEER_LEN);
 assert_eq!(&peers_bytes[..4], &[127, 0, 0, 1]);
 assert_eq!(&peers_bytes[4..], &[0x1A, 0xE1]); // 6881 in big-endian
 }

 #[test]
 fn build_tracker_response_decodes_back() {
 let session = CaptureSessionRef {
 peer_ip: Ipv4Addr::new(192, 168, 1, 100),
 peer_port: 6881,
 };
 let response = build_tracker_response(&session, 3600);
 assert!(bencode::decode(&response).is_ok());
 }

 // CaptureStore

 #[test]
 fn store_start_creates_session_and_torrent() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, torrent_bytes) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test-capture", [0; protocol::PEER_ID_LEN]);
 assert!(!token.is_empty());
 assert!(!torrent_bytes.is_empty());
 // Must be a valid .torrent
 assert!(crate::torrent::parse(&torrent_bytes).is_ok());
 // Session must be findable
 assert!(store.get_by_token(&token).is_some());
 }

 #[test]
 fn store_get_by_token_unknown_returns_none() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 assert!(store.get_by_token("nonexistent").is_none());
 }

 #[test]
 fn store_find_by_info_hash_finds_session() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, torrent_bytes) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 let meta = crate::torrent::parse(&torrent_bytes).unwrap();
 let found_token = store.find_by_info_hash(&meta.info_hash).unwrap();
 assert_eq!(found_token, token);
 }

 #[test]
 fn store_find_by_info_hash_unknown_returns_none() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let unknown_hash = [0xFF; protocol::INFO_HASH_LEN];
 assert!(store.find_by_info_hash(&unknown_hash).is_none());
 }

 #[test]
 fn store_record_announce_updates_fingerprint_and_status() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);

 let peer_id = [0xBB; protocol::PEER_ID_LEN];
 store.record_announce(
 &token,
 AnnounceData {
 peer_id,
 user_agent: "qBittorrent/5.2.2".to_string(),
 query_param_order: vec!["info_hash".into(), "peer_id".into(), "port".into()],
 raw_query: "info_hash=abc&peer_id=def&port=12345".to_string(),
 numwant: Some(200),
 http_headers: vec![("host".into(), "127.0.0.1:3000".into())],
 },
 );

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.peer_id, Some(peer_id));
 assert_eq!(fp.user_agent.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.query_param_order.as_deref(), Some(&["info_hash".to_string(), "peer_id".to_string(), "port".to_string()][..]));
 assert_eq!(fp.numwant, Some(200));

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::AnnounceCaptured);
 }

 #[test]
 fn store_record_announce_unknown_token_is_silent_noop() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 store.record_announce(
 "nonexistent",
 AnnounceData {
 peer_id: [0; protocol::PEER_ID_LEN],
 user_agent: "test".into(),
 query_param_order: vec![],
 raw_query: "".to_string(),
 numwant: None,
 http_headers: vec![],
 },
 );
 // Must not panic
 }

 #[test]
 fn store_record_handshake_advances_status() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::HandshakeCaptured);
 let fp = store.get_fingerprint(&token).unwrap();
 assert!(fp.reserved_bytes.is_some());
 assert!(fp.handshake_peer_id.is_some());
 }

 #[test]
 fn store_record_ext_handshake_advances_to_complete() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);

 let mut m = BTreeMap::new();
 m.insert("ut_pex".into(), 1u32);
 m.insert("ut_metadata".into(), 2u32);
 store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("qBittorrent/5.2.2".into()), m_dict: Some(m), reqq: Some(2000), encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() });

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::ExtHandshakeCaptured);
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.v_string.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.reqq, Some(2000));
 assert!(fp.m_dict.is_some());
 }

 #[test]
 fn store_record_ext_handshake_partial_m_only() {
 // BEP-10: all dict items are optional. A client may send only `m`
 // without `v` or `reqq` (e.g. anonymous mode omits `v`).
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);

 let mut m = BTreeMap::new();
 m.insert("ut_pex".into(), 1u32);
 store.record_ext_handshake(&token, ExtHandshakeData { v_string: None, m_dict: Some(m), reqq: None, encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() });

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::ExtHandshakeCaptured);
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.v_string, None, "v_string should be None when client omits it");
 assert_eq!(fp.reqq, None, "reqq should be None when client omits it");
 assert!(fp.m_dict.is_some(), "m_dict should be recorded");
 }

 #[test]
 fn store_record_ext_handshake_v_only() {
 // Client sends only `v` without `m` or `reqq` - unusual but valid.
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);

 store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("qBittorrent/5.2.2".into()), m_dict: None, reqq: None, encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() });

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::ExtHandshakeCaptured);
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.v_string.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.m_dict, None);
 assert_eq!(fp.reqq, None);
 }

 #[test]
 fn store_record_ext_handshake_all_none_still_advances() {
 // Even an empty ext handshake (decoded dict with no recognized keys)
 // should advance the status - the client DID send the message.
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);

 store.record_ext_handshake(&token, ExtHandshakeData { v_string: None, m_dict: None, reqq: None, encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() });

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::ExtHandshakeCaptured);
 }

 #[test]
 fn store_remove_deletes_session() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 assert!(store.get_by_token(&token).is_some());
 store.remove(&token);
 assert!(store.get_by_token(&token).is_none());
 }

 // Lock: first client wins, subsequent are rejected

 #[test]
 fn record_announce_locks_to_first_client() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);

 let first = store.record_announce(
 &token,
 AnnounceData {
 peer_id: [0xAA; protocol::PEER_ID_LEN],
 user_agent: "qBittorrent/5.2.2".into(),
 query_param_order: vec!["info_hash".into()],
 raw_query: "info_hash=abc".into(),
 numwant: Some(200),
 http_headers: vec![],
 },
 );
 assert!(first, "first announce should be recorded");

 let second = store.record_announce(
 &token,
 AnnounceData {
 peer_id: [0xBB; protocol::PEER_ID_LEN],
 user_agent: "Transmission/4.1.2".into(),
 query_param_order: vec!["info_hash".into()],
 raw_query: "info_hash=def".into(),
 numwant: Some(80),
 http_headers: vec![],
 },
 );
 assert!(!second, "second announce should be rejected");

 // Fingerprint should be from the first client only
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.peer_id, Some([0xAA; protocol::PEER_ID_LEN]));
 assert_eq!(fp.user_agent.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.numwant, Some(200));
 }

 #[test]
 fn record_handshake_locks_to_first_client() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });

 let first = store.record_handshake(&token, [0xAA; protocol::RESERVED_LEN], [0xAA; protocol::PEER_ID_LEN]);
 assert!(first, "first handshake should be recorded");

 let second = store.record_handshake(&token, [0xBB; protocol::RESERVED_LEN], [0xBB; protocol::PEER_ID_LEN]);
 assert!(!second, "second handshake should be rejected");

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.reserved_bytes, Some([0xAA; protocol::RESERVED_LEN]));
 assert_eq!(fp.handshake_peer_id, Some([0xAA; protocol::PEER_ID_LEN]));
 }

 #[test]
 fn record_ext_handshake_locks_to_first_client() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);

 let mut m1 = BTreeMap::new();
 m1.insert("ut_pex".into(), 1u32);
 let first = store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("qBittorrent/5.2.2".into()), m_dict: Some(m1), reqq: Some(2000), encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() });
 assert!(first, "first ext handshake should be recorded");

 let mut m2 = BTreeMap::new();
 m2.insert("ut_metadata".into(), 3u32);
 let second = store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("Transmission 4.1.2".into()), m_dict: Some(m2), reqq: Some(500), encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() });
 assert!(!second, "second ext handshake should be rejected");

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.v_string.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.reqq, Some(2000));
 }

 #[test]
 fn full_capture_flow_two_clients_second_rejected() {
 // Simulate two clients loading the same torrent:
 // client A announces first → recorded
 // client B announces second → rejected
 // client A handshakes → recorded
 // client B handshakes → rejected
 // client A ext handshakes → recorded
 // client B ext handshakes → rejected
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);

 // Client A announces
 assert!(store.record_announce(&token, AnnounceData { peer_id: [0xAA; protocol::PEER_ID_LEN], user_agent: "qBittorrent/5.2.2".into(), query_param_order: vec![], raw_query: "".into(), numwant: Some(200), http_headers: vec![] }));
 // Client B announces (same torrent, different client)
 assert!(!store.record_announce(&token, AnnounceData { peer_id: [0xBB; protocol::PEER_ID_LEN], user_agent: "Transmission/4.1.2".into(), query_param_order: vec![], raw_query: "".into(), numwant: Some(80), http_headers: vec![] }));

 // Client A handshakes
 assert!(store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0xAA; protocol::PEER_ID_LEN]));
 // Client B handshakes
 assert!(!store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0xBB; protocol::PEER_ID_LEN]));

 // Client A ext handshakes
 assert!(store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("qBittorrent/5.2.2".into()), m_dict: Some(BTreeMap::new()), reqq: Some(2000), encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() }));
 // Client B ext handshakes
 assert!(!store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("Transmission 4.1.2".into()), m_dict: Some(BTreeMap::new()), reqq: Some(500), encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() }));

 // Fingerprint should be 100% from client A
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.peer_id, Some([0xAA; protocol::PEER_ID_LEN]));
 assert_eq!(fp.user_agent.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.numwant, Some(200));
 assert_eq!(fp.handshake_peer_id, Some([0xAA; protocol::PEER_ID_LEN]));
 assert_eq!(fp.v_string.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.reqq, Some(2000));
 }

 #[test]
 fn store_remove_unknown_is_silent_noop() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 store.remove("nonexistent");
 // Must not panic
 }

 #[test]
 fn store_start_different_tokens_different_info_hashes() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (t1, torrent1) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "name-a", [0; protocol::PEER_ID_LEN]);
 let (t2, torrent2) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "name-b", [0; protocol::PEER_ID_LEN]);
 let meta1 = crate::torrent::parse(&torrent1).unwrap();
 let meta2 = crate::torrent::parse(&torrent2).unwrap();
 assert_ne!(t1, t2, "tokens must differ");
 assert_ne!(meta1.info_hash, meta2.info_hash, "info hashes must differ (different names)");
 }

 // Full capture flow integration

 #[test]
 fn full_capture_flow_records_all_phases() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);

 // Phase 0: Start a capture session
 let (token, torrent_bytes) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "capture-test", [0; protocol::PEER_ID_LEN]);
 let meta = crate::torrent::parse(&torrent_bytes).unwrap();

 // Verify session is waiting
 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::WaitingForAnnounce);

 // Verify the torrent's announce URL points to our tracker
 assert!(meta.announce_url.contains("/capture/"));
 assert!(meta.announce_url.contains(&token));

 // Phase 1: Simulate tracker announce from a real client
 let peer_id = [0xBB; protocol::PEER_ID_LEN];
 let user_agent = "qBittorrent/5.2.2".to_string();
 let query_order = vec![
 "info_hash".to_string(), "peer_id".to_string(), "port".to_string(),
 "uploaded".to_string(), "downloaded".to_string(), "left".to_string(),
 "corrupt".to_string(), "key".to_string(), "numwant".to_string(),
 "compact".to_string(), "no_peer_id".to_string(),
 ];
 let raw_query = "info_hash=%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB&peer_id=%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB%BB&port=12345&uploaded=0&downloaded=0&left=256&corrupt=0&key=ABCDEF12&numwant=200&compact=1&no_peer_id=1".to_string();

 store.record_announce(&token, AnnounceData { peer_id, user_agent: user_agent.clone(), query_param_order: query_order.clone(), raw_query: raw_query.clone(), numwant: Some(200), http_headers: vec![("host".into(), "10.145.10.225:3000".into()), ("accept-encoding".into(), "gzip".into())] });

 // Verify announce phase recorded correctly
 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::AnnounceCaptured);
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.peer_id, Some(peer_id));
 assert_eq!(fp.user_agent.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.query_param_order.as_deref(), Some(&query_order[..]));
 assert_eq!(fp.numwant, Some(200));
 assert_eq!(fp.raw_query.as_deref(), Some(raw_query.as_str()));

 // Phase 2: Simulate peer-wire handshake
 let reserved_bytes = [0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x05];
 let handshake_peer_id = peer_id; // Same as announce - cross-check passes

 store.record_handshake(&token, reserved_bytes, handshake_peer_id);

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::HandshakeCaptured);
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.reserved_bytes, Some(reserved_bytes));
 assert_eq!(fp.handshake_peer_id, Some(handshake_peer_id));

 // Phase 3: Simulate BEP-10 extension handshake
 let v_string = "qBittorrent/5.2.2".to_string();
 let mut m_dict = BTreeMap::new();
 m_dict.insert("ut_pex".to_string(), 1);
 m_dict.insert("ut_metadata".to_string(), 2);
 m_dict.insert("upload_only".to_string(), 3);
 m_dict.insert("lt_donthave".to_string(), 4);
 let reqq = 2000u32;

 store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some(v_string.clone()), m_dict: Some(m_dict.clone()), reqq: Some(reqq), encryption_preferred: Some(true), upload_only: Some(false), complete_ago: Some(3600), ..Default::default() });

 assert_eq!(store.get_status(&token).unwrap(), CaptureStatus::ExtHandshakeCaptured);

 // Phase 4: Verify the complete fingerprint
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.peer_id, Some(peer_id));
 assert_eq!(fp.user_agent.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.numwant, Some(200));
 assert_eq!(fp.reserved_bytes, Some(reserved_bytes));
 assert_eq!(fp.handshake_peer_id, Some(handshake_peer_id));
 assert_eq!(fp.v_string.as_deref(), Some("qBittorrent/5.2.2"));
 assert_eq!(fp.reqq, Some(2000));
 assert_eq!(fp.m_dict.unwrap(), m_dict);
 assert_eq!(fp.encryption_preferred, Some(true));
 assert_eq!(fp.upload_only, Some(false));
 assert_eq!(fp.complete_ago, Some(3600));
 assert!(fp.http_headers.is_some());
 assert_eq!(fp.http_headers.as_ref().unwrap().len(), 2);
 }

 #[test]
 fn full_capture_flow_peer_id_mismatch_still_recorded() {
 // If the handshake peer_id doesn't match the announce peer_id,
 // we still record both - this is a spoofing signal the user should see.
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);

 let announce_peer_id = [0xBB; protocol::PEER_ID_LEN];
 let handshake_peer_id = [0xCC; protocol::PEER_ID_LEN]; // Different!

 store.record_announce(&token, AnnounceData { peer_id: announce_peer_id, user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], handshake_peer_id);

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.peer_id, Some(announce_peer_id));
 assert_eq!(fp.handshake_peer_id, Some(handshake_peer_id));
 assert_ne!(fp.peer_id, fp.handshake_peer_id, "mismatch is preserved for the user to see");
 }

 #[test]
 fn capture_session_removed_after_cleanup() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 assert!(store.get_by_token(&token).is_some());
 assert!(store.get_status(&token).is_some());

 store.remove(&token);

 assert!(store.get_by_token(&token).is_none());
 assert!(store.get_status(&token).is_none());
 assert!(store.get_fingerprint(&token).is_none());
 }

 // New capture signals

 #[test]
 fn record_peer_message_tracks_order() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_peer_message(&token, "ext_handshake");
 store.record_peer_message(&token, "interested");
 store.record_peer_message(&token, "have_none");
 store.record_peer_message(&token, "keepalive");

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(
 fp.message_order.as_deref(),
 Some(&["ext_handshake".to_string(), "interested".to_string(), "have_none".to_string(), "keepalive".to_string()][..])
 );
 }

 #[test]
 fn record_peer_message_caps_at_max() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 for i in 0..MAX_CAPTURE_MESSAGES + 5 {
 store.record_peer_message(&token, &format!("msg{i}"));
 }
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.message_order.as_ref().unwrap().len(), MAX_CAPTURE_MESSAGES);
 }

 #[test]
 fn record_peer_message_unknown_token_is_silent_noop() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 store.record_peer_message("nonexistent", "keepalive");
 // Must not panic
 }

 #[test]
 fn record_keepalive_secs_stores_value() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_keepalive_secs(&token, 120);
 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.keepalive_secs, Some(120));
 }

 #[test]
 fn record_keepalive_secs_unknown_token_is_silent_noop() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 store.record_keepalive_secs("nonexistent", 120);
 // Must not panic
 }

 #[test]
 fn record_announce_stores_http_headers() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 let headers = vec![
 ("host".into(), "10.0.0.1:3000".into()),
 ("user-agent".into(), "qBittorrent/5.2.2".into()),
 ("accept-encoding".into(), "gzip, deflate".into()),
 ];
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: headers.clone() });

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.http_headers.as_deref(), Some(headers.as_slice()));
 }

 #[test]
 fn record_ext_handshake_stores_encryption_upload_only_complete_ago() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);
 store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("libtorrent/2.0.9".into()), m_dict: Some(BTreeMap::new()), reqq: Some(2000), encryption_preferred: Some(true), upload_only: Some(true), complete_ago: Some(7200), ..Default::default() });

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.encryption_preferred, Some(true));
 assert_eq!(fp.upload_only, Some(true));
 assert_eq!(fp.complete_ago, Some(7200));
 }

 #[test]
 fn record_ext_handshake_optional_fields_omitted() {
 // Client omits e, upload_only, complete_ago - should be None
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);
 store.record_ext_handshake(&token, ExtHandshakeData { v_string: Some("Minimal/1.0".into()), m_dict: Some(BTreeMap::new()), reqq: Some(500), encryption_preferred: None, upload_only: None, complete_ago: None, ..Default::default() });

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.encryption_preferred, None);
 assert_eq!(fp.upload_only, None);
 assert_eq!(fp.complete_ago, None);
 }

 #[test]
 fn record_ext_handshake_stores_new_bep10_fields() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);
 store.record_ext_handshake(&token, ExtHandshakeData {
 v_string: Some("qBittorrent/5.2.2".into()),
 m_dict: Some(BTreeMap::new()),
 reqq: Some(2000),
 encryption_preferred: Some(true),
 upload_only: Some(true),
 complete_ago: Some(-1),
 yourip: Some(vec![127, 0, 0, 1]),
 listen_port: Some(6881),
 metadata_size: Some(16384),
 ipv4: Some(vec![10, 0, 0, 1]),
 ipv6: Some(vec![0; 16]),
 share_mode: Some(false),
 });

 let fp = store.get_fingerprint(&token).unwrap();
 assert_eq!(fp.yourip.as_deref(), Some(&[127, 0, 0, 1][..]));
 assert_eq!(fp.listen_port, Some(6881));
 assert_eq!(fp.metadata_size, Some(16384));
 assert_eq!(fp.ipv4.as_deref(), Some(&[10, 0, 0, 1][..]));
 assert_eq!(fp.ipv6.as_deref(), Some(&[0; 16][..]));
 assert_eq!(fp.share_mode, Some(false));
 }

 #[test]
 fn capture_view_includes_new_bep10_fields() {
 let store = CaptureStore::new(tokio::sync::broadcast::channel(1).0);
 let (token, _) = store.start("3000", Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(127, 0, 0, 1), 6881, "test", [0; protocol::PEER_ID_LEN]);
 store.record_announce(&token, AnnounceData { peer_id: [0; protocol::PEER_ID_LEN], user_agent: "UA".into(), query_param_order: vec![], raw_query: "".into(), numwant: None, http_headers: vec![] });
 store.record_handshake(&token, [0; protocol::RESERVED_LEN], [0; protocol::PEER_ID_LEN]);
 store.record_ext_handshake(&token, ExtHandshakeData {
 v_string: Some("qBittorrent/5.2.2".into()),
 m_dict: Some(BTreeMap::new()),
 reqq: Some(2000),
 yourip: Some(vec![127, 0, 0, 1]),
 listen_port: Some(6881),
 metadata_size: Some(16384),
 ipv4: Some(vec![10, 0, 0, 1]),
 ipv6: Some(vec![0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01]),
 share_mode: Some(true),
 ..Default::default()
 });

 let fp = store.get_fingerprint(&token).unwrap();
 let view = fp.to_view();
 assert_eq!(view.yourip.as_deref(), Some("127.0.0.1"));
 assert_eq!(view.listen_port, Some(6881));
 assert_eq!(view.metadata_size, Some(16384));
 assert_eq!(view.ipv4.as_deref(), Some("10.0.0.1"));
 assert_eq!(view.ipv6.as_deref(), Some("2001:db8::1"));
 assert_eq!(view.share_mode, Some(true));
 }
}
