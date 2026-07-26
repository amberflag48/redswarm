//! Shared test fixtures - canonical values reused across module tests so a
//! fixture change is one edit instead of ~16 retyped literals.
//!
//! Only compiled under `#[cfg(test)]`.

/// Canonical 40-hex info_hash fixture used by api, db, magnet, and bencode
/// tests. Replaces the ~16 scattered copies of this exact string.
pub const SAMPLE_INFO_HASH: &str = "abcdef0123456789abcdef0123456789abcdef01";

/// SHA-1 of the bencoded `d4:infod6:lengthi100e4:name4:testee` - the golden
/// info_hash value used by both `bencode::tests` and `torrent::tests`.
pub const SAMPLE_TORRENT_INFO_HASH: &str = "5894119219a94140d5274470f2da8bf7a2b06e39";
