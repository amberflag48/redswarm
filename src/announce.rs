//! Announce client - constructs and sends tracker announce requests, parses responses.
//!
//! This is the shared engine every attack technique uses. It handles URL-encoding
//! of binary fields, client-specific query templates, and bencode response parsing.

use crate::bencode;
use crate::config::ClientSpecConfig;
use crate::data::protocol;
use crate::data::vocab;

/// Announce event type. Maps to the `event` query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
 Started,
 Completed,
 Stopped,
 None,
}

impl Event {
 fn query_fragment(self) -> String {
 match self {
 Event::Started => format!("&event={}", vocab::EVENT_STARTED),
 Event::Completed => format!("&event={}", vocab::EVENT_COMPLETED),
 Event::Stopped => format!("&event={}", vocab::EVENT_STOPPED),
 Event::None => String::new(),
 }
 }
}

/// Peer state reported to the tracker. All values are cumulative since `started`.
#[derive(Debug, Clone, Copy)]
pub struct PeerState {
 pub uploaded: u64,
 pub downloaded: u64,
 pub left: u64,
}

/// Parsed announce response from the tracker.
#[derive(Debug, Clone)]
pub struct AnnounceResponse {
 pub interval: u32,
 /// Tracker-advertised minimum re-announce interval (BEP-3 `min interval`).
 /// `None` if the tracker didn't include it. The engine should use
 /// `max(config_min, tracker_min)` to avoid re-announcing too fast.
 pub min_interval: Option<u32>,
 pub seeders: i64,
 pub leechers: i64,
 pub peer_count: usize,
 pub failure_reason: Option<String>,
}

impl AnnounceResponse {
 pub fn is_failure(&self) -> bool {
 self.failure_reason.is_some()
 }

 /// The effective announce interval, respecting the tracker's `min interval`
 /// (BEP-3). If the tracker advertised a `min interval` higher than `interval`,
 /// use it to avoid re-announcing too fast and getting banned.
 pub fn effective_interval(&self) -> u32 {
 match self.min_interval {
 Some(tracker_min) if tracker_min > self.interval => tracker_min,
 _ => self.interval,
 }
 }
}

/// Percent-encode a raw byte slice - delegates to [`crate::data::protocol::percent_encode_raw`].
fn percent_encode_raw(bytes: &[u8]) -> String {
 protocol::percent_encode_raw(bytes)
}

/// Tracker announce-interval bounds passed into [`AnnounceSession::new`].
/// Grouped so the constructor stays under clippy's argument-count limit and
/// the three values can't drift apart at a call site.
pub struct IntervalBounds {
 pub min_secs: u32,
 pub max_secs: u32,
 pub default_secs: u32,
}

/// The immutable peer identity for one announce session: the `peer_id`
/// advertised to the tracker and the per-session `key`. Persisted across
/// stop/start so the tracker sees the **same** peer on resume - if a new
/// random peer_id were generated on every restart, the tracker would treat
/// the resumed cumulative counters as a brand-new peer's baseline (delta = 0)
/// and the un-announced upload would never be credited.
#[derive(Debug, Clone)]
pub struct PeerIdentity {
 pub peer_id: [u8; protocol::PEER_ID_LEN],
 pub key: String,
}

/// All parameters needed to build an announce request for one peer session.
pub struct AnnounceSession {
 pub announce_url: String,
 pub info_hash: [u8; protocol::INFO_HASH_LEN],
 pub peer_id: [u8; protocol::PEER_ID_LEN],
 pub key: String,
 pub port: u16,
 query_template: String,
 numwant: u32,
 min_interval_secs: u32,
 max_interval_secs: u32,
 default_interval_secs: u32,
 http: reqwest::Client,
}

impl AnnounceSession {
 pub fn new(
 announce_url: &str,
 info_hash: [u8; protocol::INFO_HASH_LEN],
 client: &ClientSpecConfig,
 port: u16,
 http_timeout_secs: u64,
 bounds: IntervalBounds,
 identity: PeerIdentity,
 ) -> Self {
 let peer_id = identity.peer_id;
 let key = identity.key;
 let http = reqwest::Client::builder()
 .user_agent(&client.user_agent)
 .timeout(std::time::Duration::from_secs(http_timeout_secs))
 .default_headers(
 reqwest::header::HeaderMap::from_iter([
 (
 reqwest::header::HeaderName::from_static(protocol::HTTP_CONNECTION_CLOSE.0),
 reqwest::header::HeaderValue::from_static(protocol::HTTP_CONNECTION_CLOSE.1),
 ),
 ]),
 )
 .build()
 .expect("reqwest client build");
 Self {
 announce_url: announce_url.to_string(),
 info_hash,
 peer_id,
 key,
 port,
 query_template: client.query.clone(),
 numwant: client.numwant,
 min_interval_secs: bounds.min_secs,
 max_interval_secs: bounds.max_secs,
 default_interval_secs: bounds.default_secs,
 http,
 }
 }

 /// Build the full announce URL with query string for the given state + event.
 fn build_url(&self, state: PeerState, event: Event) -> String {
 let info_hash_enc = percent_encode_raw(&self.info_hash);
 let peer_id_enc = percent_encode_raw(&self.peer_id);
 let event_str = event.query_fragment();

 let query = self
 .query_template
 .replace(protocol::Q_INFO_HASH, &info_hash_enc)
 .replace(protocol::Q_PEER_ID, &peer_id_enc)
 .replace(protocol::Q_PORT, &self.port.to_string())
 .replace(protocol::Q_UPLOADED, &state.uploaded.to_string())
 .replace(protocol::Q_DOWNLOADED, &state.downloaded.to_string())
 .replace(protocol::Q_LEFT, &state.left.to_string())
 .replace(protocol::Q_KEY, &self.key)
 .replace(protocol::Q_EVENT, &event_str)
 // Real clients (libtorrent, Transmission, uTorrent) send numwant=0
 // on stopped events - they don't want any more peers when leaving.
 .replace(protocol::Q_NUMWANT, &(if event == Event::Stopped { 0 } else { self.numwant }).to_string());

 let sep = if self.announce_url.contains('?') {
 '&'
 } else {
 '?'
 };
 format!("{}{}{}", self.announce_url, sep, query)
 }

 /// Send an announce request and parse the response.
 pub async fn announce(
 &self,
 state: PeerState,
 event: Event,
 ) -> Result<AnnounceResponse, anyhow::Error> {
 let url = self.build_url(state, event);
 tracing::debug!(%url, "sending announce");
 let resp = self.http.get(&url).send().await.map_err(|e| {
 let cause = if e.is_connect() {
 "connection refused/failed"
 } else if e.is_timeout() {
 "timeout"
 } else {
 "network error"
 };
 anyhow::anyhow!("{cause}: {e}")
 })?;
 let status = resp.status();
 let body = resp.bytes().await?;
 tracing::debug!(%status, len = body.len(), "announce response");

 if !status.is_success() {
 // Some trackers return plain-text errors with non-200 status
 let msg = String::from_utf8_lossy(&body).to_string();
 return Ok(AnnounceResponse {
 interval: self.default_interval_secs,
 min_interval: None,
 seeders: 0,
 leechers: 0,
 peer_count: 0,
 failure_reason: Some(format!("HTTP {}: {}", status, msg)),
 });
 }

 parse_response(&body, self.min_interval_secs, self.max_interval_secs, self.default_interval_secs)
 }
}

/// Parse a bencoded tracker announce response.
fn parse_response(data: &[u8], min_interval: u32, max_interval: u32, default_interval: u32) -> Result<AnnounceResponse, anyhow::Error> {
 let root = bencode::decode_checked(data).map_err(|e| anyhow::anyhow!("{e}"))?;

 // A valid announce response must be a dict. A bare integer/list is malformed.
 if !matches!(root, bencode::Value::Dict(_)) {
 anyhow::bail!("announce response is not a bencoded dict");
 }

 let failure_reason = root.get(protocol::K_FAILURE_REASON).and_then(|v| v.as_str()).map(String::from);

 let interval = root
 .get(protocol::K_INTERVAL)
 .and_then(|v| v.as_int())
 .unwrap_or(default_interval as i64)
 .clamp(min_interval as i64, max_interval as i64) as u32;
 let tracker_min_interval = root
 .get(protocol::K_MIN_INTERVAL)
 .and_then(|v| v.as_int())
 .filter(|&v| v > 0)
 .map(|v| v as u32);
 let seeders = root.get(protocol::K_COMPLETE).and_then(|v| v.as_int()).unwrap_or(0);
 let leechers = root
 .get(protocol::K_INCOMPLETE)
 .and_then(|v| v.as_int())
 .unwrap_or(0);

 let peer_count = parse_peers(&root);

 Ok(AnnounceResponse {
 interval,
 min_interval: tracker_min_interval,
 seeders,
 leechers,
 peer_count,
 failure_reason,
 })
}

/// Count peers from either compact (binary 6-byte records) or dictionary model.
/// Also counts IPv6 peers from the `peers6` key (BEP-7, 18-byte records).
fn parse_peers(root: &bencode::Value) -> usize {
 let mut count = 0;
 if let Some(peers_val) = root.get(protocol::K_PEERS) {
 match peers_val {
 bencode::Value::Bytes(raw) => count += raw.len() / protocol::COMPACT_IPV4_PEER_LEN,
 bencode::Value::List(list) => count += list
 .iter()
 .filter(|d| d.get(protocol::K_IP).and_then(|v| v.as_str()).is_some()
 && d.get(protocol::K_PORT).and_then(|v| v.as_int()).is_some())
 .count(),
 _ => {}
 }
 }
 if let Some(bencode::Value::Bytes(raw6)) = root.get(protocol::K_PEERS6) {
 count += raw6.len() / protocol::COMPACT_IPV6_PEER_LEN;
 }
 count
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::config::KeyFormat;

 #[test]
 fn percent_encode_leaves_unreserved_bare() {
 let encoded = percent_encode_raw(b"AZaz09-._~");
 assert_eq!(encoded, "AZaz09-._~");
 }

 #[test]
 fn percent_encode_encodes_special_bytes() {
 let encoded = percent_encode_raw(&[0x12, 0xFF, 0x00, b' ', b'/']);
 assert_eq!(encoded, "%12%FF%00%20%2F");
 }

 #[test]
 fn parse_compact_response() {
 // d8:completei5e10:incompletei10e8:intervali1800e5:peers12:<12 bytes>e
 let peers_raw: &[u8] = &[
 0x7f, 0x00, 0x00, 0x01, 0x1a, 0xe1, // 127.0.0.1:6881
 0x01, 0x02, 0x03, 0x04, 0x1a, 0xe1, // 1.2.3.4:6881
 ];
 let mut buf = b"d8:completei5e10:incompletei10e8:intervali1800e5:peers12:".to_vec();
 buf.extend_from_slice(peers_raw);
 buf.push(b'e');

 let resp = parse_response(&buf, 60, 86400, 1800).unwrap();
 assert_eq!(resp.seeders, 5);
 assert_eq!(resp.leechers, 10);
 assert_eq!(resp.interval, 1800);
 assert_eq!(resp.peer_count, 2);
 }

 #[test]
 fn parse_failure_response() {
 let body = b"d14:failure reason24:Invalid passkey suppliede";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert!(resp.is_failure());
 assert_eq!(resp.failure_reason.as_deref(), Some("Invalid passkey supplied"));
 }

 // Malformed response failures

 #[test]
 fn parse_empty_response() {
 assert!(parse_response(b"", 60, 86400, 1800).is_err());
 }

 #[test]
 fn parse_garbage() {
 assert!(parse_response(b"not bencoded at all", 60, 86400, 1800).is_err());
 assert!(parse_response(b"\x00\x01\x02\x03", 60, 86400, 1800).is_err());
 }

 #[test]
 fn parse_not_a_dict() {
 assert!(parse_response(b"i42e", 60, 86400, 1800).is_err());
 assert!(parse_response(b"l4:teste", 60, 86400, 1800).is_err());
 }

 #[test]
 fn parse_truncated_bencode() {
 assert!(parse_response(b"d8:intervali1800", 60, 86400, 1800).is_err()); // no closing 'e'
 }

 #[test]
 fn parse_dict_model_peers() {
 let body = b"d8:completei5e10:incompletei10e8:intervali1800e5:peersld2:ip9:127.0.0.14:porti6881eed2:ip7:1.2.3.44:porti6881eeee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.peer_count, 2);
 }

 #[test]
 fn parse_no_peers_key() {
 let body = b"d8:completei5e10:incompletei10e8:intervali1800ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.peer_count, 0);
 }

 #[test]
 fn parse_empty_peers() {
 let body = b"d8:completei5e10:incompletei10e8:intervali1800e5:peers0:e";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.peer_count, 0);
 }

 #[test]
 fn parse_malformed_compact_peers() {
 // 5 bytes - not a multiple of 6, chunks_exact skips it
 let body = b"d8:intervali1800e5:peers5:\x01\x02\x03\x04\x05e";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.peer_count, 0);
 }

 #[test]
 fn parse_missing_interval() {
 let body = b"d8:completei5e10:incompletei10ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 // defaults to 1800
 assert_eq!(resp.interval, 1800);
 }

 #[test]
 fn parse_response_negative_interval_clamped() {
 let body = b"d8:intervali-1ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.interval, 60);
 }

 #[test]
 fn parse_response_huge_interval_clamped() {
 let body = b"d8:intervali9223372036854775807ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.interval, 86400);
 }

 #[test]
 fn parse_response_zero_interval_clamped() {
 let body = b"d8:intervali0ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.interval, 60);
 }

 #[test]
 fn parse_warning_message() {
 let body = b"d15:warning message4:test8:intervali1800ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 // not a failure, just a warning
 assert!(!resp.is_failure());
 }

 // URL building

 fn test_client() -> ClientSpecConfig {
 ClientSpecConfig {
 label: "qBittorrent".into(),
 version: "5.2.2".into(),
 peer_id_prefix: "-qB5220-".into(),
 user_agent: "qBittorrent/5.2.2".into(),
 query: "info_hash={info_hash}&peer_id={peer_id}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&compact=1&key={key}{event}&numwant={numwant}".into(),
 numwant: 200,
 aliases: vec![],
 reserved_bytes: "0000000000100005".into(),
 fast_extension: true,
 keepalive_secs: 60,
 v_string: "qBittorrent/5.2.2".into(),
 m_dict: std::collections::BTreeMap::new(),
 reqq: Some(2000),
 encryption_preferred: None,
 send_upload_only: true,
 send_complete_ago: None,
 send_yourip: true,
 key_format: KeyFormat::UpperHex,
 }
 }

 #[test]
 fn percent_encode_null_byte() {
 assert_eq!(percent_encode_raw(&[0x00]), "%00");
 }

 #[test]
 fn percent_encode_full_byte_range() {
 // Every byte from 0-255 should encode without panic
 let bytes: Vec<u8> = (0..=255).collect();
 let encoded = percent_encode_raw(&bytes);
 assert!(!encoded.is_empty());
 }

 #[test]
 fn percent_encode_tilde_is_unreserved() {
 assert_eq!(percent_encode_raw(b"~"), "~");
 }

 #[test]
 fn build_url_appends_query() {
 let session = AnnounceSession::new(
 "http://tracker.example.com/announce/pk",
 [0u8; 20],
 &test_client(),
 6881,
 10,
 IntervalBounds { min_secs: 60, max_secs: 86400, default_secs: 1800 },
 PeerIdentity { peer_id: [0u8; protocol::PEER_ID_LEN], key: "DEADBEEF".into() },
 );
 let url = session.build_url(
 PeerState { uploaded: 100, downloaded: 50, left: 0 },
 Event::Started,
 );
 assert!(url.contains("info_hash="));
 assert!(url.contains("uploaded=100"));
 assert!(url.contains("event=started"));
 assert!(url.contains("port=6881"));
 }

 #[test]
 fn build_url_no_event_omits_param() {
 let session = AnnounceSession::new(
 "http://t.com/a",
 [0u8; 20],
 &test_client(),
 6881,
 10,
 IntervalBounds { min_secs: 60, max_secs: 86400, default_secs: 1800 },
 PeerIdentity { peer_id: [0u8; protocol::PEER_ID_LEN], key: "DEADBEEF".into() },
 );
 let url = session.build_url(
 PeerState { uploaded: 0, downloaded: 0, left: 0 },
 Event::None,
 );
 assert!(!url.contains("event="));
 }

 #[test]
 fn build_url_with_existing_query() {
 let session = AnnounceSession::new(
 "http://t.com/a?passkey=abc",
 [0u8; 20],
 &test_client(),
 6881,
 10,
 IntervalBounds { min_secs: 60, max_secs: 86400, default_secs: 1800 },
 PeerIdentity { peer_id: [0u8; protocol::PEER_ID_LEN], key: "DEADBEEF".into() },
 );
 let url = session.build_url(
 PeerState { uploaded: 0, downloaded: 0, left: 0 },
 Event::None,
 );
 assert!(url.contains("?passkey=abc&"));
 }

 // Regression: AnnounceSession must use the provided PeerIdentity
 //
 // The peer_id advertised to the tracker must be the one passed in via
 // PeerIdentity, not a freshly generated random one. If AnnounceSession
 // ignores the provided identity and generates its own, the tracker
 // sees a different peer on every restart and never credits resumed
 // cumulative counters (delta = 0).

 #[test]
 fn session_uses_provided_peer_identity() {
 let mut peer_id = [0u8; protocol::PEER_ID_LEN];
 peer_id[0] = 0xAB;
 peer_id[19] = 0xCD;
 let identity = PeerIdentity { peer_id, key: "1234ABCD".into() };
 let session = AnnounceSession::new(
 "http://t.com/a",
 [0u8; 20],
 &test_client(),
 6881,
 10,
 IntervalBounds { min_secs: 60, max_secs: 86400, default_secs: 1800 },
 identity,
 );
 assert_eq!(session.peer_id, peer_id, "session must use the provided peer_id, not a random one");
 assert_eq!(session.key, "1234ABCD", "session must use the provided key");
 }

 #[test]
 fn two_sessions_with_same_identity_have_same_peer_id() {
 let identity = PeerIdentity { peer_id: [0x42; protocol::PEER_ID_LEN], key: "FFFFFFFF".into() };
 let s1 = AnnounceSession::new(
 "http://t.com/a", [0u8; 20], &test_client(), 6881, 10,
 IntervalBounds { min_secs: 60, max_secs: 86400, default_secs: 1800 },
 identity.clone(),
 );
 let s2 = AnnounceSession::new(
 "http://t.com/a", [0u8; 20], &test_client(), 6881, 10,
 IntervalBounds { min_secs: 60, max_secs: 86400, default_secs: 1800 },
 identity,
 );
 assert_eq!(s1.peer_id, s2.peer_id, "two sessions with the same identity must have the same peer_id");
 assert_eq!(s1.key, s2.key, "two sessions with the same identity must have the same key");
 }

 // min interval (BEP-3)

 #[test]
 fn parse_min_interval_present() {
 let body = b"d8:intervali900e12:min intervali1800e8:completei5e10:incompletei10ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.interval, 900);
 assert_eq!(resp.min_interval, Some(1800));
 assert_eq!(resp.effective_interval(), 1800, "tracker min interval should override interval");
 }

 #[test]
 fn parse_min_interval_absent() {
 let body = b"d8:intervali900e8:completei5e10:incompletei10ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.interval, 900);
 assert_eq!(resp.min_interval, None);
 assert_eq!(resp.effective_interval(), 900, "without min interval, use interval");
 }

 #[test]
 fn parse_min_interval_lower_than_interval_uses_interval() {
 let body = b"d8:intervali1800e12:min intervali60e8:completei5ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.interval, 1800);
 assert_eq!(resp.min_interval, Some(60));
 assert_eq!(resp.effective_interval(), 1800, "when min < interval, use interval");
 }

 #[test]
 fn parse_min_interval_zero_is_ignored() {
 let body = b"d8:intervali900e12:min intervali0ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.min_interval, None, "zero min interval should be filtered out");
 }

 #[test]
 fn parse_min_interval_negative_is_ignored() {
 let body = b"d8:intervali900e12:min intervali-1ee";
 let resp = parse_response(body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.min_interval, None, "negative min interval should be filtered out");
 }

 // peers6 (BEP-7)

 #[test]
 fn parse_peers6_ipv6() {
 // 1 IPv4 peer (6 bytes) + 1 IPv6 peer (18 bytes)
 let peers4: Vec<u8> = vec![0x7f, 0x00, 0x00, 0x01, 0x1a, 0xe1];
 let peers6: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x1a, 0xe1];
 let mut body = b"d8:intervali1800e5:peers".to_vec();
 body.extend_from_slice(peers4.len().to_string().as_bytes());
 body.push(b':');
 body.extend_from_slice(&peers4);
 body.extend_from_slice(b"6:peers6");
 body.extend_from_slice(peers6.len().to_string().as_bytes());
 body.push(b':');
 body.extend_from_slice(&peers6);
 body.push(b'e');
 let resp = parse_response(&body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.peer_count, 2, "should count 1 IPv4 + 1 IPv6 peer");
 }

 #[test]
 fn parse_peers6_only() {
 let peers6: Vec<u8> = vec![0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x1a, 0xe1];
 let mut body = b"d8:intervali1800e6:peers6".to_vec();
 body.extend_from_slice(peers6.len().to_string().as_bytes());
 body.push(b':');
 body.extend_from_slice(&peers6);
 body.push(b'e');
 let resp = parse_response(&body, 60, 86400, 1800).unwrap();
 assert_eq!(resp.peer_count, 1, "should count 1 IPv6 peer");
 }
}
