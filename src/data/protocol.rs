//! BitTorrent protocol constants - bencode dict keys, magnet URI keys,
//! announce query-template placeholders, and the fixed-length binary field
//! sizes mandated by BEP-3.

// Fixed-length binary fields
/// BEP-3 peer_id length, in bytes.
pub const PEER_ID_LEN: usize = 20;
/// Maximum TCP/UDP port number (65535). Single source of truth for the
/// peer-port validation bound and the settings UI `max` attribute.
pub const MAX_PORT: u16 = u16::MAX;
/// String form of [`MAX_PORT`] for HTML `max` attributes.
pub const MAX_PORT_STR: &str = "65535";
/// Maximum peer_id_prefix length (same as PEER_ID_LEN - the prefix fills the
/// start of the peer_id, so it can't exceed 20 chars). Standard Azureus-style
/// prefixes are 8 chars (`-XXYYYY-`).
pub const PEER_ID_PREFIX_MAX_LEN: usize = PEER_ID_LEN;

// BEP-20 client identity (peer_id prefix → canonical client name)
// Azureus-style peer_ids prefix the 20 bytes with `-XXYYYY-` where `XX` is a
// 2-char client-family code (BEP-20). These are the canonical display names
// for each code - a fixed protocol registry used by
// `capture::decode_peer_id_prefix` to label a *captured* peer's client. They
// are NOT the user's editable config `label` (the display name for the user's
// own emulated client in config.toml); the two are independent values that
// happen to align for the stock clients. Single source of truth so the
// decoder and its tests reference one constant per family. The capture tests
// assert against the literal strings to independently pin the exact value (a
// const typo surfaces as a test failure).
pub const BEP20_CLIENT_QBITTORRENT: &str = "qBittorrent";
pub const BEP20_CLIENT_TRANSMISSION: &str = "Transmission";
pub const BEP20_CLIENT_DELUGE: &str = "Deluge";
pub const BEP20_CLIENT_UTORRENT: &str = "µTorrent";
pub const BEP20_CLIENT_BITTORRENT: &str = "BitTorrent";
pub const BEP20_CLIENT_RTORRENT: &str = "rTorrent";
pub const BEP20_CLIENT_VUZE: &str = "Vuze";
pub const BEP20_CLIENT_LIBTORRENT: &str = "libtorrent";

/// BEP-3 info_hash length, in bytes.
pub const INFO_HASH_LEN: usize = 20;
/// Hex-encoded info_hash length (40 = 20 bytes × 2).
pub const INFO_HASH_HEX_LEN: usize = 40;
/// Base32-encoded info_hash length (RFC 4648, no padding).
pub const INFO_HASH_BASE32_LEN: usize = 32;
/// Compact IPv4 peer record: 4 bytes IP + 2 bytes port.
pub const COMPACT_IPV4_PEER_LEN: usize = 6;
/// Compact IPv6 peer record: 16 bytes IP + 2 bytes port (BEP-7).
pub const COMPACT_IPV6_PEER_LEN: usize = 18;

/// SHA-1 digest length in bytes - used for piece hashes in the `pieces` field.
pub const SHA1_LEN: usize = 20;

// Capture torrent defaults
/// Piece length for generated capture torrents (16 KiB). Non-configurable -
/// the torrent is a dummy for fingerprint capture, not real data transfer.
pub const CAPTURE_PIECE_LENGTH: u64 = 16_384;
/// Total content size for generated capture torrents (1 MiB - large enough
/// that real clients try to connect to peers for it, but still a dummy).
pub const CAPTURE_TORRENT_SIZE: u64 = 1_048_576;
/// Placeholder SHA-1 piece hashes for capture torrents. The torrent is a dummy;
/// no real data exists. One hash per piece (64 pieces for 1 MiB / 16 KiB).
pub const CAPTURE_DUMMY_PIECE_HASH: [u8; SHA1_LEN] = [0u8; SHA1_LEN];

// Peer-wire protocol (BEP-3)
/// The protocol string sent in the handshake: `"BitTorrent protocol"`.
pub const PSTR: &[u8] = b"BitTorrent protocol";
/// Length prefix byte for the protocol string (19).
pub const PSTRLEN: u8 = 19;
/// Total handshake size: 1 (pstrlen) + 19 (pstr) + 8 (reserved) + 20 (info_hash) + 20 (peer_id).
pub const HANDSHAKE_LEN: usize = 1 + 19 + 8 + PEER_ID_LEN + INFO_HASH_LEN;
/// Reserved bytes length in the handshake.
pub const RESERVED_LEN: usize = 8;

/// Peer-wire message IDs (BEP-3).
pub const MSG_CHOKE: u8 = 0;
pub const MSG_UNCHOKE: u8 = 1;
pub const MSG_INTERESTED: u8 = 2;
pub const MSG_NOT_INTERESTED: u8 = 3;
pub const MSG_HAVE: u8 = 4;
pub const MSG_BITFIELD: u8 = 5;
pub const MSG_REQUEST: u8 = 6;
pub const MSG_PIECE: u8 = 7;
pub const MSG_CANCEL: u8 = 8;
/// BEP-6 Fast Extension: have_all (replaces bitfield for seeders).
pub const MSG_HAVE_ALL: u8 = 14;
/// BEP-6 Fast Extension: have_none.
pub const MSG_HAVE_NONE: u8 = 15;
/// BEP-6 Fast Extension: suggest piece.
pub const MSG_SUGGEST_PIECE: u8 = 13;
/// BEP-6 Fast Extension: reject request.
pub const MSG_REJECT_REQUEST: u8 = 16;
/// BEP-6 Fast Extension: allowed fast.
pub const MSG_ALLOWED_FAST: u8 = 17;
/// BEP-5 DHT port message.
pub const MSG_PORT: u8 = 9;
/// BEP-10 Extension protocol message.
pub const MSG_EXTENDED: u8 = 20;

/// BEP-10 extended message sub-ID for the extension handshake (the first
/// message after the wire handshake, sent as MSG_EXTENDED with sub-id 0).
pub const EXT_HANDSHAKE_ID: u8 = 0;

/// Reserved byte index for the LTEP (BEP-10 Extension Protocol) capability bit.
pub const LTEP_BYTE_INDEX: usize = 5;
/// Bit mask for the LTEP capability in reserved bytes: `reserved[5] & 0x10`.
pub const LTEP_BIT_MASK: u8 = 0x10;

/// Reserved byte index for DHT (BEP-5) capability.
pub const DHT_BYTE_INDEX: usize = 7;
/// Bit mask for DHT capability: `reserved[7] & 0x01`.
pub const DHT_BIT_MASK: u8 = 0x01;

/// Reserved byte index for Fast Extension (BEP-6).
pub const FAST_EXT_BYTE_INDEX: usize = 7;
/// Bit mask for Fast Extension: `reserved[7] & 0x04`.
pub const FAST_EXT_BIT_MASK: u8 = 0x04;

/// Length of the message header: 4-byte BE length prefix + 1-byte message ID.
pub const MSG_HEADER_LEN: usize = 5;
/// Maximum message body we accept from peers (64 KiB - anything larger is hostile).
pub const MAX_PEER_MSG_LEN: usize = 65_536;
/// Size of the discard buffer for draining peer message bodies.
/// Never grows - bounds memory per connection regardless of message size.
pub const DISCARD_BUF_LEN: usize = 256;
/// Keepalive message: 4 zero bytes (length prefix = 0, no message ID).
pub const KEEPALIVE_MSG: [u8; 4] = [0, 0, 0, 0];
/// Bitfield payload sent when Fast Extension is not available - claims all
/// pieces (single byte, all bits set = 8 pieces). We don't know the real
/// piece count, but this is sufficient to appear as a seeder.
pub const SEEDER_BITFIELD: [u8; 1] = [0xFF];
/// Reserved bytes start index in the handshake.
pub const RESERVED_OFFSET: usize = 20;
/// Info hash start index in the handshake.
pub const INFO_HASH_OFFSET: usize = 28;
/// Peer ID start index in the handshake.
pub const PEER_ID_OFFSET: usize = 48;


// Announce query-template placeholders
/// Substituted in `announce::build_url` against `ClientSpecConfig.query`.
pub const Q_INFO_HASH: &str = "{info_hash}";
pub const Q_PEER_ID: &str = "{peer_id}";
pub const Q_PORT: &str = "{port}";
pub const Q_UPLOADED: &str = "{uploaded}";
pub const Q_DOWNLOADED: &str = "{downloaded}";
pub const Q_LEFT: &str = "{left}";
pub const Q_KEY: &str = "{key}";
pub const Q_EVENT: &str = "{event}";
pub const Q_NUMWANT: &str = "{numwant}";

// Bencode dict keys (BEP-3 / BEP-48)
pub const K_ANNOUNCE: &[u8] = b"announce";
pub const K_INFO: &[u8] = b"info";
pub const K_NAME: &[u8] = b"name";
pub const K_LENGTH: &[u8] = b"length";
pub const K_FILES: &[u8] = b"files";
pub const K_PIECE_LENGTH: &[u8] = b"piece length";
pub const K_PIECES: &[u8] = b"pieces";
pub const K_FAILURE_REASON: &[u8] = b"failure reason";
pub const K_INTERVAL: &[u8] = b"interval";
pub const K_COMPLETE: &[u8] = b"complete";
pub const K_INCOMPLETE: &[u8] = b"incomplete";
pub const K_PEERS: &[u8] = b"peers";
pub const K_IP: &[u8] = b"ip";
pub const K_PORT: &[u8] = b"port";

/// BEP-3 tracker response: minimum re-announce interval (distinct from `interval`).
pub const K_MIN_INTERVAL: &[u8] = b"min interval";
/// BEP-7 compact IPv6 peers (18-byte records: 16 IP + 2 port).
pub const K_PEERS6: &[u8] = b"peers6";

// BEP-10 extension handshake dict keys
/// Extension name → local message ID mapping dict key.
pub const K_M: &[u8] = b"m";
/// Client name + version string dict key.
pub const K_V: &[u8] = b"v";
/// Max outstanding request queue size dict key.
pub const K_REQQ: &[u8] = b"reqq";
/// Encryption preference (0/1) - whether the client prefers/requires encryption.
pub const K_E: &[u8] = b"e";
/// Upload-only flag (0/1, BEP-21) - whether the client is a partial seed.
pub const K_UPLOAD_ONLY: &[u8] = b"upload_only";
/// Seconds since the torrent completed (libtorrent-specific).
pub const K_COMPLETE_AGO: &[u8] = b"complete_ago";
/// Peer's compact IP as seen by us (4 or 16 bytes).
pub const K_YOURIP: &[u8] = b"yourip";
/// Listen port (outgoing connections only).
pub const K_P: &[u8] = b"p";
/// Info-dict size in bytes (BEP-9, sent when ut_metadata is advertised).
pub const K_METADATA_SIZE: &[u8] = b"metadata_size";
/// Compact IPv4 bind address (4 bytes).
pub const K_IPV4: &[u8] = b"ipv4";
/// Compact IPv6 bind address (16 bytes).
pub const K_IPV6: &[u8] = b"ipv6";
/// Share-mode flag (libtorrent-specific).
pub const K_SHARE_MODE: &[u8] = b"share_mode";

// Magnet URI

// BEP-10 extension names (protocol constants, not configurable)
/// BEP-11 Peer Exchange extension name.
pub const EXT_UT_PEX: &str = "ut_pex";
/// BEP-9 Metadata exchange extension name.
pub const EXT_UT_METADATA: &str = "ut_metadata";

// HTTP header constants (non-configurable - all BT clients send these)
/// All real BitTorrent clients send `Connection: close` for tracker announces.
pub const HTTP_CONNECTION_CLOSE: (&str, &str) = ("connection", "close");
/// MIME type for `.torrent` files and bencoded tracker responses.
pub const MIME_BITTORRENT: &str = "application/x-bittorrent";
/// Custom header carrying the capture session token (set on the `.torrent`
/// download response so the capture UI can echo it back).
pub const X_CAPTURE_TOKEN: &str = "X-Capture-Token";

// HTTP Cache-Control directives (FRONTEND.md §2)
/// `no-cache` - revalidate on every request (HTML document, API, non-fingerprinted static).
pub const CACHE_NO_CACHE: &str = "no-cache";
/// `public, max-age=31536000, immutable` - fingerprinted bundle, cache forever.
pub const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

// HTTP route paths (shared between router registration and URL builders)
/// Capture announce route path template (registered in the router and built
/// by `capture::start`). Keeping both sides on the same constant prevents
/// silent desync on a route rename.
pub const CAPTURE_ANNOUNCE_PATH: &str = "/capture/{token}/announce";
/// Capture scrape route path template.
pub const CAPTURE_SCRAPE_PATH: &str = "/capture/{token}/scrape";

// Minimal bencoded scrape response (BEP-48)
/// `d5:filesd8:completei1e10:downloadedi0e10:incompletei0eee` - a scrape
/// response claiming 1 seeder, 0 downloaders, 0 incomplete. Sent by the
/// capture scrape endpoint to satisfy clients that scrape before announcing.
pub const MINIMAL_SCRAPE_RESPONSE: &[u8] = b"d5:filesd8:completei1e10:downloadedi0e10:incompletei0eee";

// RFC 4648 base32 alphabet (used by magnet `urn:btih=` decoding)
pub const BASE32_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

// Magnet URI
pub const MAGNET_PREFIX: &str = "magnet:?";
pub const MAGNET_BTIH_PREFIX: &str = "urn:btih:";
pub const MAGNET_XT: &str = "xt";
pub const MAGNET_TR: &str = "tr";
pub const MAGNET_DN: &str = "dn";
pub const MAGNET_XL: &str = "xl";

// Percent-encoding (RFC 3986 unreserved set)

/// Percent-encode a raw byte slice for use in a URL query parameter.
///
/// Only unreserved chars (RFC 3986: `A-Za-z0-9-._~`) are left bare; every other
/// byte becomes `%XX` with uppercase hex. This matches what real BitTorrent
/// clients produce - using `+` for non-unreserved bytes breaks some trackers.
pub fn percent_encode_raw(bytes: &[u8]) -> String {
 let mut out = String::with_capacity(bytes.len() * 3);
 for &b in bytes {
 if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
 out.push(b as char);
 } else {
 out.push_str(&format!("%{:02X}", b));
 }
 }
 out
}

/// Percent-decode a percent-encoded string into raw bytes. The inverse of
/// [`percent_encode_raw`]. Centralized here so both capture and magnet
/// parsing share one decoder.
pub fn percent_decode_raw(s: &str) -> Vec<u8> {
 percent_encoding::percent_decode_str(s).collect()
}

/// Percent-decode into a lossy UTF-8 string (for human-readable values like
/// magnet `dn` display names).
pub fn percent_decode_str(s: &str) -> String {
 percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string()
}
