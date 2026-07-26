//! Hot-reload of `config.toml` at runtime.
//!
//! [`reload_config`] re-reads `config.toml`, validates it, atomically swaps
//! the whole [`AppConfig`] into [`AppState`](crate::api::AppState), then
//! re-applies any structural settings that readers alone can't pick up:
//!
//! | Setting | Action |
//! |---|---|
//! | `server.log_filter` | Reloads the `tracing` env filter in place. |
//! | `server.db_url` / `database.max_connections` | Re-creates the SQLite pool. |
//! | `peer_server.*` / `tracker.peer_port` | Restarts the peer-wire server. |
//! | `nat.gateway_ip` (or `peer_port` while NAT active) | Re-resolves NAT-PMP. |
//! | `server.bind_addr` | Signals the HTTP server to gracefully rebind. |
//!
//! Running audits are **unaffected**: they hold frozen `Arc` snapshots of the
//! config, pool, and peer server from their [`start_engine`](crate::api::start_engine)
//! time, so a swap is invisible to them. Only per-request handlers and
//! newly-started audits see the new values immediately - the "new audits only"
//! policy. Swapped-out subsystems (old pool / peer server) stay alive until
//! the last running audit referencing them ends, then drop cleanly (the peer
//! server's `Drop` cancels its accept loop; the pool's last `Arc` closes it).
//!
//! On any load/validate failure the old config is kept (no partial swap) and
//! `Err` is returned. Structural re-applies are best-effort: a failure (e.g.
//! the new `peer_port` is already in use) is logged and the old subsystem is
//! retained; the config is still swapped so readers see the user's intent and
//! the change applies on the next successful reload or a restart.

use std::net::IpAddr;
use std::sync::Arc;

use crate::api::SharedState;
use crate::config::{self, AppConfig};
use crate::engine::AppEvent;
use crate::nat;
use crate::peer_server::PeerServer;

/// Re-load `config.toml` (path from [`config::path`]) and re-apply. Entry
/// point for the file watcher.
pub async fn reload_config(state: &SharedState) -> anyhow::Result<()> {
    reload_config_from(state, &config::path()).await
}

/// Re-load config from an explicit path, validate, atomically swap the
/// config, and re-apply structural changes. Broadcasts a `config_reloaded`
/// SSE event on success. Exposed for tests so they don't race on the
/// `REDSWARM_CONFIG` env var.
pub async fn reload_config_from(state: &SharedState, path: &str) -> anyhow::Result<()> {
    let new = match config::load_from_path(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, path, "config reload rejected - keeping current config");
            return Err(e);
        }
    };
    let old = state.config.load_full();

    // No-op guard: if the file content is byte-identical to the live config
    // (e.g. the settings UI wrote the same values, or an editor saved without
    // changes), skip the swap + structural re-apply + SSE broadcast. This also
    // prevents a double-reload toast when the settings API writes the file and
    // then the file watcher fires on the same write.
    if new == *old {
        tracing::debug!(path, "config unchanged - skipping reload");
        return Ok(());
    }

    tracing::info!(path, "config reloaded - applying");

    // 1. Always swap the config - it is the source of truth; per-request
    //    handlers and new audits pick it up immediately.
    state.config.store(Arc::new(new.clone()));

    // 2. Log filter - reload the tracing env filter in place.
    if new.server.log_filter != old.server.log_filter {
        (state.log_reload)(&new.server.log_filter);
        tracing::info!(log_filter = %new.server.log_filter, "log filter reloaded");
    }

    // 3. Database pool - re-create if db_url or max_connections changed.
    if new.server.db_url != old.server.db_url
        || new.database.max_connections != old.database.max_connections
    {
        match crate::db::connect(&new.server.db_url, new.database.max_connections).await {
            Ok(new_pool) => {
                state.pool.store(Arc::new(new_pool));
                tracing::info!(
                    db_url = %new.server.db_url,
                    max_connections = new.database.max_connections,
                    "database pool re-created (old pool drops once running audits end)"
                );
            }
            Err(e) => {
                // Keep the old pool; the config is already swapped, so new
                // requests use the old pool until the next successful reload.
                tracing::error!(error = %e, "pool re-create failed - keeping old pool");
            }
        }
    }

    // 4. Peer-wire server - restart if peer_server.* or tracker.peer_port
    //    changed. Running audits keep their old `Arc<PeerServer>` (frozen);
    //    it auto-stops via `Drop` once they all end.
    if new.peer_server != old.peer_server || new.tracker.peer_port != old.tracker.peer_port {
        reapply_peer_server(state, &old, &new).await;
    }

    // 5. NAT-PMP - re-resolve if gateway_ip changed, or if peer_port changed
    //    while NAT is active (the mapping's internal port follows peer_port).
    let gateway_changed = new.nat.gateway_ip.trim() != old.nat.gateway_ip.trim();
    let nat_active = !new.nat.gateway_ip.trim().is_empty();
    let peer_port_changed = new.tracker.peer_port != old.tracker.peer_port;
    if gateway_changed || (nat_active && peer_port_changed) {
        reapply_nat(state, &new);
    }

    // 6. HTTP listener - signal a graceful rebind if bind_addr changed and no
    //    `REDSWARM_ADDR` env override is present (env wins at startup and
    //    permanently suppresses runtime bind_addr changes).
    if new.server.bind_addr != old.server.bind_addr
        && std::env::var("REDSWARM_ADDR").is_err()
    {
        tracing::info!(
            old = %old.server.bind_addr,
            new = %new.server.bind_addr,
            "bind_addr changed - signalling HTTP rebind"
        );
        state.rebind_notify.notify_one();
    }

    // 7. Broadcast to all SSE clients - carries the full new config so the UI
    //    can surgically update fields without a re-fetch.
    let _ = state.events_tx.send(AppEvent::ConfigReloaded {
        config: state.config.load_full(),
    });

    Ok(())
}

/// Start a fresh peer-wire server with the new config and swap it in.
///
/// **Same port, no running audits** - swap out the old (drops it, cancels the
/// accept loop, releases the port), poll-bind the new (the OS may take a
/// moment to release the port after the listener closes).
///
/// **Same port, running audits hold the old** - the port stays bound by their
/// frozen `Arc<PeerServer>` snapshots; we can't rebind. Defer (keep old) and
/// log; the change applies on the next reload with no running audits, or on
/// restart.
///
/// **Port changed** - old and new bind different ports, so both coexist. Bind
/// the new first; only swap on success. The old server stays on the old port
/// until running audits holding it end, then drops via `Drop`.
async fn reapply_peer_server(state: &SharedState, old: &AppConfig, new: &AppConfig) {
    let bind_addr = format!("{}:{}", std::net::Ipv4Addr::UNSPECIFIED, new.tracker.peer_port);
    let capture_store = state.capture_store.clone();
    let port_changed = new.tracker.peer_port != old.tracker.peer_port;

    if !port_changed {
        // Same port - two listeners can't coexist. Check if running audits
        // hold the old server (frozen); if so, the port stays bound.
        let running_count = state.running.read().await.len();
        if running_count > 0 {
            tracing::warn!(
                running_audits = running_count,
                "peer server restart deferred - running audits hold the port; stop them to apply"
            );
            return;
        }
        // No running audits - swap out + stop+await the old so the port is
        // released synchronously (the accept loop's TcpListener drops when
        // the task exits, which `stop().await` waits for).
        let old_ps = state
            .peer_server
            .swap(Arc::new(PeerServer::disabled(capture_store.clone())));
        old_ps.stop().await; // cancels + awaits accept loop → port releases

        // Bind the new server immediately - the port is now free.
        let new_ps = if new.peer_server.enabled {
            match PeerServer::start(bind_addr.clone(), &new.peer_server, capture_store.clone()) {
                Ok(ps) => Arc::new(ps),
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        bind_addr,
                        "peer server rebind failed after stopping old - \
                         new audits get a disabled server until the next reload"
                    );
                    return;
                }
            }
        } else {
            Arc::new(PeerServer::disabled(capture_store))
        };
        state.peer_server.store(new_ps);
        tracing::info!(bind_addr, "peer server restarted");
    } else {
        // Port changed - old and new coexist on different ports. Bind the new
        // first; only swap on success.
        let new_ps = if new.peer_server.enabled {
            match PeerServer::start(bind_addr.clone(), &new.peer_server, capture_store) {
                Ok(ps) => Arc::new(ps),
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        bind_addr,
                        "peer server bind failed on new port - keeping old server"
                    );
                    return;
                }
            }
        } else {
            Arc::new(PeerServer::disabled(capture_store))
        };
        state.peer_server.store(new_ps);
        tracing::info!(
            bind_addr,
            "peer server restarted on new port (old port released when running audits end)"
        );
    }
}

/// Cancel the old NAT-PMP lease (if any), then resolve a fresh mapping (or
/// disable NAT when `gateway_ip` is empty). The resolve is spawned in the
/// background so a slow/unreachable gateway doesn't block the reload loop -
/// subsequent config edits are processed immediately while the NAT-PMP query
/// runs in parallel. On resolve failure, NAT is disabled and logged - the app
/// continues without connectability from WAN.
fn reapply_nat(state: &SharedState, new: &AppConfig) {
    // Cancel the old lease-renew task so it stops refreshing the stale mapping.
    if let Some(old) = state.nat.load_full() {
        old.cancel.cancel();
    }

    if new.nat.gateway_ip.trim().is_empty() {
        state.nat.store(None);
        tracing::info!("NAT-PMP disabled (gateway_ip empty)");
        return;
    }

    // Clear the stale mapping immediately so readers don't use the old public
    // IP/port while the new one is being resolved.
    state.nat.store(None);

    // validate() guarantees gateway_ip parses when non-empty.
    let gateway: IpAddr = new.nat.gateway_ip.trim().parse().expect("validated");
    let internal_port = new.tracker.peer_port;
    let lease_lifetime_secs = new.nat.lease_lifetime_secs;
    let renew_interval_secs = new.nat.renew_interval_secs;
    let state_clone = Arc::clone(state);
    tokio::spawn(async move {
        match nat::resolve_and_maintain(
            gateway,
            internal_port,
            lease_lifetime_secs,
            renew_interval_secs,
        )
        .await
        {
            Ok(m) => {
                state_clone.nat.store(Some(Arc::new(m)));
                tracing::info!("NAT-PMP re-resolved");
            }
            Err(e) => {
                tracing::error!(error = %e, "NAT-PMP re-resolve failed - NAT disabled");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::AppState;
    use crate::config::test_helpers;
    use arc_swap::{ArcSwap, ArcSwapOption};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{broadcast, RwLock};

    async fn state_from(cfg: crate::config::AppConfig) -> SharedState {
        let pool = crate::db::connect("sqlite::memory:", 2).await.unwrap();
        let (events_tx, _) = broadcast::channel::<AppEvent>(crate::config::BROADCAST_CHANNEL_CAPACITY);
        Arc::new(AppState {
            pool: ArcSwap::from_pointee(pool),
            running: RwLock::new(HashMap::new()),
            config: ArcSwap::from_pointee(cfg),
            events_tx: events_tx.clone(),
            peer_server: ArcSwap::from_pointee(crate::peer_server::PeerServer::disabled(
                crate::capture::CaptureStore::new(events_tx.clone()),
            )),
            capture_store: crate::capture::CaptureStore::new(events_tx),
            nat: ArcSwapOption::new(None),
            log_reload: Box::new(|_: &str| {}),
            rebind_notify: Arc::new(tokio::sync::Notify::new()),
        })
    }

    /// Serialize an `AppConfig` to a TOML string. Round-trips through `toml`
    /// so the test never drifts from the real parser.
    fn to_toml(cfg: &crate::config::AppConfig) -> String {
        toml::to_string(cfg).expect("app_config serializes to toml")
    }

    fn write_config(path: &std::path::Path, cfg: &crate::config::AppConfig) {
        std::fs::write(path, to_toml(cfg)).unwrap();
    }

    fn unique_tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("{name}_{}.toml", std::process::id()))
    }

    #[tokio::test]
    async fn reload_swaps_new_config() {
        let tmp = unique_tmp("rf_reload_ok");
        let cfg = test_helpers::app_config();
        write_config(&tmp, &cfg);
        let state = state_from(cfg).await;

        let mut next = state.config.load_full().as_ref().clone();
        next.engine.tick_interval_secs = 7;
        write_config(&tmp, &next);
        reload_config_from(&state, tmp.to_str().unwrap())
            .await
            .expect("reload should succeed");
        assert_eq!(state.config.load().engine.tick_interval_secs, 7);
        let _ = std::fs::remove_file(&tmp);
    }

    /// An invalid config file is rejected: the old config stays live and
    /// reload_config returns Err.
    #[tokio::test]
    async fn reload_rejects_invalid_keeps_old() {
        let tmp = unique_tmp("rf_reload_bad");
        let cfg = test_helpers::app_config();
        write_config(&tmp, &cfg);
        let state = state_from(cfg).await;
        let old_tick = state.config.load().engine.tick_interval_secs;

        // Overwrite with a config that fails validation: min_interval_secs = 0
        // violates `>= 1`. Also bump tick_interval_secs so we can confirm the
        // old config is the one still live (not the rejected one).
        let mut bad = state.config.load_full().as_ref().clone();
        bad.tracker.min_interval_secs = 0;
        bad.engine.tick_interval_secs = 99;
        std::fs::write(&tmp, to_toml(&bad)).unwrap();

        let err = reload_config_from(&state, tmp.to_str().unwrap()).await;
        assert!(err.is_err(), "invalid config must be rejected");
        // Old config must be unchanged.
        assert_eq!(state.config.load().tracker.min_interval_secs, 1);
        assert_eq!(state.config.load().engine.tick_interval_secs, old_tick);
        let _ = std::fs::remove_file(&tmp);
    }

    /// A reload that changes only engine timing swaps the config and emits
    /// the `ConfigReloaded` event.
    #[tokio::test]
    async fn reload_broadcasts_config_reloaded_event() {
        let tmp = unique_tmp("rf_reload_evt");
        let cfg = test_helpers::app_config();
        write_config(&tmp, &cfg);
        let state = state_from(cfg).await;

        let mut rx = state.events_tx.subscribe();
        let mut next = state.config.load_full().as_ref().clone();
        next.engine.stat_interval_secs = 11;
        write_config(&tmp, &next);
        reload_config_from(&state, tmp.to_str().unwrap())
            .await
            .unwrap();

        let saw = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if matches!(rx.recv().await, Ok(AppEvent::ConfigReloaded { .. })) {
                    return true;
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(saw, "reload must broadcast ConfigReloaded");
        let _ = std::fs::remove_file(&tmp);
    }

    /// Reloading a file whose content matches the live config is a no-op: no
    /// swap, no structural re-apply, and no `ConfigReloaded` broadcast. This
    /// prevents a double toast when the settings API writes the file and the
    /// file watcher fires on the same write.
    #[tokio::test]
    async fn reload_unchanged_config_is_noop() {
        let tmp = unique_tmp("rf_reload_noop");
        let cfg = test_helpers::app_config();
        write_config(&tmp, &cfg);
        let state = state_from(cfg).await;

        let mut rx = state.events_tx.subscribe();
        // Re-write the identical config and reload.
        write_config(&tmp, state.config.load_full().as_ref());
        reload_config_from(&state, tmp.to_str().unwrap())
            .await
            .unwrap();

        let saw = tokio::time::timeout(std::time::Duration::from_millis(200), async {
            rx.recv().await
        })
        .await;
        assert!(saw.is_err(), "unchanged reload must not broadcast any event");
        let _ = std::fs::remove_file(&tmp);
    }

    /// Changing server.bind_addr fires the rebind Notify (when no env
    /// override is set). This test clears any leaked REDSWARM_ADDR first.
    #[tokio::test]
    async fn reload_signals_rebind_on_bind_addr_change() {
        // `REDSWARM_ADDR` is an env override that suppresses runtime
        // bind_addr changes; ensure it's unset so the reloader signals a
        // rebind. Safe in single-threaded test scope.
        unsafe { std::env::remove_var("REDSWARM_ADDR") };
        let tmp = unique_tmp("rf_reload_rebind");
        let cfg = test_helpers::app_config();
        write_config(&tmp, &cfg);
        let state = state_from(cfg).await;

        let mut next = state.config.load_full().as_ref().clone();
        next.server.bind_addr = "127.0.0.1:39998".into();
        write_config(&tmp, &next);
        reload_config_from(&state, tmp.to_str().unwrap())
            .await
            .unwrap();

        // notify_one stores a permit if no waiter; notified() returns immediately.
        let fired = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            state.rebind_notify.notified(),
        )
        .await;
        assert!(fired.is_ok(), "bind_addr change must signal rebind");
        let _ = std::fs::remove_file(&tmp);
    }
}
