//! .torrent file parsing and generation.
//!
//! Parsing extracts announce URL, info_hash, name, and total size from an
//! existing .torrent file. Generation builds a minimal dummy .torrent for
//! fingerprint capture - valid enough for real clients to accept and announce
//! to our tracker, without containing real data.

use crate::bencode;
use crate::data::protocol;
use std::collections::BTreeMap;

pub struct TorrentMeta {
 pub announce_url: String,
 pub info_hash: [u8; protocol::INFO_HASH_LEN],
 pub name: String,
 pub total_size: u64,
}

/// Parse a .torrent file's raw bytes into metadata needed for announcing.
pub fn parse(data: &[u8]) -> Result<TorrentMeta, String> {
 let root = bencode::decode(data)?;
 let announce = root
 .get(protocol::K_ANNOUNCE)
 .and_then(|v| v.as_str())
 .ok_or("missing 'announce' key")?
 .to_string();

 let info_hash = bencode::info_hash(data)?;

 let info = root.get(protocol::K_INFO).ok_or("missing 'info' key")?;
 let name = info
 .get(protocol::K_NAME)
 .and_then(|v| v.as_str())
 .ok_or("missing 'info.name'")?
 .to_string();

 let total_size = if let Some(len) = info.get(protocol::K_LENGTH).and_then(|v| v.as_int()) {
 len as u64
 } else if let Some(files) = info.get(protocol::K_FILES).and_then(|v| {
 if let bencode::Value::List(l) = v {
 Some(l)
 } else {
 None
 }
 }) {
 let mut sum: u64 = 0;
 for f in files {
 if let Some(len) = f.get(protocol::K_LENGTH).and_then(|v| v.as_int()) {
 sum += len as u64;
 }
 }
 sum
 } else {
 return Err("no 'length' or 'files' in info dict".into());
 };

 Ok(TorrentMeta {
 announce_url: announce,
 info_hash,
 name,
 total_size,
 })
}

/// Generate a minimal .torrent file for fingerprint capture.
///
/// The torrent contains a dummy `info` dict - a single file with one piece
/// and a placeholder SHA-1 hash. Real BitTorrent clients will accept this
/// torrent and announce to `announce_url`. The `info_hash` is SHA-1 of the
/// bencoded `info` dict, matching what clients will recompute and use in
/// their announce requests.
///
/// The torrent does NOT contain real data - download attempts will fail hash
/// verification. This is intentional: we only need the announce and peer
/// handshake to capture the client's fingerprint.
///
/// Returns `(torrent_bytes, info_hash)`.
pub fn generate(announce_url: &str, name: &str) -> (Vec<u8>, [u8; protocol::INFO_HASH_LEN]) {
 // Build the info dict (single-file torrent, one piece).
 // Keys are sorted by BTreeMap byte order: length < name < piece length < pieces.
 let mut info = BTreeMap::new();
 info.insert(
 protocol::K_LENGTH.to_vec(),
 bencode::Value::Int(protocol::CAPTURE_TORRENT_SIZE as i64),
 );
 info.insert(
 protocol::K_NAME.to_vec(),
 bencode::Value::Bytes(name.as_bytes().to_vec()),
 );
 info.insert(
 protocol::K_PIECE_LENGTH.to_vec(),
 bencode::Value::Int(protocol::CAPTURE_PIECE_LENGTH as i64),
 );
 info.insert(
 protocol::K_PIECES.to_vec(),
 bencode::Value::Bytes(
 protocol::CAPTURE_DUMMY_PIECE_HASH
 .repeat(
 (protocol::CAPTURE_TORRENT_SIZE.div_ceil(protocol::CAPTURE_PIECE_LENGTH))
 as usize,
 ),
 ),
 );

 // Build the top-level torrent dict.
 // Keys sorted: announce < info.
 let mut top = BTreeMap::new();
 top.insert(
 protocol::K_ANNOUNCE.to_vec(),
 bencode::Value::Bytes(announce_url.as_bytes().to_vec()),
 );
 top.insert(protocol::K_INFO.to_vec(), bencode::Value::Dict(info));

 let torrent_bytes = bencode::encode(&bencode::Value::Dict(top));
 let info_hash = bencode::info_hash(&torrent_bytes)
 .expect("generated torrent must contain a valid info dict");
 (torrent_bytes, info_hash)
}

#[cfg(test)]
mod tests {
 use super::*;

 #[test]
 fn empty_input() {
 assert!(parse(b"").is_err());
 }

 #[test]
 fn not_a_dict() {
 assert!(parse(b"i42e").is_err());
 assert!(parse(b"l4:teste").is_err());
 }

 #[test]
 fn missing_announce() {
 let data = b"d4:infod6:lengthi100e4:name4:testee";
 assert!(parse(data).is_err());
 }

 #[test]
 fn missing_info() {
 let data = b"d8:announce20:http://example.com/ae";
 assert!(parse(data).is_err());
 }

 #[test]
 fn missing_name() {
 let data = b"d8:announce20:http://example.com/a4:infod6:lengthi100ee";
 assert!(parse(data).is_err());
 }

 #[test]
 fn missing_length_and_files() {
 let data = b"d8:announce20:http://example.com/a4:infod4:name4:testee";
 assert!(parse(data).is_err());
 }

 #[test]
 fn single_file_valid() {
 let data = b"d8:announce20:http://example.com/a4:infod6:lengthi100e4:name4:testee";
 let meta = parse(data).unwrap();
 assert_eq!(meta.announce_url, "http://example.com/a");
 assert_eq!(meta.name, "test");
 assert_eq!(meta.total_size, 100);
 }

 #[test]
 fn multi_file_valid() {
 // info dict with files list: [{length: 50, path: [a.txt]}, {length: 30, path: [b.txt]}]
 let data = b"d8:announce20:http://example.com/a4:infod5:filesld6:lengthi50e4:pathl5:a.txteed6:lengthi30e4:pathl5:b.txteee4:name4:testee";
 let meta = parse(data).unwrap();
 assert_eq!(meta.total_size, 80);
 }

 #[test]
 fn info_hash_is_computed() {
 let data = b"d8:announce20:http://example.com/a4:infod6:lengthi100e4:name4:testee";
 let meta = parse(data).unwrap();
 // Info dict d6:lengthi100e4:name4:teste → SHA-1 info_hash.
 assert_eq!(
 crate::bencode::hex_encode(&meta.info_hash),
 crate::data::fixtures::SAMPLE_TORRENT_INFO_HASH
 );
 // Same input → same hash (determinism).
 let meta2 = parse(data).unwrap();
 assert_eq!(meta.info_hash, meta2.info_hash);
 }

 // generate

 #[test]
 fn generate_roundtrips_through_parse() {
 let (torrent, info_hash) = generate("http://127.0.0.1:6881/capture/abc/announce", "capture-test");
 let meta = parse(&torrent).unwrap();
 assert_eq!(meta.announce_url, "http://127.0.0.1:6881/capture/abc/announce");
 assert_eq!(meta.name, "capture-test");
 assert_eq!(meta.total_size, protocol::CAPTURE_TORRENT_SIZE);
 assert_eq!(meta.info_hash, info_hash);
 }

 #[test]
 fn generate_info_hash_is_deterministic() {
 let (_, hash1) = generate("http://example.com/a", "test");
 let (_, hash2) = generate("http://example.com/a", "test");
 assert_eq!(hash1, hash2, "same inputs must produce same info_hash");
 }

 #[test]
 fn generate_different_announce_same_info_hash() {
 // The announce URL is NOT part of the info dict, so changing it
 // must NOT change the info_hash. This is the fundamental BT property
 // that lets the same torrent content be tracked by multiple trackers.
 let (_, hash1) = generate("http://tracker-a.com/announce", "same-name");
 let (_, hash2) = generate("http://tracker-b.com/announce", "same-name");
 assert_eq!(
 hash1, hash2,
 "different announce URLs must produce the same info_hash"
 );
 }

 #[test]
 fn generate_different_name_different_info_hash() {
 // The name IS part of the info dict, so changing it must change
 // the info_hash.
 let (_, hash1) = generate("http://example.com/announce", "name-a");
 let (_, hash2) = generate("http://example.com/announce", "name-b");
 assert_ne!(
 hash1, hash2,
 "different names must produce different info_hashes"
 );
 }

 #[test]
 fn generate_pieces_has_correct_count() {
 // With CAPTURE_TORRENT_SIZE (1 MiB) and CAPTURE_PIECE_LENGTH (16 KiB),
 // there are 64 pieces → pieces = 64 × 20 = 1280 bytes.
 let (torrent, _) = generate("http://example.com/announce", "test");
 let root = bencode::decode(&torrent).unwrap();
 let info = root.get(protocol::K_INFO).unwrap();
 let pieces = info.get(protocol::K_PIECES).unwrap();
 let expected_pieces = (protocol::CAPTURE_TORRENT_SIZE.div_ceil(protocol::CAPTURE_PIECE_LENGTH)) as usize;
 assert_eq!(pieces.as_bytes().unwrap().len(), expected_pieces * protocol::SHA1_LEN);
 }

 #[test]
 fn generate_piece_length_matches_constant() {
 let (torrent, _) = generate("http://example.com/announce", "test");
 let root = bencode::decode(&torrent).unwrap();
 let info = root.get(protocol::K_INFO).unwrap();
 let pl = info.get(protocol::K_PIECE_LENGTH).unwrap();
 assert_eq!(pl.as_int(), Some(protocol::CAPTURE_PIECE_LENGTH as i64));
 }

 #[test]
 fn generate_info_dict_keys_sorted_canonically() {
 // Verify the bencoded info dict has keys in canonical (sorted) order.
 // Expected order: length < name < piece length < pieces
 let (torrent, _) = generate("http://example.com/announce", "test");
 let root = bencode::decode(&torrent).unwrap();
 let info = root.get(protocol::K_INFO).unwrap();
 let dict = info.as_dict().unwrap();
 let keys: Vec<&Vec<u8>> = dict.keys().collect();
 let mut sorted = keys.clone();
 sorted.sort();
 assert_eq!(keys, sorted, "info dict keys must be canonically sorted");
 }

 #[test]
 fn generate_torrent_is_valid_bencode() {
 let (torrent, _) = generate("http://example.com/announce", "test");
 // Must decode without error
 assert!(bencode::decode(&torrent).is_ok());
 }

 #[test]
 fn generate_empty_name_still_valid_bencode() {
 // Edge case: empty name. The torrent is still valid bencode - the
 // client may reject it, but that's the client's problem. Our job
 // is to produce valid bencode, not to validate user input.
 let (torrent, _) = generate("http://example.com/announce", "");
 assert!(bencode::decode(&torrent).is_ok());
 let meta = parse(&torrent).unwrap();
 assert_eq!(meta.name, "");
 }
}
