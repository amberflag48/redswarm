//! Client emulation - peer_id generation and label lookup.
//!
//! All client specification data (peer_id prefixes, User-Agents, query
//! templates, numwant, aliases) now lives in `config.toml` under
//! `[[clients]]` and is surfaced via [`crate::config::ClientSpecConfig`].

use rand::Rng;

use crate::config::KeyFormat;

/// Generate a 20-byte peer_id: the client prefix + random alphanumeric bytes.
pub fn generate_peer_id(prefix: &str) -> [u8; crate::data::protocol::PEER_ID_LEN] {
    let mut id = [0u8; crate::data::protocol::PEER_ID_LEN];
    let pfx = prefix.as_bytes();
    let pfx_len = pfx.len().min(crate::data::protocol::PEER_ID_LEN);
    id[..pfx_len].copy_from_slice(&pfx[..pfx_len]);
    let charset = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    for b in &mut id[pfx_len..] {
        *b = charset[rng.random_range(0..charset.len())];
    }
    id
}

/// Generate a random per-session key in the given format.
pub fn generate_key(format: KeyFormat) -> String {
    format.generate()
}

/// Resolve a stored `peer_id_prefix` or alias back to an index into the
/// `clients` slice. The `peer_id_prefix` is the unique identity key - it
/// is matched exactly (case-sensitive, since it's a protocol identifier).
/// Aliases are matched case-insensitively as a convenience fallback.
pub fn find_by_client(
    clients: &[crate::config::ClientSpecConfig],
    key: &str,
) -> Option<usize> {
    clients.iter().position(|c| {
        c.peer_id_prefix == key
            || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ClientSpecConfig, KeyFormat};

    fn make_client(label: &str, version: &str, prefix: &str, aliases: &[&str]) -> ClientSpecConfig {
        ClientSpecConfig {
            label: label.to_string(),
            version: version.to_string(),
            peer_id_prefix: prefix.to_string(),
            user_agent: "TestClient/1.0".to_string(),
            query: "info_hash={info_hash}&peer_id={peer_id}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&compact=1&key={key}{event}".to_string(),
            numwant: 50,
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            reserved_bytes: "0000000000100005".to_string(),
            fast_extension: true,
            keepalive_secs: 90,
            v_string: "TestClient/1.0".to_string(),
            m_dict: std::collections::BTreeMap::new(),
            reqq: Some(500),
            encryption_preferred: None,
            send_upload_only: true,
            send_complete_ago: None,
            send_yourip: true,
            key_format: KeyFormat::UpperHex,
        }
    }

    #[test]
    fn peer_id_is_20_bytes() {
        assert_eq!(generate_peer_id("-qB5220-").len(), 20);
    }

    #[test]
    fn peer_id_starts_with_prefix() {
        let prefix = "-qB5220-";
        let id = generate_peer_id(prefix);
        assert_eq!(&id[..prefix.len()], prefix.as_bytes());
    }

    #[test]
    fn peer_id_random_part_is_alphanumeric() {
        let prefix = "-qB5220-";
        let id = generate_peer_id(prefix);
        for &b in &id[prefix.len()..] {
            assert!(b.is_ascii_alphanumeric(), "non-alphanumeric byte: {:#x}", b);
        }
    }

    #[test]
    fn peer_ids_are_unique() {
        assert_ne!(generate_peer_id("-qB5220-"), generate_peer_id("-qB5220-"));
    }

    #[test]
    fn peer_id_empty_prefix_fills_all_random() {
        let id = generate_peer_id("");
        for &b in &id[..] {
            assert!(b.is_ascii_alphanumeric());
        }
    }

    #[test]
    fn peer_id_long_prefix_truncated() {
        let long = "ThisPrefixIsWayTooLongForAPeerId";
        let id = generate_peer_id(long);
        assert_eq!(&id[..20], &long.as_bytes()[..20]);
    }

    #[test]
    fn key_upper_hex_is_8_digits() {
        let key = generate_key(KeyFormat::UpperHex);
        assert_eq!(key.len(), 8);
        for c in key.chars() {
            assert!(c.is_ascii_digit() || c.is_uppercase(), "char '{c}' not uppercase hex");
        }
    }

    #[test]
    fn key_lower_hex_is_8_lowercase_digits() {
        let key = generate_key(KeyFormat::LowerHex);
        assert_eq!(key.len(), 8);
        for c in key.chars() {
            assert!(c.is_ascii_digit() || c.is_lowercase(), "char '{c}' not lowercase hex");
        }
    }

    #[test]
    fn key_decimal_is_numeric() {
        let key = generate_key(KeyFormat::Decimal);
        for c in key.chars() {
            assert!(c.is_ascii_digit(), "char '{c}' not decimal");
        }
    }

    #[test]
    fn keys_are_unique() {
        assert_ne!(generate_key(KeyFormat::UpperHex), generate_key(KeyFormat::UpperHex));
    }

    #[test]
    fn find_by_client_matches_peer_id_prefix() {
        let clients = vec![
            make_client("qBittorrent", "5.2.2", "-qB5220-", &["Qbittorrent"]),
            make_client("Transmission", "4.1.2", "-TR4120-", &["Transmission"]),
        ];
        assert_eq!(find_by_client(&clients, "-qB5220-"), Some(0));
        assert_eq!(find_by_client(&clients, "-TR4120-"), Some(1));
    }

    #[test]
    fn find_by_client_matches_alias() {
        let clients = vec![make_client("qBittorrent", "5.2.2", "-qB5220-", &["Qbittorrent", "qbittorrent"])];
        assert_eq!(find_by_client(&clients, "Qbittorrent"), Some(0));
        assert_eq!(find_by_client(&clients, "qbittorrent"), Some(0));
    }

    #[test]
    fn find_by_client_returns_none_for_unknown() {
        let clients = vec![make_client("Deluge", "2.2.0", "-DE220s-", &["Deluge"])];
        assert_eq!(find_by_client(&clients, "-XX0000-"), None);
        assert_eq!(find_by_client(&clients, "Unknown"), None);
        assert_eq!(find_by_client(&clients, ""), None);
    }

    #[test]
    fn find_by_client_returns_none_for_empty_clients() {
        assert_eq!(find_by_client(&[], "-qB5220-"), None);
    }
}
