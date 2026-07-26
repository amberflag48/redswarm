//! Magnet link parser - extracts info_hash, tracker URL, name, and size.
//!
//! Magnet format: `magnet:?xt=urn:btih:<hash>&dn=<name>&tr=<url>&xl=<size>`
//! The info hash can be 40 hex chars or 32 base32 chars (both → 20 bytes).

use crate::data::protocol;

#[derive(Debug)]
pub struct MagnetMeta {
    pub info_hash: [u8; protocol::INFO_HASH_LEN],
    pub announce_url: String,
    pub name: String,
    pub total_size: u64,
}

pub fn parse(uri: &str) -> Result<MagnetMeta, String> {
    let query = uri
        .strip_prefix(protocol::MAGNET_PREFIX)
        .ok_or(format!("not a magnet link (must start with '{}')", protocol::MAGNET_PREFIX))?;

    let mut info_hash: Option<[u8; protocol::INFO_HASH_LEN]> = None;
    let mut announce_url: Option<String> = None;
    let mut name = String::new();
    let mut total_size: u64 = 0;

    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        // In magnet links, '+' means space (same as URL query encoding)
        let value = value.replace('+', " ");
        let value = protocol::percent_decode_str(&value);

        match key {
            protocol::MAGNET_XT => {
                let hash_str = value
                    .strip_prefix(protocol::MAGNET_BTIH_PREFIX)
                    .ok_or(format!("unsupported xt type (expected {})", protocol::MAGNET_BTIH_PREFIX))?;
                info_hash = Some(decode_info_hash(hash_str)?);
            }
            protocol::MAGNET_TR => {
                // Use the first tracker URL
                if announce_url.is_none() {
                    announce_url = Some(value);
                }
            }
            protocol::MAGNET_DN => name = value,
            protocol::MAGNET_XL => {
                total_size = value.parse().unwrap_or(0);
            }
            _ => {}
        }
    }

    let info_hash = info_hash.ok_or(format!("magnet missing {} (info hash)", protocol::MAGNET_XT))?;
    let announce_url = announce_url.ok_or(format!("magnet missing {} (tracker URL)", protocol::MAGNET_TR))?;
    if total_size == 0 {
        return Err(format!(
            "magnet missing {} (exact length) - the tracker needs the torrent size. \
             Drop the .torrent file instead (magnet links from private trackers rarely include size).",
            protocol::MAGNET_XL
        ));
    }

    Ok(MagnetMeta {
        info_hash,
        announce_url,
        name,
        total_size,
    })
}

/// Decode a 40-char hex string or 32-char base32 string into 20 bytes.
fn decode_info_hash(s: &str) -> Result<[u8; protocol::INFO_HASH_LEN], String> {
    match s.len() {
        n if n == protocol::INFO_HASH_HEX_LEN => crate::bencode::hex_decode_20(s),
        n if n == protocol::INFO_HASH_BASE32_LEN => decode_base32(s),
        n => Err(format!("info hash must be {} hex or {} base32 chars, got {n}", protocol::INFO_HASH_HEX_LEN, protocol::INFO_HASH_BASE32_LEN)),
    }
}

/// RFC 4648 base32 (no padding) - decodes 32 chars into 20 bytes.
fn decode_base32(s: &str) -> Result<[u8; protocol::INFO_HASH_LEN], String> {
    let bytes = s.as_bytes();
    if bytes.len() != protocol::INFO_HASH_BASE32_LEN {
        return Err(format!("base32 hash must be {} chars, got {}", protocol::INFO_HASH_BASE32_LEN, bytes.len()));
    }
    let mut bits: u64 = 0;
    let mut nbits: u32 = 0;
    let mut out = [0u8; protocol::INFO_HASH_LEN];
    let mut oi = 0;
    for &c in bytes {
        let val = protocol::BASE32_ALPHABET
            .iter()
            .position(|&a| a == c.to_ascii_uppercase())
            .ok_or(format!("invalid base32 char: {c:?}"))? as u64;
        bits = (bits << 5) | val;
        nbits += 5;
        while nbits >= 8 && oi < protocol::INFO_HASH_LEN {
            nbits -= 8;
            out[oi] = (bits >> nbits) as u8;
            oi += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a magnet URI with the canonical sample info_hash and an optional
    /// `&`-prefixed suffix (e.g. `&tr=...&dn=...`), so test fixtures share one
    /// source of truth (`data::fixtures::SAMPLE_INFO_HASH`). Always includes
    /// `&xl=1073741824` so the size check passes.
    fn sample_magnet(suffix: &str) -> String {
        format!(
            "{prefix}xt={btih}{hash}&xl=1073741824{suffix}",
            prefix = crate::data::protocol::MAGNET_PREFIX,
            btih = crate::data::protocol::MAGNET_BTIH_PREFIX,
            hash = crate::data::fixtures::SAMPLE_INFO_HASH,
        )
    }

    #[test]
    fn not_a_magnet_link() {
        assert!(parse("http://example.com").is_err());
        assert!(parse("magnet:").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn missing_xt() {
        assert!(parse("magnet:?tr=http://tracker.com/announce").is_err());
    }

    #[test]
    fn missing_tr() {
        assert!(parse(&sample_magnet("")).is_err());
    }

    #[test]
    fn unsupported_xt_type() {
        assert!(parse("magnet:?xt=urn:sha1:abcdef&tr=http://t.com/a").is_err());
    }

    #[test]
    fn hash_wrong_length() {
        assert!(parse("magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789&tr=http://t.com/a").is_err());
    }

    #[test]
    fn invalid_hex_char() {
        assert!(parse("magnet:?xt=urn:btih:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz&tr=http://t.com/a").is_err());
    }

    #[test]
    fn invalid_base32_char() {
        assert!(parse("magnet:?xt=urn:btih:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA!!!!!!!!&tr=http://t.com/a").is_err());
    }

    #[test]
    fn truncated_hex_hash() {
        assert!(parse("magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef0&tr=http://t.com/a").is_err());
    }

    #[test]
    fn valid_hex_hash() {
        let m = sample_magnet("&tr=http://tracker.com/announce");
        let meta = parse(&m).unwrap();
        assert_eq!(meta.info_hash[0], 0xAB);
        assert_eq!(meta.announce_url, "http://tracker.com/announce");
    }

    #[test]
    fn valid_base32_hash() {
        // JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP decodes (RFC 4648 base32) to
        // "Hello!deadbeef" twice → 20 bytes.
        let m = "magnet:?xt=urn:btih:JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&tr=http://t.com/a&xl=1073741824";
        let meta = parse(m).unwrap();
        assert_eq!(
            crate::bencode::hex_encode(&meta.info_hash),
            "48656c6c6f21deadbeef48656c6c6f21deadbeef"
        );
    }

    #[test]
    fn url_encoded_tracker() {
        let m = sample_magnet("&tr=http%3A%2F%2Ftracker.com%2Fannounce%2Fpasskey");
        let meta = parse(&m).unwrap();
        assert_eq!(meta.announce_url, "http://tracker.com/announce/passkey");
    }

    #[test]
    fn name_and_size_parsed() {
        let m = sample_magnet("&tr=http://t.com/a&dn=Test+Torrent&xl=1073741824");
        let meta = parse(&m).unwrap();
        assert_eq!(meta.name, "Test Torrent");
        assert_eq!(meta.total_size, 1073741824);
    }

    #[test]
    fn invalid_size_rejected() {
        // xl=notanumber parses as 0 → rejected with a clear error.
        let m = sample_magnet("&tr=http://t.com/a&xl=notanumber");
        let err = parse(&m).unwrap_err();
        assert!(err.contains("xl"), "error should mention xl: {err}");
    }

    #[test]
    fn missing_size_rejected() {
        // No xl= at all → rejected. Private-tracker magnets rarely include xl=,
        // so the user must drop the .torrent file instead.
        let m = "magnet:?xt=urn:btih:".to_string()
            + crate::data::fixtures::SAMPLE_INFO_HASH
            + "&tr=http://t.com/a";
        let err = parse(&m).unwrap_err();
        assert!(err.contains("xl"), "error should mention xl: {err}");
        assert!(err.contains(".torrent"), "error should suggest .torrent file: {err}");
    }

    #[test]
    fn multiple_trackers_takes_first() {
        let m = sample_magnet("&tr=http://first.com/a&tr=http://second.com/a");
        let meta = parse(&m).unwrap();
        assert_eq!(meta.announce_url, "http://first.com/a");
    }
}
