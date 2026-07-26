//! NAT-PMP client - discovers the public IP + port from a NAT-PMP gateway
//! (e.g. ProtonVPN WireGuard's `10.2.0.1`).
//!
//! When `[nat] gateway_ip` is set in `config.toml`, [`resolve_and_maintain`]
//! queries the gateway at startup for:
//!   1. The public (external) IP address - [`natpmp::external_address`].
//!   2. A pair of UDP+TCP port mappings - [`natpmp::port_mapping`] with the
//!      internal port set to `[tracker] peer_port` and the external port
//!      chosen by the gateway.
//!
//! Per RFC 6886, the gateway translates inbound traffic on the public port
//! to the internal port. So the peer-wire server listens on the INTERNAL
//! port (`[tracker] peer_port`), and the PUBLIC port is what gets announced
//! to the tracker (peers connect to the public port, gateway translates to
//! internal). The returned [`NatMapping`] (`{ public_ip, public_port }`)
//! carries the public port for announcing. A background task refreshes the
//! 60-second lease every 45 seconds so the mapping doesn't expire.

use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroU16;
use std::time::Duration;

use crab_nat::{InternetProtocol, PortMappingOptions};
use crab_nat::natpmp;
use tokio_util::sync::CancellationToken;

/// The public endpoint resolved via NAT-PMP - what the tracker and peers
/// should connect to. When NAT-PMP is disabled, this is `None` and the app
/// falls back to `[tracker] peer_port` (and the tracker infers the IP from
/// the TCP source).
#[derive(Debug, Clone)]
pub struct NatMapping {
    /// The gateway's public (external) IPv4 address.
    pub public_ip: Ipv4Addr,
    /// The public port - announced to the tracker. The gateway translates
    /// inbound traffic on this port to the internal port (`[tracker]
    /// peer_port`), which is where the peer-wire server listens.
    pub public_port: u16,
    /// Cancelling this token stops the background lease-renew task. The
    /// hot-reloader cancels the old mapping's token before resolving a new
    /// one (e.g. when `[nat] gateway_ip` changes), so the old lease stops
    /// being renewed and the renew task exits cleanly.
    pub cancel: CancellationToken,
}

/// Query the NAT-PMP gateway for the public IP and a port mapping, then
/// spawn a background task to refresh the lease.
///
/// `internal_port` is the local port the peer-wire server listens on (from
/// `[tracker] peer_port`). The gateway chooses the public port and
/// translates inbound traffic on it back to `internal_port` (RFC 6886 §1).
/// The returned `public_port` is what gets announced to the tracker.
///
/// `lease_lifetime_secs` and `renew_interval_secs` come from `[nat]` in
/// `config.toml` - see `NatConfig::validate` for the invariants.
///
/// # Errors
///
/// Returns `Err` if the gateway is unreachable, doesn't support NAT-PMP, or
/// rejects the mapping request. The caller should fall back to the local
/// `peer_port` in that case.
pub async fn resolve_and_maintain(
    gateway: IpAddr,
    internal_port: u16,
    lease_lifetime_secs: u32,
    renew_interval_secs: u64,
) -> anyhow::Result<NatMapping> {
    let internal = NonZeroU16::new(internal_port)
        .ok_or_else(|| anyhow::anyhow!("NAT-PMP internal port must be > 0"))?;

    // 1. Get the public IP.
    let public_ip = natpmp::external_address(gateway, None)
        .await
        .map_err(|e| anyhow::anyhow!("NAT-PMP external_address failed: {e:?}"))?;

    // 2. Request UDP + TCP mappings (gateway chooses the public port).
    let opts = PortMappingOptions {
        external_port: None,
        lifetime_seconds: Some(lease_lifetime_secs),
        timeout_config: None,
    };
    let mut udp = natpmp::port_mapping(gateway, InternetProtocol::Udp, internal, opts)
        .await
        .map_err(|e| anyhow::anyhow!("NAT-PMP UDP port_mapping failed: {e:?}"))?;
    let mut tcp = natpmp::port_mapping(gateway, InternetProtocol::Tcp, internal, opts)
        .await
        .map_err(|e| anyhow::anyhow!("NAT-PMP TCP port_mapping failed: {e:?}"))?;

    let public_port = udp.external_port().get();
    let tcp_port = tcp.external_port().get();
    if public_port != tcp_port {
        tracing::warn!(
            udp_port = public_port,
            tcp_port,
            "NAT-PMP gateway returned different public ports for UDP and TCP - using UDP port"
        );
    }

    tracing::info!(
        %public_ip,
        public_port,
        internal_port,
        lifetime_secs = lease_lifetime_secs,
        "NAT-PMP mapping established - gateway translates public port to internal port"
    );

    // 3. Spawn a background task to refresh the lease before it expires.
    //    The task observes `cancel` so the hot-reloader can stop it cleanly
    //    when `[nat] gateway_ip` changes (before establishing a new mapping).
    let cancel = CancellationToken::new();
    let renew_cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = renew_cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(renew_interval_secs)) => {}
            }
            match udp.renew().await {
                Ok(()) => tracing::debug!("NAT-PMP UDP lease renewed"),
                Err(e) => tracing::warn!(error = ?e, "NAT-PMP UDP renewal failed"),
            }
            match tcp.renew().await {
                Ok(()) => tracing::debug!("NAT-PMP TCP lease renewed"),
                Err(e) => tracing::warn!(error = ?e, "NAT-PMP TCP renewal failed"),
            }
        }
    });

    Ok(NatMapping { public_ip, public_port, cancel })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nat_mapping_is_clone() {
        let m = NatMapping {
            public_ip: Ipv4Addr::new(146, 70, 198, 39),
            public_port: 42351,
            cancel: CancellationToken::new(),
        };
        let copied = m.clone();
        assert_eq!(copied.public_port, m.public_port);
        assert_eq!(copied.public_ip, m.public_ip);
    }

    #[test]
    fn nat_mapping_holds_public_values() {
        let m = NatMapping {
            public_ip: Ipv4Addr::new(1, 2, 3, 4),
            public_port: 12345,
            cancel: CancellationToken::new(),
        };
        assert_eq!(m.public_ip, Ipv4Addr::new(1, 2, 3, 4));
        assert_eq!(m.public_port, 12345);
    }

    #[test]
    fn nat_mapping_cancel_stops_is_cancelled() {
        let m = NatMapping {
            public_ip: Ipv4Addr::new(1, 2, 3, 4),
            public_port: 12345,
            cancel: CancellationToken::new(),
        };
        assert!(!m.cancel.is_cancelled());
        m.cancel.cancel();
        assert!(m.cancel.is_cancelled());
    }

    #[test]
    fn nonzero_u16_rejects_zero() {
        assert!(NonZeroU16::new(0).is_none());
        assert!(NonZeroU16::new(1).is_some());
        assert!(NonZeroU16::new(6881).is_some());
        assert!(NonZeroU16::new(u16::MAX).is_some());
    }
}
