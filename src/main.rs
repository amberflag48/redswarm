//! RedSwarm - a hands-free, sophisticated P2P ratio cheater for private BitTorrent trackers.
//!
//! Drop a torrent or paste a magnet; the tool probes all client emulations
//! until one passes the tracker's whitelist, then keeps attacking with it.
//!
//! Hot reload: `config.toml` is watched at runtime and re-applied atomically
//! via [`crate::reload`] + [`crate::watcher`]. Running audits are frozen on
//! the config/pool/peer-server they captured at start; per-request handlers
//! and new audits always read the latest values.

mod announce;
mod api;
mod bencode;
mod capture;
mod config;
mod data;
mod db;
mod engine;
mod magnet;
mod nat;
mod peer_id;
mod peer_server;
mod render;
mod reload;
mod singleton;
mod swarm;
mod templates;
mod torrent;
mod watcher;

use std::sync::Arc;
use std::time::Duration;

use tracing_subscriber::{fmt, prelude::*};

use crate::data::vocab;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
 let app_config = config::load()?;

 // Reloadable tracing env filter. `server.log_filter` changes are applied
 // live by the hot-reloader via `state.log_reload`.
 let base_filter = tracing_subscriber::EnvFilter::try_from_default_env()
 .unwrap_or_else(|_| {
 app_config
 .server
 .log_filter
 .clone()
 .parse::<tracing_subscriber::EnvFilter>()
 .unwrap_or_else(|_| "info".parse().expect("\"info\" is a valid EnvFilter"))
 });
 let (filter, log_handle) = tracing_subscriber::reload::Layer::new(base_filter);
 let log_reload: Box<dyn Fn(&str) + Send + Sync> = Box::new(move |s: &str| {
 if let Ok(f) = s.parse::<tracing_subscriber::EnvFilter>() {
 let _ = log_handle.modify(|cur| *cur = f);
 }
 });
 tracing_subscriber::registry()
 .with(filter)
 .with(fmt::layer())
 .init();

 // Take over from any prior instance still holding the ports, so this
 // process can bind cleanly (no "Address already in use"). Best-effort; if
 // no other instance is running this is a fast /proc scan that finds nothing.
 singleton::take_over();

 let db_url =
 std::env::var("REDSWARM_DB").unwrap_or_else(|_| app_config.server.db_url.clone());

 let pool = db::connect(&db_url, app_config.database.max_connections).await?;
 tracing::info!("database ready: {db_url}");

 let (events_tx, _) =
 tokio::sync::broadcast::channel::<engine::AppEvent>(crate::config::BROADCAST_CHANNEL_CAPACITY);

 // The peer-wire server always binds on the internal port ([tracker]
 // peer_port). Per RFC 6886, the NAT-PMP gateway translates inbound traffic
 // on the public port to this internal port. The advertised port (what the
 // tracker tells peers) is the NAT public port when NAT is active, else the
 // internal port - computed at consumption time (start_engine / capture),
 // NOT by mutating the stored config, so a hot reload of config.toml
 // (which holds the internal port) never clobbers the NAT override.
 let internal_port = app_config.tracker.peer_port;

 // Start the peer-wire server (one global listener shared by all audits).
 let capture_store = capture::CaptureStore::new(events_tx.clone());
 let peer_server = peer_server::PeerServer::start(
 format!("{}:{}", std::net::Ipv4Addr::UNSPECIFIED, internal_port),
 &app_config.peer_server,
 capture_store.clone(),
 )
 .unwrap_or_else(|e| {
 tracing::warn!(error = %e, "peer server failed to start - continuing without connectability");
 peer_server::PeerServer::disabled(capture_store.clone())
 });

 // Resolve NAT-PMP mapping if a gateway is configured. The gateway chooses
 // a public port and translates inbound traffic on it to the internal port
 // (RFC 6886). The peer-wire listener stays on the internal port; only the
 // advertised port (nat.public_port) is used by the engine/capture. On
 // failure, the app continues with the local port (non-connectable from WAN).
 let nat_mapping = if !app_config.nat.gateway_ip.trim().is_empty() {
 let gateway: std::net::IpAddr = app_config
 .nat
 .gateway_ip
 .trim()
 .parse()
 .expect("config validation guarantees a valid IP");
 match nat::resolve_and_maintain(
 gateway,
 internal_port,
 app_config.nat.lease_lifetime_secs,
 app_config.nat.renew_interval_secs,
 )
 .await
 {
 Ok(m) => {
 tracing::info!(
 public_ip = %m.public_ip,
 public_port = m.public_port,
 internal_port,
 "NAT-PMP active - announcing public port to tracker"
 );
 Some(m)
 }
 Err(e) => {
 tracing::error!(
 error = %e,
 "NAT-PMP resolve failed - continuing with local peer_port (non-connectable from WAN)"
 );
 None
 }
 }
 } else {
 None
 };

 let state = Arc::new(api::AppState {
 pool: arc_swap::ArcSwap::from_pointee(pool),
 running: Default::default(),
 config: arc_swap::ArcSwap::from_pointee(app_config.clone()),
 events_tx,
 peer_server: arc_swap::ArcSwap::from_pointee(peer_server),
 capture_store,
 nat: arc_swap::ArcSwapOption::new(nat_mapping.map(Arc::new)),
 log_reload,
 rebind_notify: Arc::new(tokio::sync::Notify::new()),
 });

 // Auto-restart tasks that were running before the process stopped/crashed.
 // Their status is still "running" in the DB, and their peer state was saved
 // at the last stat tick - enough to resume.
 {
 let pool = state.pool.load_full();
 let running = db::list_audits(&pool)
 .await
 .unwrap_or_default()
 .into_iter()
 .filter(|r| r.status == vocab::STATUS_RUNNING)
 .collect::<Vec<_>>();
 for row in &running {
 tracing::info!(id = row.id, name = %row.name, "auto-restarting audit on boot");
 match api::start_engine(&state, row.id).await {
 Ok(true) => {}
 Ok(false) => tracing::warn!(id = row.id, "auto-restart: already running"),
 Err(e) => tracing::warn!(id = row.id, error = ?e, "auto-restart failed"),
 }
 }
 if !running.is_empty() {
 tracing::info!("auto-restarted {} audit(s)", running.len());
 }
 }

 // Watch config.toml for hot reload. Best-effort; failures are logged and
 // never crash the app (the last good config is kept).
 watcher::spawn(state.clone());

 // HTTP server with a graceful-rebind loop. On `server.bind_addr` change,
 // the reloader fires `state.rebind_notify`; axum drains in-flight requests,
 // returns, and the loop rebinds to the current config's bind_addr. The env
 // override `REDSWARM_ADDR` (if set) wins permanently and suppresses
 // runtime bind_addr changes. On a bind failure, fall back to the last
 // known-good address so the dashboard stays up.
 //
 // Dual-stack: when binding `0.0.0.0:PORT` (IPv4 wildcard), also bind
 // `[::]:PORT` with IPV6_V6ONLY=true (IPv6-only) so both protocols are
 // served. `localhost` resolves to `::1` first on most systems; without
 // this, IPv6 clients get connection-refused and fall back to IPv4 after
 // a ~300ms delay. The IPv6 socket is V6ONLY so it doesn't conflict with
 // the IPv4 wildcard listener.
 let mut last_good: Option<String> = None;
 loop {
 let bind_addr =
 std::env::var("REDSWARM_ADDR").unwrap_or_else(|_| state.config.load().server.bind_addr.clone());
 let app = api::router(state.clone());
 let listeners = match bind_dual_stack(&bind_addr).await {
 Ok(ls) => {
 last_good = Some(bind_addr.clone());
 ls
 }
 Err(e) => {
 tracing::error!(error = %e, addr = %bind_addr, "bind failed");
 match &last_good {
 Some(g) => match bind_dual_stack(g).await {
 Ok(ls) => ls,
 Err(e2) => {
 tracing::error!(error = %e2, "fallback bind failed - retrying");
 tokio::time::sleep(Duration::from_secs(
 state.config.load().server.rebind_retry_secs,
 ))
 .await;
 continue;
 }
 },
 None => {
 tokio::time::sleep(Duration::from_secs(
 state.config.load().server.rebind_retry_secs,
 ))
 .await;
 continue;
 }
 }
 }
 };
 tracing::info!("listening on http://{bind_addr}");
 // Serve the same app on all listeners (IPv4 + IPv6). Each clone of the
 // router is consumed by its `axum::serve` call. On rebind signal, hard-
 // abort all server tasks: this drops the listeners (ports release
 // immediately) and all in-flight connections. The browser's
 // `EventSource` auto-reconnects on its built-in retry. We use a hard
 // abort instead of `with_graceful_shutdown` because the latter waits
 // for ALL connections to drain - but the global SSE stream
 // (`GET /api/events`) is long-lived and never closes on its own, so a
 // graceful drain would block forever. Rebinds are rare (only on
 // `server.bind_addr` change), so dropping in-flight requests is fine.
 let server_tasks: Vec<_> = listeners
 .into_iter()
 .map(|l| {
 let app = app.clone();
 tokio::spawn(async move { axum::serve(l, app).await })
 })
 .collect();
 state.rebind_notify.notified().await;
 tracing::info!("rebind signalled - stopping HTTP server to rebind");
 for task in &server_tasks {
 task.abort();
 }
 for task in server_tasks {
 let _ = task.await;
 }
 }
}

/// Bind the primary address. If it's `0.0.0.0:PORT` (IPv4 wildcard), also
/// bind `[::]:PORT` with `IPV6_V6ONLY=true` (IPv6-only) so both protocols
/// are served. Returns all successfully bound listeners; the primary bind
/// failure is propagated as `Err` (caller handles fallback).
async fn bind_dual_stack(bind_addr: &str) -> std::io::Result<Vec<tokio::net::TcpListener>> {
 let primary = tokio::net::TcpListener::bind(bind_addr).await?;
 // Capture the actually-bound port before `primary` is moved into the vec.
 let primary_port = primary.local_addr().map(|a| a.port());
 let mut listeners = vec![primary];

 // If the primary is IPv4 wildcard (0.0.0.0:PORT), also bind an IPv6-only
 // listener on [::]:PORT so `localhost` (which resolves to ::1 first on
 // most systems) connects instantly without IPv4 fallback delay.
 if let Ok(addr) = bind_addr.parse::<std::net::SocketAddr>()
 && addr.ip().is_unspecified() && addr.is_ipv4()
 {
 // Use the primary's actually-bound port, not the requested one, so a
 // wildcard bind on port 0 (ephemeral) dual-stacks onto the SAME port on
 // both protocols - otherwise the IPv4 and IPv6 listeners end up on two
 // different ephemeral ports. For a fixed port this is a no-op (the
 // bound port equals the requested port).
 let v4_port = primary_port.unwrap_or(addr.port());
 let v6 = std::net::SocketAddr::new(
 std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
 v4_port,
 );
 match bind_ipv6_only(v6) {
 Ok(l) => {
 tracing::info!("also listening on http://[::]:{}", v4_port);
 listeners.push(l);
 }
 Err(e) => {
 // IPv6 unavailable or port in use - not fatal; IPv4 works.
 tracing::warn!(error = %e, "IPv6 bind failed - IPv4 only");
 }
 }
 }

 Ok(listeners)
}

/// Bind an IPv6-only listener (`IPV6_V6ONLY=1`) so it doesn't conflict with
/// the IPv4 wildcard listener on dual-stack systems. Uses `socket2` to set
/// the socket option before bind - tokio's `TcpListener::bind` doesn't
/// expose `IPV6_V6ONLY` and the system default varies.
fn bind_ipv6_only(addr: std::net::SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
 use socket2::{Domain, Socket, Type};
 let socket = Socket::new(Domain::IPV6, Type::STREAM, None)?;
 socket.set_only_v6(true)?; // IPV6_V6ONLY=1 - IPv6 only, no IPv4-mapped
 socket.set_reuse_address(true)?;
 socket.bind(&addr.into())?;
 socket.listen(128)?;
 let std_listener: std::net::TcpListener = socket.into();
 std_listener.set_nonblocking(true)?;
 tokio::net::TcpListener::from_std(std_listener)
}

#[cfg(test)]
mod tests {
 use super::{bind_dual_stack, bind_ipv6_only};

 // bind_dual_stack: the dual-stack path

 #[tokio::test]
 async fn bind_dual_stack_ipv4_wildcard_binds_v4_and_optionally_v6() {
 // 0.0.0.0:0 → the IPv4 primary binds (ephemeral port); when IPv6 is
 // available a second V6ONLY listener is added. Sandboxes without IPv6
 // get just the one IPv4 listener, so accept either.
 let listeners = bind_dual_stack("0.0.0.0:0").await.expect("bind 0.0.0.0:0");
 assert!(!listeners.is_empty(), "at least the IPv4 primary must bind");
 let v4 = listeners[0].local_addr().expect("primary local_addr");
 assert!(v4.is_ipv4(), "primary listener is IPv4");
 assert!(v4.port() > 0, "port 0 must resolve to an ephemeral port");
 if listeners.len() > 1 {
 let v6 = listeners[1].local_addr().expect("v6 local_addr");
 assert!(v6.is_ipv6(), "second listener must be IPv6");
 // The IPv6 listener must share the IPv4 primary's port (dual-stack
 // on the SAME port). Before the fix, an ephemeral (port 0) wildcard
 // bind put the two listeners on two different ephemeral ports
 // because the IPv6 bind reused the requested port (0) instead of
 // the primary's assigned port.
 assert_eq!(v6.port(), v4.port(), "v4 and v6 must share the same port");
 }
 }

 #[tokio::test]
 async fn bind_dual_stack_loopback_ipv4_is_single_stack() {
 // 127.0.0.1 is not an unspecified wildcard, so no IPv6 dual-stack.
 let listeners = bind_dual_stack("127.0.0.1:0").await.expect("bind 127.0.0.1:0");
 assert_eq!(listeners.len(), 1, "loopback IPv4 must not add an IPv6 listener");
 let addr = listeners[0].local_addr().expect("local_addr");
 assert!(addr.is_ipv4());
 assert_eq!(addr.ip(), std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
 }

 #[tokio::test]
 async fn bind_dual_stack_ipv6_loopback_is_single_stack() {
 // [::1] is IPv6 and not unspecified, so the dual-stack branch is skipped.
 let listeners = bind_dual_stack("[::1]:0").await.expect("bind [::1]:0");
 assert_eq!(listeners.len(), 1);
 let addr = listeners[0].local_addr().expect("local_addr");
 assert!(addr.is_ipv6());
 assert_eq!(addr.ip(), std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
 }

 #[tokio::test]
 async fn bind_dual_stack_unparseable_address_returns_err() {
 assert!(bind_dual_stack("not-a-socket-addr").await.is_err());
 }

 #[tokio::test]
 async fn bind_dual_stack_port_in_use_returns_err() {
 // Hold a listener on an ephemeral port, then try to dual-stack-bind
 // the same address - the primary bind must fail (propagated as Err).
 let holder = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind holder");
 let port = holder.local_addr().expect("local_addr").port();
 let addr = format!("127.0.0.1:{port}");
 assert!(bind_dual_stack(&addr).await.is_err(), "binding an in-use port must error");
 drop(holder);
 }

 // bind_ipv6_only

 #[tokio::test]
 async fn bind_ipv6_only_binds_ipv6_loopback() {
 let addr = std::net::SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), 0);
 match bind_ipv6_only(addr) {
 Ok(l) => {
 let local = l.local_addr().expect("local_addr");
 assert!(local.is_ipv6(), "V6ONLY listener is IPv6");
 assert_eq!(local.ip(), std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
 assert!(local.port() > 0, "ephemeral port assigned");
 }
 Err(e) => {
 // IPv6 unavailable in this environment - skip rather than fail.
 eprintln!("bind_ipv6_only skipped (IPv6 unavailable): {e}");
 }
 }
 }
}
