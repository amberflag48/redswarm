//! Minimal bencode codec with position tracking.
//!
//! Bencode is the BitTorrent serialization format (BEP-3). This module decodes
//! bencoded bytes into a `Value` tree, encodes a `Value` tree back into
//! canonical bencode bytes, and provides raw-byte extraction for info_hash
//! computation (SHA-1 of the raw `info` dict bytes from a .torrent).

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub enum Value {
 Bytes(Vec<u8>),
 Int(i64),
 List(Vec<Value>),
 Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
 pub fn as_bytes(&self) -> Option<&[u8]> {
 match self {
 Value::Bytes(b) => Some(b),
 _ => None,
 }
 }

 pub fn as_int(&self) -> Option<i64> {
 match self {
 Value::Int(i) => Some(*i),
 _ => None,
 }
 }

 pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
 match self {
 Value::Dict(d) => Some(d),
 _ => None,
 }
 }

 pub fn get(&self, key: &[u8]) -> Option<&Value> {
 self.as_dict()?.get(key)
 }

 pub fn as_str(&self) -> Option<&str> {
 std::str::from_utf8(self.as_bytes()?).ok()
 }
}

struct Decoder<'a> {
 data: &'a [u8],
 pos: usize,
}

impl<'a> Decoder<'a> {
 fn new(data: &'a [u8]) -> Self {
 Self { data, pos: 0 }
 }

 fn peek(&self) -> Option<u8> {
 self.data.get(self.pos).copied()
 }

 fn decode_value(&mut self) -> Result<Value, String> {
 match self.peek() {
 Some(b'd') => self.decode_dict(),
 Some(b'l') => self.decode_list(),
 Some(b'i') => self.decode_int(),
 Some(c) if c.is_ascii_digit() => self.decode_bytes(),
 Some(c) => Err(format!("invalid bencode type byte: {:#x} at {}", c, self.pos)),
 None => Err("unexpected end of data".into()),
 }
 }

 fn decode_int(&mut self) -> Result<Value, String> {
 self.pos += 1; // skip 'i'
 let start = self.pos;
 while self.peek() != Some(b'e') {
 if self.pos >= self.data.len() {
 return Err("unterminated integer".into());
 }
 self.pos += 1;
 }
 let s = std::str::from_utf8(&self.data[start..self.pos])
 .map_err(|_| "invalid integer encoding".to_string())?;
 let n: i64 = s.parse().map_err(|_| format!("invalid integer: {}", s))?;
 self.pos += 1; // skip 'e'
 Ok(Value::Int(n))
 }

 fn decode_bytes(&mut self) -> Result<Value, String> {
 let start = self.pos;
 while self.peek() != Some(b':') {
 if self.pos >= self.data.len() {
 return Err("unterminated byte-string length".into());
 }
 self.pos += 1;
 }
 let len_str = std::str::from_utf8(&self.data[start..self.pos])
 .map_err(|_| "invalid byte-string length".to_string())?;
 let len: usize = len_str
 .parse()
 .map_err(|_| format!("invalid byte-string length: {}", len_str))?;
 self.pos += 1; // skip ':'
 let end = self.pos.checked_add(len)
 .ok_or("byte-string length overflows usize")?;
 if end > self.data.len() {
 return Err("byte-string extends past end of data".into());
 }
 let bytes = self.data[self.pos..end].to_vec();
 self.pos = end;
 Ok(Value::Bytes(bytes))
 }

 fn decode_list(&mut self) -> Result<Value, String> {
 self.pos += 1; // skip 'l'
 let mut items = Vec::new();
 while self.peek() != Some(b'e') {
 if self.pos >= self.data.len() {
 return Err("unterminated list".into());
 }
 items.push(self.decode_value()?);
 }
 self.pos += 1; // skip 'e'
 Ok(Value::List(items))
 }

 fn decode_dict(&mut self) -> Result<Value, String> {
 self.pos += 1; // skip 'd'
 let mut map = BTreeMap::new();
 while self.peek() != Some(b'e') {
 if self.pos >= self.data.len() {
 return Err("unterminated dict".into());
 }
 let key = self.decode_bytes()?;
 let Value::Bytes(key_bytes) = key else {
 return Err("dict key is not a byte-string".into());
 };
 let val = self.decode_value()?;
 map.insert(key_bytes, val);
 }
 self.pos += 1; // skip 'e'
 Ok(Value::Dict(map))
 }
}

/// Decode bencoded data into a `Value` tree.
pub fn decode(data: &[u8]) -> Result<Value, String> {
 let mut dec = Decoder::new(data);
 let val = dec.decode_value()?;
 Ok(val)
}

/// Decode bencoded data, wrapping the error with a `"bencode decode: "` prefix.
/// Shared by `announce::parse_response` so the error-message format lives in
/// one place.
pub fn decode_checked(data: &[u8]) -> Result<Value, String> {
 decode(data).map_err(|e| format!("bencode decode: {e}"))
}

/// Encode a `Value` tree into canonical bencode bytes.
///
/// Dict keys are emitted in lexicographic byte order - guaranteed by the
/// `BTreeMap<Vec<u8>, _>` backing of `Value::Dict`, which is exactly the
/// canonical ordering BEP-3 requires. Integers are formatted as base-10 ASCII
/// with no leading zeros (except `0` itself) and no `+` sign. The output is
/// always valid bencode and round-trips through `decode`.
pub fn encode(value: &Value) -> Vec<u8> {
 let mut buf = Vec::new();
 encode_to(value, &mut buf);
 buf
}

fn encode_to(value: &Value, buf: &mut Vec<u8>) {
 match value {
 Value::Int(n) => {
 buf.push(b'i');
 buf.extend_from_slice(n.to_string().as_bytes());
 buf.push(b'e');
 }
 Value::Bytes(bytes) => {
 buf.extend_from_slice(bytes.len().to_string().as_bytes());
 buf.push(b':');
 buf.extend_from_slice(bytes);
 }
 Value::List(items) => {
 buf.push(b'l');
 for item in items {
 encode_to(item, buf);
 }
 buf.push(b'e');
 }
 Value::Dict(map) => {
 buf.push(b'd');
 for (key, val) in map {
 buf.extend_from_slice(key.len().to_string().as_bytes());
 buf.push(b':');
 buf.extend_from_slice(key);
 encode_to(val, buf);
 }
 buf.push(b'e');
 }
 }
}

/// Encode a byte slice as lowercase hex.
pub fn hex_encode(bytes: &[u8]) -> String {
 bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Decode a hex string into a byte vector. Returns `Err` on odd length or
/// non-hex characters. Shared by info_hash parsing and reserved_bytes parsing.
pub fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
 if !hex.len().is_multiple_of(2) {
 return Err(format!("hex string must have even length, got {}", hex.len()));
 }
 let bytes = hex.as_bytes();
 let mut out = Vec::with_capacity(hex.len() / 2);
 for i in 0..bytes.len() / 2 {
 let hi = hex_nibble(bytes[i * 2])?;
 let lo = hex_nibble(bytes[i * 2 + 1])?;
 out.push((hi << 4) | lo);
 }
 Ok(out)
}

/// Decode a 40-char hex string into a 20-byte array.
///
/// Returns `Err` if the input is not exactly 40 chars or contains non-hex
/// characters. Never silently corrupts invalid input. Used for both
/// info_hash and peer_id (both are 20-byte values per BEP-3).
pub fn hex_decode_20(hex: &str) -> Result<[u8; crate::data::protocol::INFO_HASH_LEN], String> {
 if hex.len() != crate::data::protocol::INFO_HASH_HEX_LEN {
 return Err(format!(
 "expected exactly {} hex chars, got {}",
 crate::data::protocol::INFO_HASH_HEX_LEN,
 hex.len()
 ));
 }
 let bytes = hex.as_bytes();
 let mut out = [0u8; crate::data::protocol::INFO_HASH_LEN];
 for i in 0..crate::data::protocol::INFO_HASH_LEN {
 let hi = hex_nibble(bytes[i * 2])?;
 let lo = hex_nibble(bytes[i * 2 + 1])?;
 out[i] = (hi << 4) | lo;
 }
 Ok(out)
}

pub fn hex_nibble(c: u8) -> Result<u8, String> {
 match c {
 b'0'..=b'9' => Ok(c - b'0'),
 b'a'..=b'f' => Ok(c - b'a' + 10),
 b'A'..=b'F' => Ok(c - b'A' + 10),
 _ => Err(format!("invalid hex char: {c:?}")),
 }
}

/// Extract the raw bytes of the `info` dict from a .torrent file and compute its SHA-1.
///
/// The info_hash is SHA-1 of the *original* bencoded `info` value bytes - not a
/// re-serialization. This function parses just enough to locate the info dict's
/// byte range, then hashes the raw slice.
pub fn info_hash(data: &[u8]) -> Result<[u8; crate::data::protocol::INFO_HASH_LEN], String> {
 let mut dec = Decoder::new(data);
 if dec.peek() != Some(b'd') {
 return Err("torrent file does not start with a dict".into());
 }
 dec.pos += 1; // skip 'd'

 while dec.peek() != Some(b'e') {
 if dec.pos >= dec.data.len() {
 return Err("unterminated torrent dict".into());
 }
 let key = dec.decode_bytes()?;
 let Value::Bytes(key_bytes) = key else {
 return Err("torrent dict key is not a byte-string".into());
 };
 let val_start = dec.pos;
 let _val = dec.decode_value()?;
 let val_end = dec.pos;

 if key_bytes == crate::data::protocol::K_INFO {
 use sha1::{Sha1, Digest};
 let mut hasher = Sha1::new();
 hasher.update(&dec.data[val_start..val_end]);
 return Ok(hasher.finalize().into());
 }
 }
 Err("no 'info' key in torrent".into())
}

#[cfg(test)]
mod tests {
 use super::*;

 // decode failures

 #[test]
 fn empty_input() {
 assert!(decode(b"").is_err());
 }

 #[test]
 fn invalid_type_byte() {
 assert!(decode(b"x").is_err());
 assert!(decode(b"z123").is_err());
 }

 #[test]
 fn unterminated_integer() {
 assert!(decode(b"i42").is_err()); // missing 'e'
 assert!(decode(b"i").is_err());
 }

 #[test]
 fn non_numeric_integer() {
 assert!(decode(b"iabe").is_err());
 assert!(decode(b"i1.5e").is_err()); // floats not allowed
 }

 #[test]
 fn unterminated_byte_string_length() {
 assert!(decode(b"5").is_err()); // just a number, no colon
 assert!(decode(b"5abc").is_err()); // no colon
 }

 #[test]
 fn byte_string_extends_past_end() {
 // claims 10 bytes but only provides 3
 assert!(decode(b"10:abc").is_err());
 }

 #[test]
 fn byte_string_length_overflow_usize_max() {
 // Adversarial: length = usize::MAX. Before the fix, `self.pos + len`
 // overflowed (debug panic / release wrap → slice panic). Must now be a
 // clean Err on both 32- and 64-bit. On 32-bit the length itself won't
 // parse as usize and yields Err; on 64-bit it parses and checked_add
 // catches the overflow. Either way: Err, never a panic.
 assert!(decode(b"18446744073709551615:x").is_err());
 }

 #[test]
 fn byte_string_huge_length_truncated() {
 // Huge but sub-usize::MAX length that does NOT overflow `pos + len`
 // (on 64-bit) but still exceeds the remaining data. Confirms the
 // `end > data.len()` guard is still reached after checked_add.
 assert!(decode(b"9999999999999999999:x").is_err());
 }

 #[test]
 fn byte_string_length_overflow_inside_dict() {
 // Same attack reachable through dict parsing (the path taken by tracker
 // responses and .torrent files). The malicious length is the value, not
 // the key, to exercise decode_value → decode_bytes from decode_dict.
 assert!(decode(b"d3:key18446744073709551615:xe").is_err());
 }

 #[test]
 fn unterminated_list() {
 assert!(decode(b"l").is_err()); // just 'l', no 'e'
 assert!(decode(b"li42e").is_err()); // missing closing 'e'
 }

 #[test]
 fn unterminated_dict() {
 assert!(decode(b"d").is_err()); // just 'd', no 'e'
 assert!(decode(b"d3:keyi42e").is_err()); // missing closing 'e'
 }

 #[test]
 fn dict_key_not_a_string() {
 // key is an integer, not a byte string
 assert!(decode(b"di42ei0ee").is_err());
 }

 #[test]
 fn dict_missing_value_for_key() {
 // key with no value before dict ends
 assert!(decode(b"d3:keye").is_err());
 }

 #[test]
 fn empty_integer() {
 assert!(decode(b"ie").is_err()); // no digits
 }

 #[test]
 fn byte_string_zero_length() {
 let v = decode(b"0:").unwrap();
 assert_eq!(v.as_bytes(), Some(&b""[..]));
 }

 #[test]
 fn empty_list() {
 let v = decode(b"le").unwrap();
 assert!(matches!(v, Value::List(l) if l.is_empty()));
 }

 #[test]
 fn empty_dict() {
 let v = decode(b"de").unwrap();
 assert!(matches!(v, Value::Dict(d) if d.is_empty()));
 }

 #[test]
 fn nested_structures() {
 // d3:keyl i42e i99e ee → {"key": [42, 99]}
 let v = decode(b"d3:keyli42ei99eee").unwrap();
 let d = v.as_dict().unwrap();
 let list = d.get(b"key".as_slice()).unwrap();
 assert!(matches!(list, Value::List(l) if l.len() == 2));
 }

 // info_hash failures

 #[test]
 fn info_hash_no_info_key() {
 let data = b"d8:announce20:http://example.com/e";
 assert!(info_hash(data).is_err());
 }

 #[test]
 fn info_hash_not_a_dict() {
 let data = b"i42e";
 assert!(info_hash(data).is_err());
 }

 #[test]
 fn info_hash_empty_dict() {
 let data = b"de";
 assert!(info_hash(data).is_err());
 }

 #[test]
 fn info_hash_valid() {
 // d4:infod6:lengthi100e4:name4:testee → info dict is
 // d6:lengthi100e4:name4:teste; SHA-1 of those raw bytes is the info_hash.
 let data = b"d4:infod6:lengthi100e4:name4:testee";
 let hash = info_hash(data).unwrap();
 assert_eq!(hex_encode(&hash), crate::data::fixtures::SAMPLE_TORRENT_INFO_HASH);
 }

 // hex helpers

 #[test]
 fn hex_roundtrip() {
 let bytes = [0x12, 0xAB, 0xCD, 0xEF, 0x00, 0xFF];
 let encoded = hex_encode(&bytes);
 assert_eq!(encoded, "12abcdef00ff");
 let decoded = hex_decode_20(&format!("{:040x}", 0u128)).unwrap();
 assert_eq!(decoded, [0u8; 20]);
 }

 #[test]
 fn hex_decode_handles_uppercase() {
 let decoded = hex_decode_20("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").unwrap();
 assert_eq!(decoded, [0xFFu8; 20]);
 }

 #[test]
 fn hex_decode_handles_mixed_case() {
 let decoded = hex_decode_20("aAbBcCdDeEfF0123456789aAbBcCdDeEfF012345").unwrap();
 assert_eq!(decoded[0], 0xAA);
 assert_eq!(decoded[1], 0xBB);
 }

 // hex_decode_20: adversarial / failure-path tests

 #[test]
 fn hex_decode_rejects_empty() {
 assert!(hex_decode_20("").is_err());
 }

 #[test]
 fn hex_decode_rejects_short() {
 let s = "abcdef0123456789abcdef0123456789abcdef0";
 assert_eq!(s.len(), 39);
 let err = hex_decode_20(s).unwrap_err();
 assert!(err.contains("40 hex chars"), "got: {err}");
 }

 #[test]
 fn hex_decode_rejects_long() {
 let s = "abcdef0123456789abcdef0123456789abcdef012";
 assert_eq!(s.len(), 41);
 let err = hex_decode_20(s).unwrap_err();
 assert!(err.contains("40 hex chars"), "got: {err}");
 }

 #[test]
 fn hex_decode_rejects_invalid_chars() {
 let s = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
 assert_eq!(s.len(), 40);
 let err = hex_decode_20(s).unwrap_err();
 assert!(err.contains("invalid hex char"), "got: {err}");
 }

 #[test]
 fn hex_decode_valid_40_chars() {
 let s = crate::data::fixtures::SAMPLE_INFO_HASH;
 let decoded = hex_decode_20(s).unwrap();
 assert_eq!(decoded[0], 0xAB);
 assert_eq!(decoded[1], 0xCD);
 assert_eq!(decoded[2], 0xEF);
 assert_eq!(decoded[3], 0x01);
 assert_eq!(decoded[19], 0x01);
 }

 // hex_decode (arbitrary length)

 #[test]
 fn hex_decode_valid_8_bytes() {
 let decoded = hex_decode("0000000000100005").unwrap();
 assert_eq!(decoded, vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x05]);
 }

 #[test]
 fn hex_decode_empty_string() {
 let result: Vec<u8> = hex_decode("").unwrap();
 assert!(result.is_empty());
 }

 #[test]
 fn hex_decode_rejects_odd_length() {
 assert!(hex_decode("abc").is_err());
 }

 #[test]
 fn hex_decode_rejects_invalid_chars_arbitrary() {
 assert!(hex_decode("xy00").is_err());
 }

 // encode: byte-exact format

 #[test]
 fn encode_empty_bytes() {
 assert_eq!(encode(&Value::Bytes(vec![])), b"0:");
 }

 #[test]
 fn encode_empty_list() {
 assert_eq!(encode(&Value::List(vec![])), b"le");
 }

 #[test]
 fn encode_empty_dict() {
 assert_eq!(encode(&Value::Dict(BTreeMap::new())), b"de");
 }

 #[test]
 fn encode_integer_zero() {
 assert_eq!(encode(&Value::Int(0)), b"i0e");
 }

 #[test]
 fn encode_negative_integer() {
 assert_eq!(encode(&Value::Int(-42)), b"i-42e");
 }

 #[test]
 fn encode_min_integer() {
 // i64::MIN - must not panic and must round-trip exactly.
 assert_eq!(
 encode(&Value::Int(i64::MIN)),
 b"i-9223372036854775808e"
 );
 }

 #[test]
 fn encode_simple_bytes_exact() {
 // "spam" → 4:spam
 assert_eq!(encode(&Value::Bytes(b"spam".to_vec())), b"4:spam");
 }

 #[test]
 fn encode_binary_bytes_not_terminated_early() {
 // Bytes containing b':' and b'e' must not confuse the encoder -
 // the length prefix unambiguously delimits the content.
 let payload = b"e:e:e".to_vec();
 let encoded = encode(&Value::Bytes(payload.clone()));
 assert_eq!(encoded, b"5:e:e:e");
 // Round-trip: decode must recover the exact bytes.
 assert_eq!(decode(&encoded).unwrap().as_bytes(), Some(&payload[..]));
 }

 #[test]
 fn encode_list_exact() {
 // ["spam", 42] → l4:spami42ee
 let list = Value::List(vec![Value::Bytes(b"spam".to_vec()), Value::Int(42)]);
 assert_eq!(encode(&list), b"l4:spami42ee");
 }

 // encode: canonical dict ordering

 #[test]
 fn encode_dict_keys_sorted() {
 // Insert keys in reverse order; BTreeMap sorts on insert so the
 // encoded output must be lexicographically sorted.
 let mut map = BTreeMap::new();
 map.insert(b"zebra".to_vec(), Value::Int(3));
 map.insert(b"alpha".to_vec(), Value::Int(1));
 map.insert(b"mid".to_vec(), Value::Int(2));
 let dict = Value::Dict(map);
 // d5:alphai1e3:midi2e5:zebrai3ee
 assert_eq!(
 encode(&dict),
 b"d5:alphai1e3:midi2e5:zebrai3ee"
 );
 }

 #[test]
 fn encode_dict_keys_sorted_by_raw_bytes_not_utf8() {
 // Bencode sorts keys by raw byte value, not by Unicode code point.
 // 0xFF (not valid UTF-8 lead byte) sorts after all ASCII.
 // Key 1 = [0x00], key 2 = [0xFF]. Byte order: 0x00 < 0xFF.
 let mut map = BTreeMap::new();
 map.insert(vec![0xFF], Value::Int(2));
 map.insert(vec![0x00], Value::Int(1));
 let dict = Value::Dict(map);
 assert_eq!(
 encode(&dict),
 b"d1:\x00i1e1:\xFFi2ee"
 );
 }

 #[test]
 fn encode_dict_no_leading_zeros_in_int_values() {
 // Integers must never have leading zeros - i01e is invalid bencode.
 let mut map = BTreeMap::new();
 map.insert(b"n".to_vec(), Value::Int(0));
 map.insert(b"m".to_vec(), Value::Int(7));
 let dict = Value::Dict(map);
 assert_eq!(encode(&dict), b"d1:mi7e1:ni0ee");
 }

 // encode: nested structures

 #[test]
 fn encode_nested_dict_in_list() {
 // [{"k": [1, 2]}] → ld1:kli1ei2eeee
 let mut inner = BTreeMap::new();
 inner.insert(b"k".to_vec(), Value::List(vec![Value::Int(1), Value::Int(2)]));
 let list = Value::List(vec![Value::Dict(inner)]);
 assert_eq!(encode(&list), b"ld1:kli1ei2eeee");
 }

 // encode: round-trip

 #[test]
 fn encode_roundtrip_complex() {
 // Build a structure, encode, decode, and verify equality.
 let mut info = BTreeMap::new();
 info.insert(b"length".to_vec(), Value::Int(100));
 info.insert(b"name".to_vec(), Value::Bytes(b"test".to_vec()));
 let mut top = BTreeMap::new();
 top.insert(b"announce".to_vec(), Value::Bytes(b"http://ex.com/".to_vec()));
 top.insert(b"info".to_vec(), Value::Dict(info));
 let original = Value::Dict(top);
 let encoded = encode(&original);
 let decoded = decode(&encoded).unwrap();
 // Structural equality: re-encode decoded and compare bytes (avoids
 // needing PartialEq on Value, which isn't derived).
 assert_eq!(encode(&decoded), encoded);
 }

 #[test]
 fn encode_roundtrip_adversarial_lengths() {
 // Byte strings with lengths that have leading-digit zeros in other
 // bases but not in decimal - 10 bytes, 100 bytes, 1 byte.
 for len in [1usize, 10, 100, 255, 256, 1000] {
 let v = Value::Bytes(vec![0x41; len]);
 let encoded = encode(&v);
 let decoded = decode(&encoded).unwrap();
 assert_eq!(decoded.as_bytes().map(|b| b.len()), Some(len));
 }
 }

 // encode: real-world shapes

 #[test]
 fn encode_torrent_info_hash_roundtrip() {
 // Encode a minimal torrent dict, compute info_hash on the result,
 // and verify it matches the golden fixture. This proves the encoder
 // produces bytes that the existing info_hash function can hash.
 let mut info = BTreeMap::new();
 info.insert(b"length".to_vec(), Value::Int(100));
 info.insert(b"name".to_vec(), Value::Bytes(b"test".to_vec()));
 let mut top = BTreeMap::new();
 top.insert(b"info".to_vec(), Value::Dict(info));
 let encoded = encode(&Value::Dict(top));
 let hash = info_hash(&encoded).unwrap();
 assert_eq!(
 hex_encode(&hash),
 crate::data::fixtures::SAMPLE_TORRENT_INFO_HASH
 );
 }

 #[test]
 fn encode_bep10_extension_handshake() {
 // BEP-10 ext handshake: d1:md11:ut_metadatai2e6:ut_pexi1ee1:v15:qBittorrent/4.5e
 // Keys sorted: m < v. Inside m: ut_metadata < ut_pex.
 // "qBittorrent/4.5" is 15 bytes.
 let mut m = BTreeMap::new();
 m.insert(b"ut_metadata".to_vec(), Value::Int(2));
 m.insert(b"ut_pex".to_vec(), Value::Int(1));
 let mut top = BTreeMap::new();
 top.insert(b"m".to_vec(), Value::Dict(m));
 top.insert(b"v".to_vec(), Value::Bytes(b"qBittorrent/4.5".to_vec()));
 let encoded = encode(&Value::Dict(top));
 assert_eq!(
 encoded,
 b"d1:md11:ut_metadatai2e6:ut_pexi1ee1:v15:qBittorrent/4.5e"
 );
 }

 #[test]
 fn encode_tracker_announce_response_compact() {
 // Minimal tracker response: d8:completei0e10:incompletei0e8:intervali1800e5:peers0:e
 // (empty peers list - compact, 0 bytes)
 let mut top = BTreeMap::new();
 top.insert(b"complete".to_vec(), Value::Int(0));
 top.insert(b"incomplete".to_vec(), Value::Int(0));
 top.insert(b"interval".to_vec(), Value::Int(1800));
 top.insert(b"peers".to_vec(), Value::Bytes(vec![]));
 let encoded = encode(&Value::Dict(top));
 assert_eq!(
 encoded,
 b"d8:completei0e10:incompletei0e8:intervali1800e5:peers0:e"
 );
 // Must decode back cleanly.
 assert!(decode(&encoded).is_ok());
 }
}
