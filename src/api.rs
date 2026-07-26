//! HTTP API handlers - REST + SSE for the dashboard.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::{ArcSwap, ArcSwapOption};
use askama::Template;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::sse::{Event, KeepAlive, KeepAliveStream, Sse};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use tokio::sync::{broadcast, oneshot, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tower_http::compression::CompressionLayer;

use crate::capture;
use crate::data::{labels, sse, units, vocab};
use crate::db;
use crate::engine::{self, AppEvent, AuditConfig, AuditEvent, TaskSummary};
use crate::magnet;
use crate::render;
use crate::templates;
use crate::torrent;

/// Shared application state, cheaply cloned per request/task as `Arc<AppState>`.
///
/// The hot-reloadable resources - `config`, `pool`, `peer_server`, `nat` - are
/// held in `ArcSwap`/`ArcSwapOption` so the watcher can atomically swap a
/// freshly loaded `config.toml` without blocking readers. Each reader loads a
/// consistent snapshot (`load()` for a cheap guard, `load_full()` for an owned
/// `Arc` held across `.await`s). Running audits capture their own `Arc`
/// snapshots at [`start_engine`] time, freezing them for the audit's lifetime
/// (per the "new audits only" hot-reload policy).
pub struct AppState {
 /// Database pool - swappable when `server.db_url` or
 /// `database.max_connections` changes. Running audits keep their old
 /// `Arc<SqlitePool>` snapshot; the old pool drops once they end.
 pub pool: ArcSwap<sqlx::SqlitePool>,
 pub running: RwLock<HashMap<i64, RunningAudit>>,
 /// Global config - swappable on every successful `config.toml` reload.
 pub config: ArcSwap<crate::config::AppConfig>,
 pub events_tx: broadcast::Sender<AppEvent>,
 /// Peer-wire server - swappable when `peer_server.*` or
 /// `tracker.peer_port` changes. Old server auto-stops (via `Drop`) once
 /// the last running audit holding a snapshot ends.
 pub peer_server: ArcSwap<crate::peer_server::PeerServer>,
 pub capture_store: crate::capture::CaptureStore,
 /// NAT-PMP mapping - `None` when NAT-PMP is disabled. Swappable when
 /// `nat.gateway_ip` changes; the old lease task is cancelled first.
 pub nat: ArcSwapOption<crate::nat::NatMapping>,
 /// Hot-reloads the `tracing` env filter when `server.log_filter` changes.
 /// Set up in `main` from a `tracing_subscriber` reload handle.
 pub log_reload: Box<dyn Fn(&str) + Send + Sync>,
 /// Notifies the HTTP server loop to gracefully rebind when
 /// `server.bind_addr` changes at runtime.
 pub rebind_notify: Arc<tokio::sync::Notify>,
}

/// In-memory state of a running audit, held in `state.running` keyed by
/// audit id. Created by `start_engine`, removed by the engine task's
/// cleanup tail after the task fully shuts down.
pub struct RunningAudit {
 pub cancel: CancellationToken,
 /// Resolves when the engine task has fully shut down (stopped announce
 /// sent, DB writer drained, status flipped to stopped, `running` entry
 /// removed). Taken by `stop_running_task` so edit/delete can await it.
 pub done: Option<oneshot::Receiver<()>>,
 /// Column visibility for this audit's mode/strategy - computed once at
 /// start from `LogColumns::for_config` and reused by the SSE handler so
 /// SSE-injected log rows match the server-rendered table columns.
 pub log_columns: templates::LogColumns,
 /// Last-seen upload speed (bytes/sec) from the engine tick - used by
 /// `sum_goal_counters` to compute global goal ETAs without a DB round-trip.
 pub last_up_bps: u64,
 /// Last-seen download speed (bytes/sec).
 pub last_down_bps: u64,
}

pub type SharedState = Arc<AppState>;

// Handlers

pub async fn index(State(state): State<SharedState>) -> Html<String> {
 // Render the page with the FULL task list + log panel server-side, so first
 // paint IS the final DOM - no JS hydration swap, no layout shift, no forced
 // reflow. JS only reads state (clientMap, runtime settings, counts) from
 // the bootstrap JSON and the pre-rendered DOM; it never rebuilds the
 // initial view.
 let pool = state.pool.load_full();
 let cfg = state.config.load();

 let clients: Vec<(String, String)> = cfg.clients.iter()
 .map(|c| (c.peer_id_prefix.clone(), c.display_name()))
 .collect();
 let settings = (**cfg).clone();

 let rows = db::list_audits(&pool).await.unwrap_or_default();
 let mut summaries = Vec::with_capacity(rows.len());
 for r in &rows {
 summaries.push(task_summary(&state, r).await);
 }

 // Build the active log response (for the first audit, if any) so we can
 // server-render the full log panel.
 let (log_data, active_log_id) = if let Some(first) = rows.first() {
 let id = first.id;
 match build_audit_log_response(&state, &pool, id).await {
 Ok(data) => (Some(data), id),
 Err(_) => (None, id),
 }
 } else {
 (None, 0)
 };

 // Topbar counts from the summaries (server-side, so first paint shows them).
 let running_count = summaries.iter().filter(|s| s.status == vocab::STATUS_RUNNING).count();
 let stopped_count = summaries.iter().filter(|s| s.status == vocab::STATUS_STOPPED).count();

 // Build global goal tiles for the topbar - one per enabled goal with its
 // summed progress + ETA. Server-rendered so first paint shows them; the
 // JS patches them live from `goal_progress` SSE events.
 let goal_tiles = build_global_goal_tiles(&state).await;

 // Pre-render the HTML via the shared render functions (same output the JS
 // would produce - verified by tests).
 let topbar_stats_html = render::render_topbar_stats(running_count, stopped_count, &goal_tiles);
 let task_list_html = render::render_task_list(&summaries, active_log_id);
 let goals = db::list_goals(&pool).await.unwrap_or_default();
 let goal_list_html = render::render_goals_table(&goals);
 let log_panel_html = if let Some(data) = &log_data {
 render::render_log_panel(
 &data.events,
 Some(&data.audit_info),
 &data.columns,
 &data.total_uploaded,
 data.success_count,
 )
 } else {
 format!(r#"<div class="empty">{}</div>"#, labels::EMPTY_LOG)
 };

 // Pre-render the byte-unit <option>s, the client dropdown <option>s, and
 // the settings modal nav + panes so the JS never builds any of this DOM.
 let byte_unit_options_mib = render::render_byte_unit_options(&units::BYTE_UNIT_MIB.to_string());
 let byte_amount_options_mib = render::render_byte_amount_options(&units::BYTE_UNIT_MIB.to_string());
 let client_dropdown_html = render::render_client_dropdown(&clients);
 let settings_nav_html = render::render_settings_nav();
 let settings_panes_html = render::render_settings_panes();

 let bootstrap = serde_json::json!({
 "clients": clients,
 "settings": settings,
 });

 let bootstrap_json = json_for_script(&serde_json::to_string(&bootstrap).unwrap_or_default());

 Html(templates::IndexTemplate {
 bootstrap_json,
 topbar_stats_html,
 task_list_html,
 goal_list_html,
 log_panel_html,
 byte_unit_options_mib,
 byte_amount_options_mib,
 client_dropdown_html,
 settings_nav_html,
 settings_panes_html,
 }.render().unwrap_or_default())
}

/// Escape a JSON string for safe embedding inside an HTML `<script>` element.
///
/// `serde_json::to_string` produces valid JSON (and valid JS) but does not
/// escape `<`, `>`, or `&`, so any of those in a string value lets an attacker
/// break out of the script element. This rewrites the few characters that are
/// dangerous in a script context to their JS `\u` escapes - the result is
/// still valid JSON and valid JS, and no longer contains a literal
/// `</script>`. `U+2028`/`U+2029` are valid in JSON strings but not in JS
/// string literals (pre-2019 engines), so they are escaped too.
fn json_for_script(s: &str) -> String {
 s.replace('<', "\\u003c")
 .replace('>', "\\u003e")
 .replace('&', "\\u0026")
 .replace('\u{2028}', "\\u2028")
 .replace('\u{2029}', "\\u2029")
}

/// `GET /api/bootstrap` - single endpoint returning all data needed for
/// initial page render. Combines clients, settings, audits, and the active
/// log in one response to eliminate 4 sequential round-trips.
pub async fn bootstrap(State(state): State<SharedState>) -> Json<serde_json::Value> {
 let cfg = state.config.load();
 let pool = state.pool.load();

 // Clients (peer_id_prefix, display_name) pairs
 let clients: Vec<(String, String)> = cfg.clients.iter()
 .map(|c| (c.peer_id_prefix.clone(), c.display_name()))
 .collect();

 // Settings (full AppConfig)
 let settings = (**cfg).clone();

 // Audits (TaskSummary list)
 let rows = db::list_audits(&pool).await.unwrap_or_default();
 let mut summaries = Vec::with_capacity(rows.len());
 for r in &rows {
 summaries.push(task_summary(&state, r).await);
 }

 // Active log (first running audit, or first audit if any)
 let log = if let Some(first) = rows.first() {
 build_audit_log_response(&state, &pool, first.id).await.ok().map(|data| {
 serde_json::json!({
 "events": data.events,
 "total_uploaded": data.total_uploaded,
 "success_count": data.success_count,
 "columns": data.columns,
 "audit_info": data.audit_info,
 })
 })
 } else {
 None
 };

 Json(serde_json::json!({
 "clients": clients,
 "settings": settings,
 "audits": summaries,
 "log": log,
 }))
}

/// `GET /api/clients` - the current emulated-client identities. Used by the
/// frontend to refresh the new-task "Client emulation" dropdown when
/// `config.toml` is hot-reloaded (clients added/removed/renamed at runtime).
/// Returns `(peer_id_prefix, display_name)` pairs - the prefix is the unique
/// key (the dropdown `value` and what `forced_client` stores), the display
/// name is what the user sees.
pub async fn list_clients(State(state): State<SharedState>) -> Json<Vec<(String, String)>> {
 let cfg = state.config.load();
 Json(cfg.clients.iter().map(|c| (c.peer_id_prefix.clone(), c.display_name())).collect())
}

/// `GET /api/settings` - the full current `AppConfig` as JSON. Used by the
/// settings modal to populate every field. Returns a clone of the live
/// `ArcSwap` config snapshot so the UI always reflects what the backend sees.
pub async fn get_settings(State(state): State<SharedState>) -> Json<crate::config::AppConfig> {
 let cfg = state.config.load_full();
 Json((*cfg).clone())
}

/// `PUT /api/settings` - validate the incoming `AppConfig`, write it to
/// `config.toml`, then hot-reload via [`reload::reload_config_from`]. The
/// reloader atomically swaps the config, re-applies structural settings (DB
/// pool, peer-wire server, NAT-PMP, log filter, HTTP bind address), and
/// broadcasts a `config_reloaded` SSE event. Running audits are unaffected
/// (frozen on their startup config); new audits and per-request handlers see
/// the new values immediately. A no-op save (identical config) is
/// short-circuited by the reloader - no swap, no broadcast.
pub async fn update_settings(
 State(state): State<SharedState>,
 Json(body): Json<crate::config::AppConfig>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 body.validate()
 .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
 let path = crate::config::path();
 crate::config::save_to_path(&path, &body)
 .map_err(|e| server_err(e, "write config.toml"))?;
 crate::reload::reload_config_from(&state, &path)
 .await
 .map_err(|e| server_err(e, "reload config"))?;
 Ok(Json(serde_json::json!({ "path": path })))
}

/// Resolve a stored `peer_id_prefix` (or alias) to the client's display name.
/// Used by server-rendered templates that can't access the JS `clientMap`.
/// Falls back to the raw string if no matching client is found.
fn resolve_client_display(cfg: &crate::config::AppConfig, key: &str) -> String {
 crate::peer_id::find_by_client(&cfg.clients, key)
 .map(|idx| cfg.clients[idx].display_name())
 .unwrap_or_else(|| key.to_string())
}

/// `GET /api/audits` - returns the task list as JSON.
pub async fn list_audits_json(State(state): State<SharedState>) -> Json<Vec<TaskSummary>> {
 let pool = state.pool.load_full();
 let rows = db::list_audits(&pool).await.unwrap_or_default();
 let mut summaries = Vec::with_capacity(rows.len());
 for r in rows {
 summaries.push(task_summary(&state, &r).await);
 }
 Json(summaries)
}

/// `GET /html/audits` - returns the pre-rendered task list HTML fragment
/// (the `<table>` with header + rows, or the empty placeholder). The server is
/// the single source of truth for task-row HTML; the JS never builds it. Used
/// by `loadTaskList` after a config reload or empty↔non-empty transition.
/// No row is marked active (the active-log id is client-side state; the JS
/// re-applies `.active` after inserting the fragment).
pub async fn list_audits_html(State(state): State<SharedState>) -> Html<String> {
 let pool = state.pool.load_full();
 let rows = db::list_audits(&pool).await.unwrap_or_default();
 let mut summaries = Vec::with_capacity(rows.len());
 for r in &rows {
 summaries.push(task_summary(&state, r).await);
 }
 Html(render::render_task_list(&summaries, 0))
}

fn extract_host(url: &str) -> String {
 url.split("//")
 .nth(1)
 .and_then(|rest| rest.split('/').next())
 .or_else(|| url.split('/').next())
 .unwrap_or(url)
 .to_string()
}

/// Build a `TaskSummary` from a DB row + peer state, for SSE `task_created` events.
async fn task_summary(state: &SharedState, r: &db::AuditRow) -> TaskSummary {
 let pool = state.pool.load_full();
 let cfg = state.config.load();
 let config: AuditConfig =
 serde_json::from_str(&r.config_json).unwrap_or_else(|_| AuditConfig::from_defaults(&cfg.defaults, &cfg.swarm_defaults));
 let peer = db::get_peer_state(&pool, r.id).await.unwrap_or_default();
 TaskSummary {
 id: r.id,
 name: r.name.clone(),
 tracker: extract_host(&r.announce_url),
 announce_url: r.announce_url.clone(),
 info_hash: r.info_hash.clone(),
 working_client: r.working_client.as_ref().map(|c| resolve_client_display(&cfg, c)),
 status: r.status.clone(),
 created_at: r.created_at.clone(),
 uploaded: peer.uploaded,
 downloaded: peer.downloaded,
 mode: match config.mode {
 crate::engine::Mode::DownloadAndUpload => labels::MODE_DU_ABBR.into(),
 crate::engine::Mode::UploadOnly => labels::MODE_UO_ABBR.into(),
 },
 strategy: match config.speed_mode {
 crate::engine::SpeedMode::Fixed => labels::STRATEGY_FIXED.into(),
 crate::engine::SpeedMode::Dynamic => labels::STRATEGY_DYNAMIC.into(),
 },
 goal: config.goal,
 }
}

#[derive(Deserialize)]
pub struct CreateAudit {
 pub name: String,
 pub announce_url: String,
 pub info_hash: String,
 pub torrent_size: u64,
 pub config: AuditConfig,
}

/// `PUT /api/audits/{id}` request body - only the config is editable; the
/// torrent identity (announce_url, info_hash, torrent_size, name) is locked.
#[derive(Deserialize)]
pub struct UpdateAudit {
 pub config: AuditConfig,
}

/// Log an internal error at ERROR level and wrap it as a 500 response, so
/// database failures surface in backend logs - not only in the HTTP body.
fn server_err<E: std::fmt::Display>(e: E, what: &'static str) -> (StatusCode, String) {
 tracing::error!(error = %e, what, "request failed");
 (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub async fn create_audit(
 State(state): State<SharedState>,
 Json(body): Json<CreateAudit>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let mut config = body.config;
 if config.announce_url.is_empty() {
 config.announce_url = body.announce_url.clone();
 }
 if config.info_hash.is_empty() {
 config.info_hash = body.info_hash.clone();
 }
 if config.torrent_size == 0 {
 config.torrent_size = body.torrent_size;
 }
 // Fill unset fields from config.toml defaults
 let cfg = state.config.load();
 let pool = state.pool.load_full();
 let d = &cfg.defaults;
 if config.upload_bps == 0 {
 config.upload_bps = d.upload_bps;
 }
 if config.download_bps == 0 {
 config.download_bps = d.download_bps;
 }
 if config.jitter_pct == 0 {
 config.jitter_pct = d.jitter_pct as u8;
 }
 if config.ramp_up_secs == 0 {
 config.ramp_up_secs = d.ramp_up_secs;
 }
 if config.swarm.avg_leecher_download_bps == 0 {
 config.swarm = crate::swarm::SwarmConfig::from_defaults(&cfg.swarm_defaults);
 }
 config
 .validate()
 .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
 let config_json = serde_json::to_string(&config)
 .map_err(|e| server_err(e, "serialize audit config"))?;
 let id = db::insert_audit(
 &pool,
 &body.name,
 &body.announce_url,
 &body.info_hash,
 body.torrent_size,
 &config_json,
 )
 .await
 .map_err(|e| server_err(e, "insert_audit"))?;
 let row = db::get_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "get_audit after insert"))?
 .ok_or_else(|| {
 tracing::error!(id, "row vanished immediately after insert");
 (StatusCode::INTERNAL_SERVER_ERROR, "row vanished after insert".into())
 })?;
 let task = task_summary(&state, &row).await;
 let _ = state.events_tx.send(AppEvent::TaskCreated { task });
 // Pre-render the log panel HTML so the client can display it without a
 // second round-trip to GET /html/audits/{id}/log.
 let log_data = build_audit_log_response(&state, &pool, id).await
 .map_err(|e| server_err(e, "build log panel after create"))?;
 let log_html = render::render_log_panel(
 &log_data.events,
 Some(&log_data.audit_info),
 &log_data.columns,
 &log_data.total_uploaded,
 log_data.success_count,
 );
 Ok(Json(serde_json::json!({ "id": id, "log_html": log_html })))
}

/// `GET /api/audits/{id}` - return the full audit row with raw config, for
/// populating the edit-task form. The config is deserialized from the stored
/// `config_json` and returned as a structured object (not display strings).
pub async fn get_audit(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let pool = state.pool.load_full();
 let row = db::get_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "get_audit"))?
 .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;
 let cfg = state.config.load();
 let config: AuditConfig =
 serde_json::from_str(&row.config_json).unwrap_or_else(|_| AuditConfig::from_defaults(&cfg.defaults, &cfg.swarm_defaults));
 Ok(Json(serde_json::json!({
 "id": row.id,
 "name": row.name,
 "announce_url": row.announce_url,
 "info_hash": row.info_hash,
 "torrent_size": row.torrent_size,
 "config": config,
 })))
}

/// `PUT /api/audits/{id}` - edit a task's config. The torrent identity
/// (announce_url, info_hash, torrent_size) is locked: the handler overwrites
/// those three fields in the incoming config with the values already stored
/// in the DB, so a tampered request can't change the torrent.
///
/// If the config is unchanged, this is a no-op (a running task is left
/// running). If the config changed, the task is stopped (if running), the
/// event log + peer state (counters, peer_id, key, working_client) are wiped
/// (equivalent to delete + recreate). The new config is persisted, and the
/// task is restarted (only if it was running). The fresh start generates a
/// new peer_id and probes from scratch. Emits `task_updated`, `task_progress`
/// (reset), and `task_client` (cleared) over SSE.
pub async fn update_audit(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
 Json(body): Json<UpdateAudit>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let pool = state.pool.load_full();
 let row = db::get_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "get_audit"))?
 .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;

 let mut config = body.config;
 // Lock the torrent identity - the edit form can't change these.
 config.announce_url = row.announce_url.clone();
 config.info_hash = row.info_hash.clone();
 config.torrent_size = row.torrent_size as u64;
 config
 .validate()
 .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

 // Compare against the stored config to detect a real change. The stored
 // config already has the locked identity fields, so a pure identity match
 // (unchanged config) is detected without stopping anything.
 let old_config: AuditConfig = serde_json::from_str(&row.config_json).unwrap_or_else(|_| {
 let cfg = state.config.load();
 AuditConfig::from_defaults(&cfg.defaults, &cfg.swarm_defaults)
 });
 let changed = config != old_config;
 if !changed {
 return Ok(Json(serde_json::json!({ "id": id, "unchanged": true })));
 }

 // Capture whether the task was running before stopping, so we can restart
 // it after the reset (edit = "stop + start back with new settings").
 let was_running = state.running.read().await.contains_key(&id);
 stop_running_task(&state, id).await;

 let config_json = serde_json::to_string(&config)
 .map_err(|e| server_err(e, "serialize audit config"))?;
 db::update_audit_config(&pool, id, &config_json)
 .await
 .map_err(|e| server_err(e, "update_audit_config"))?;
 db::reset_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "reset_audit"))?;

 // SSE: tell the UI the counters and working client are cleared.
 let _ = state.events_tx.send(AppEvent::TaskProgress { id, uploaded: 0, downloaded: 0 });
 let _ = state.events_tx.send(AppEvent::TaskClient { id, working_client: None });
 let row = db::get_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "get_audit after update"))?
 .ok_or_else(|| {
 tracing::error!(id, "row vanished immediately after update");
 (StatusCode::INTERNAL_SERVER_ERROR, "row vanished after update".into())
 })?;
 let task = task_summary(&state, &row).await;
 let _ = state.events_tx.send(AppEvent::TaskUpdated { task });

 if was_running {
 // Fresh start: reset_audit wiped peer state, so start_engine sees no
 // resume data and generates a new peer_id + probes from scratch.
 start_engine(&state, id).await?;
 }
 // Pre-render the log panel HTML (empty after reset) so the client can
 // display it without a second round-trip to GET /html/audits/{id}/log.
 let log_data = build_audit_log_response(&state, &pool, id).await
 .map_err(|e| server_err(e, "build log panel after update"))?;
 let log_html = render::render_log_panel(
 &log_data.events,
 Some(&log_data.audit_info),
 &log_data.columns,
 &log_data.total_uploaded,
 log_data.success_count,
 );
 Ok(Json(serde_json::json!({ "id": id, "restarted": was_running, "log_html": log_html })))
}

/// Start (or resume) the engine for audit `id`. Shared by the HTTP
/// [`start_audit`] handler and the boot-time auto-restart loop in `main()`.
/// Returns `Ok(true)` if the engine was started, `Ok(false)` if it was
/// already running.
pub async fn start_engine(state: &SharedState, id: i64) -> Result<bool, (StatusCode, String)> {
 // Snapshot the config + pool for this audit's lifetime. The engine and
 // its DB writer borrow these (frozen) for the whole task, so a hot-reload
 // swap of `state.config`/`state.pool` does NOT affect a running audit -
 // only newly-started audits pick up the new values (the "new audits only"
 // hot-reload policy).
 //
 // Apply the NAT public-port override to the advertised `tracker.peer_port`:
 // config.toml holds the INTERNAL port (what the peer-wire server binds);
 // when NAT-PMP is active, the tracker must be told the PUBLIC port so peers
 // reach us through the gateway (RFC 6886). This is computed at snapshot
 // time so a frozen audit keeps the port it was started with.
 let mut cfg_snap = (*state.config.load_full()).clone();
 if let Some(m) = state.nat.load_full() {
 cfg_snap.tracker.peer_port = m.public_port;
 }
 let cfg_snap = Arc::new(cfg_snap);
 let pool_snap = state.pool.load_full();
 let row = db::get_audit(&pool_snap, id)
 .await
 .map_err(|e| server_err(e, "get_audit"))?
 .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;

 // Don't start if already running
 if state.running.read().await.contains_key(&id) {
 return Ok(false);
 }

 let mut config: AuditConfig = serde_json::from_str(&row.config_json)
 .unwrap_or_else(|_| AuditConfig::from_defaults(&cfg_snap.defaults, &cfg_snap.swarm_defaults));
 let log_columns = templates::LogColumns::for_config(config.mode, config.speed_mode);

 let (tx, _rx0) = broadcast::channel::<AuditEvent>(crate::config::BROADCAST_CHANNEL_CAPACITY);
 let cancel = CancellationToken::new();
 let (done_tx, done_rx) = oneshot::channel::<()>();

 {
 let mut running = state.running.write().await;
 running.insert(
 id,
 RunningAudit {
 cancel: cancel.clone(),
 done: Some(done_rx),
 log_columns,
 last_up_bps: 0,
 last_down_bps: 0,
 },
 );
 }
 db::update_status(&pool_snap, id, vocab::STATUS_RUNNING)
 .await
 .map_err(|e| server_err(e, "update_status running"))?;
 let _ = state.events_tx.send(AppEvent::TaskStatus { id, status: vocab::STATUS_RUNNING.into() });

 // Don't clear events - preserve history across stop/start.
 // Continue seq numbering from where the last run left off.
 let start_seq = db::get_max_seq(&pool_snap, id)
 .await
 .unwrap_or(0);

 config.announce_url = row.announce_url.clone();
 config.info_hash = row.info_hash.clone();
 config.torrent_size = row.torrent_size as u64;

 // Resolve which client to use. A forced_client from the config takes
 // priority (user explicitly chose one in the edit form → skip probing).
 // Otherwise fall back to the working_client stored from a previous
 // successful probe (resume). If neither is set, the engine probes all.
 let forced_client = config
 .forced_client
 .as_deref()
 .and_then(|key| crate::peer_id::find_by_client(&cfg_snap.clients, key));
 let known_client = forced_client.or_else(|| {
 row.working_client
 .as_deref()
 .and_then(|key| crate::peer_id::find_by_client(&cfg_snap.clients, key))
 });

 // When the probe is skipped (forced client, or resumed from a previously
 // stored working client), the engine never emits a probe event, so the
 // working-client key is never persisted nor broadcast to the UI. Record
 // it here: the engine uses the resolved index directly; this only writes
 // the display key and notifies the UI so the task list / log panel show
 // the client instead of "-" / "probing...".
 if let Some(idx) = known_client {
 let prefix = cfg_snap.clients[idx].peer_id_prefix.clone();
 if let Err(e) = db::set_working_client(&pool_snap, id, &prefix).await {
 tracing::warn!(error = %e, "set_working_client for known client failed");
 }
 let _ = state.events_tx.send(AppEvent::TaskClient {
 id,
 working_client: Some(prefix),
 });
 }

 let resume = db::get_peer_state(&pool_snap, id)
 .await
 .map_err(|e| server_err(e, "get_peer_state"))?;
 let resume = if resume.uploaded > 0 || resume.downloaded > 0 || resume.left > 0 {
 Some(resume.into())
 } else {
 None
 };

 // Subscribe the DB writer before spawning so the engine task owns the
 // receiver; the original `tx` (dropped when start_engine returns) is the
 // only other sender besides `tx_engine`.
 let writer_rx = tx.subscribe();

 // Peer-server snapshot - frozen for this audit so a hot-reload swap of
 // `state.peer_server` (with a new port/timeouts) does not disturb the
 // running audit's wire identity. The old server auto-stops once the last
 // audit holding it ends (see `PeerServer::Drop`).
 let ps_snap = state.peer_server.load_full();

 // Engine task - runs the audit, owns the DB writer sub-task, then marks
 // stopped + cleans up. The DB writer is spawned here (not as a sibling)
 // so the engine task can await its drain before signaling `done`, which
 // lets edit/delete wait for a fully quiesced shutdown.
 let cancel_engine = cancel.clone();
 let tx_engine = tx.clone();
 let events_tx_engine = state.events_tx.clone();
 let state_engine = state.clone();
 tokio::spawn(async move {
 let writer = {
 let pool_writer = Arc::clone(&pool_snap);
 let events_tx_writer = state_engine.events_tx.clone();
 let state_writer = state_engine.clone();
 let mut rx = writer_rx;
 tokio::spawn(async move {
 loop {
 match rx.recv().await {
 Ok(ev) => {
 if let Some(client) = ev.working_client.as_ref()
 && let Err(e) = db::set_working_client(&pool_writer, id, client).await {
 tracing::warn!(error = %e, "working client update failed");
 }
 if let Err(e) = db::insert_event(&pool_writer, &ev).await {
 tracing::warn!(error = %e, "event insert failed");
 }
 // Forward to global SSE
 let _ = events_tx_writer.send(AppEvent::Audit(ev.clone()));
 // Progress update on tick/started/stopped events
 if matches!(ev.event, vocab::EVENT_TICK | vocab::EVENT_STARTED | vocab::EVENT_STOPPED | vocab::EVENT_REGULAR | vocab::EVENT_COMPLETED) {
 let _ = events_tx_writer.send(AppEvent::TaskProgress {
 id: ev.audit_id,
 uploaded: ev.uploaded,
 downloaded: ev.downloaded,
 });
 // Cache the latest speeds on the RunningAudit so
 // `sum_goal_counters` can read them without a DB
 // round-trip, then broadcast global goal progress.
 {
 let mut running = state_writer.running.write().await;
 if let Some(r) = running.get_mut(&ev.audit_id) {
 r.last_up_bps = ev.fair_share_bps;
 r.last_down_bps = ev.dynamic_target_bps;
 }
 }
 broadcast_goals_for_task(&state_writer, ev.audit_id).await;
 }
 // Working client detected during probe
 if let Some(client) = ev.working_client.as_ref() {
 let _ = events_tx_writer.send(AppEvent::TaskClient {
 id: ev.audit_id,
 working_client: Some(client.clone()),
 });
 }
 }
 Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!(n, "db writer lagged"),
 Err(broadcast::error::RecvError::Closed) => break,
 }
 }
 })
 };
 engine::run(config, &cfg_snap, id, engine::RunOptions { known_client, resume, start_seq, peer_server: Some(ps_snap) }, &pool_snap, tx_engine, cancel_engine)
 .await;
 // tx_engine was consumed by engine::run and drops on its return - the
 // broadcast channel closes (the original `tx` already dropped when
 // start_engine returned), so the writer's recv() returns Closed.
 let _ = writer.await;
 db::update_status(&pool_snap, id, vocab::STATUS_STOPPED)
 .await
 .unwrap_or_else(|e| tracing::warn!(error = %e, "status update failed"));
 let _ = events_tx_engine.send(AppEvent::TaskStatus { id, status: vocab::STATUS_STOPPED.into() });
 let mut running = state_engine.running.write().await;
 running.remove(&id);
 let _ = done_tx.send(());
 });

 Ok(true)
}

pub async fn start_audit(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let started = start_engine(&state, id).await?;
 Ok(Json(if started {
 serde_json::json!({ "id": id, "started": true })
 } else {
 serde_json::json!({ "id": id, "already_running": true })
 }))
}

pub async fn stop_audit(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let pool = state.pool.load_full();
 // Verify the audit exists - a 404 is more useful than a silent 200.
 db::get_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "get_audit"))?
 .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;
 let running = state.running.read().await;
 if let Some(r) = running.get(&id) {
 r.cancel.cancel();
 }
 drop(running);
 // The engine task sends TaskStatus { stopped } when it finishes.
 // But send it here too for immediate UI feedback before the engine winds down.
 let _ = state.events_tx.send(AppEvent::TaskStatus { id, status: vocab::STATUS_STOPPED.into() });
 Ok(Json(serde_json::json!({ "id": id, "stopped": true })))
}

/// Cancel a running task and wait for it to fully shut down (stopped announce
/// sent, DB writer drained, status flipped). Used by edit/delete so they can
/// mutate a task that was running without rejecting the request. If the task
/// isn't running, returns immediately. The wait is bounded by
/// `engine.stop_grace_secs` as a safety net against a hung engine.
async fn stop_running_task(state: &SharedState, id: i64) {
 let done: Option<oneshot::Receiver<()>> = {
 let mut running = state.running.write().await;
 running.get_mut(&id).and_then(|r| {
 r.cancel.cancel();
 r.done.take()
 })
 };
 if let Some(mut done) = done {
 // Optimistic UI feedback - the engine task also sends this (a no-op
 // duplicate once the status is already stopped).
 let _ = state.events_tx.send(AppEvent::TaskStatus { id, status: vocab::STATUS_STOPPED.into() });
 let _ = tokio::time::timeout(
 std::time::Duration::from_secs(state.config.load().engine.stop_grace_secs),
 &mut done,
 ).await;
 }
}

pub async fn delete_audit(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 // Stop the task first if it's running, then delete.
 stop_running_task(&state, id).await;
 let pool = state.pool.load_full();
 // Verify the audit exists before deleting - a 404 is more useful than a
 // silent 200 for a nonexistent id.
 db::get_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "get_audit"))?
 .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;
 db::delete_audit(&pool, id)
 .await
 .map_err(|e| server_err(e, "delete_audit"))?;
 let _ = state.events_tx.send(AppEvent::TaskDeleted { id });
 Ok(Json(serde_json::json!({ "id": id, "deleted": true })))
}

/// Build the `config_rows` for the audit-info panel - only fields the user
/// can set in the "New task" form AND that are relevant to the chosen
/// mode/strategy. Shared by [`audit_log_json`] so the JSON contract matches
/// what the former HTML endpoint rendered.
fn build_config_rows(config: &AuditConfig) -> Vec<(String, String)> {
 use crate::engine::{Mode, SpeedMode};
 let mut rows = vec![
 (labels::L_MODE.into(), match config.mode {
 Mode::DownloadAndUpload => labels::MODE_DU_FULL.into(),
 Mode::UploadOnly => labels::MODE_UO_FULL.into(),
 }),
 (labels::L_STRATEGY.into(), match config.speed_mode {
 SpeedMode::Fixed => labels::STRATEGY_FIXED.into(),
 SpeedMode::Dynamic => labels::STRATEGY_DYNAMIC.into(),
 }),
 (labels::L_UPLOAD_SPEED.into(), units::fmt_speed_bps(config.upload_bps)),
 ];

 // Download speed only matters in Download+Upload mode
 if config.mode == Mode::DownloadAndUpload {
 rows.push((labels::L_DOWNLOAD_SPEED.into(), units::fmt_speed_bps(config.download_bps)));
 }

 rows.push((labels::L_JITTER.into(), format!("±{}%", config.jitter_pct)));
 rows.push((labels::L_RAMP_UP.into(), format!("{}s", config.ramp_up_secs)));

 // Start pct only matters in Download+Upload mode
 if config.mode == Mode::DownloadAndUpload {
 rows.push((labels::L_START_PCT.into(), format!("{}%", config.start_download_pct)));
 }

 rows.push((labels::L_FREEZE_ZERO_LEECHERS.into(), if config.freeze_on_zero_leechers { labels::ON } else { labels::OFF }.into()));

 // Freeze 0 seeders only matters in Download+Upload mode (Upload only never downloads)
 if config.mode == Mode::DownloadAndUpload {
 rows.push((labels::L_FREEZE_ZERO_SEEDERS.into(), if config.freeze_on_zero_seeders { labels::ON } else { labels::OFF }.into()));
 }

 // Swarm params only matter in Dynamic mode
 if config.speed_mode == SpeedMode::Dynamic {
 rows.push((labels::L_SWARM_MULTIPLIER.into(), format!("{:.1}×", config.swarm.fair_share_multiplier)));
 rows.push((labels::L_MAX_UPLOAD.into(), if config.swarm.max_upload_bps > 0 { units::fmt_speed_bps(config.swarm.max_upload_bps) } else { labels::INFINITY.into() }));
 rows.push((labels::L_MAX_DOWNLOAD.into(), if config.swarm.max_download_bps > 0 { units::fmt_speed_bps(config.swarm.max_download_bps) } else { labels::INFINITY.into() }));
 }

 // Goal config: direction + target(s) + (reverse mode) deadline + reached
 // action. Hidden when the goal is disabled.
 if config.goal.enabled {
 rows.push((labels::L_GOAL_DIRECTION.into(), match config.goal.direction {
 crate::engine::GoalDirection::Upload => labels::GOAL_DIRECTION_UPLOAD.into(),
 crate::engine::GoalDirection::DownloadAndUpload => labels::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD.into(),
 }));
 if config.goal.direction.tracks_upload() {
 rows.push((labels::L_GOAL_TARGET.into(), units::fmt_bytes(config.goal.upload_target)));
 }
 if config.goal.direction.tracks_download() {
 rows.push((labels::L_GOAL_DOWNLOAD_TARGET.into(), units::fmt_bytes(config.goal.download_target)));
 }
 if config.goal.target_secs > 0 {
 rows.push((labels::L_GOAL_TIME.into(), units::fmt_duration(config.goal.target_secs)));
 }
 rows.push((labels::L_GOAL_REACHED_ACTION.into(), match config.goal.reached_action {
 crate::engine::GoalReachedAction::Stop => labels::GOAL_REACHED_STOP.into(),
 crate::engine::GoalReachedAction::ContinueInitial => labels::GOAL_REACHED_CONTINUE_INITIAL.into(),
 crate::engine::GoalReachedAction::ContinueCustom => labels::GOAL_REACHED_CONTINUE_CUSTOM.into(),
 }));
 if config.goal.reached_action == crate::engine::GoalReachedAction::ContinueCustom {
 rows.push((labels::L_GOAL_REACHED_SPEED.into(), units::fmt_speed_bps(config.goal.reached_bps)));
 }
 }

 rows
}

/// Typed audit-log response - shared between `audit_log_json` (serialized to
/// JSON for the API) and `index` (used to server-render the log panel HTML).
/// A single struct ensures both paths see the same data.
struct AuditLogResponse {
 events: Vec<templates::EventView>,
 total_uploaded: String,
 success_count: usize,
 columns: templates::LogColumns,
 audit_info: templates::AuditInfoView,
}

/// `GET /api/audits/{id}/log` - returns the audit log as JSON: events, the
/// running totals, the mode/strategy-driven column visibility, and the
/// audit-info panel (torrent identity + configuration rows).
pub async fn audit_log_json(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let pool = state.pool.load_full();
 let data = build_audit_log_response(&state, &pool, id)
 .await
 .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
 Ok(Json(serde_json::json!({
 "events": data.events,
 "total_uploaded": data.total_uploaded,
 "success_count": data.success_count,
 "columns": data.columns,
 "audit_info": data.audit_info,
 })))
}

/// `GET /html/audits/{id}/log` - returns the pre-rendered log panel HTML
/// fragment. The server is the single source of truth for log-panel HTML; the
/// JS never builds it. Used by `loadLog` when the user clicks a task row.
pub async fn audit_log_html(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
) -> Response {
 let pool = state.pool.load_full();
 match build_audit_log_response(&state, &pool, id).await {
 Ok(data) => Html(render::render_log_panel(
 &data.events,
 Some(&data.audit_info),
 &data.columns,
 &data.total_uploaded,
 data.success_count,
 )).into_response(),
 Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
 }
}

/// Build the audit log response. Shared between `audit_log_json` (serialized
/// to JSON) and `index` (used to server-render the log panel HTML).
async fn build_audit_log_response(
 state: &SharedState,
 pool: &sqlx::SqlitePool,
 id: i64,
) -> anyhow::Result<AuditLogResponse> {
 let cfg = state.config.load();
 let events = db::list_events(pool, id, cfg.ui.event_log_limit).await?;
 let audit = db::get_audit(pool, id).await?.ok_or_else(|| anyhow::anyhow!("audit not found"))?;
 let config: AuditConfig = serde_json::from_str(&audit.config_json).unwrap_or_else(|_| AuditConfig::from_defaults(&cfg.defaults, &cfg.swarm_defaults));
 let columns = templates::LogColumns::for_config(config.mode, config.speed_mode);
 let views: Vec<_> = events.iter().map(|ev| templates::EventView::from_event(ev, columns.show_download_speed)).collect();
 let total_uploaded = views.first().map(|v| v.uploaded_display.clone()).unwrap_or_default();
 let success_count = views.iter().filter(|v| v.success).count();
 let audit_info = templates::AuditInfoView {
 name: audit.name.clone(),
 status: audit.status.clone(),
 working_client: audit.working_client.as_ref().map(|c| resolve_client_display(&cfg, c)),
 torrent_info: vec![
 (labels::L_ANNOUNCE_URL.to_string(), audit.announce_url.clone()),
 (labels::L_INFO_HASH.to_string(), audit.info_hash.clone()),
 (labels::L_TORRENT_SIZE.to_string(), units::fmt_bytes_i64(audit.torrent_size)),
 ],
 config_rows: build_config_rows(&config),
 goal: config.goal,
 };
 Ok(AuditLogResponse { events: views, total_uploaded, success_count, columns, audit_info })
}

/// GET /api/events - global SSE stream. One connection drives all dynamic UI:
/// log panel (audit events), task list (status/client/progress), and lifecycle
/// (created/deleted). The JS dispatcher routes by event name.
pub async fn sse_global(
 State(state): State<SharedState>,
) -> Sse<KeepAliveStream<ReceiverStream<Result<Event, std::convert::Infallible>>>> {
 let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(
 crate::config::SSE_CHANNEL_CAPACITY,
 );
 let sse_state = state.clone();
 let mut rx = state.events_tx.subscribe();
 tokio::spawn(async move {
 loop {
 match rx.recv().await {
 Ok(ev) => {
 let (name, json) = match &ev {
 AppEvent::Audit(audit_ev) => {
 // Pre-render the log <tr> HTML so the JS never
 // builds row HTML - single source of truth in
 // render.rs. Column visibility is cached in the
 // running task's `log_columns` (computed once from
 // the audit's mode/strategy at start time) so SSE
 // rows match the server-rendered table columns.
 let cols = {
 let running = sse_state.running.read().await;
 running.get(&audit_ev.audit_id)
 .map(|r| r.log_columns)
 .unwrap_or(templates::LogColumns { show_downloaded: true, show_left: true, show_download_speed: true })
 };
 let html = render::render_log_row_from_audit(audit_ev, &cols);
 (sse::EV_AUDIT, serde_json::to_string(&serde_json::json!({
 "audit_id": audit_ev.audit_id,
 "seq": audit_ev.seq,
 "timestamp": audit_ev.timestamp,
 "phase": audit_ev.phase,
 "client": audit_ev.client,
 "event": audit_ev.event,
 "uploaded": audit_ev.uploaded,
 "downloaded": audit_ev.downloaded,
 "left": audit_ev.left,
 "success": audit_ev.success,
 "failure_reason": audit_ev.failure_reason,
 "interval": audit_ev.interval,
 "seeders": audit_ev.seeders,
 "leechers": audit_ev.leechers,
 "peer_count": audit_ev.peer_count,
 "latency_ms": audit_ev.latency_ms,
 "working_client": audit_ev.working_client,
 "fair_share_bps": audit_ev.fair_share_bps,
 "dynamic_target_bps": audit_ev.dynamic_target_bps,
 "next_announce_in_secs": audit_ev.next_announce_in_secs,
 "html": html,
 })))
 }
 AppEvent::TaskCreated { task } => {
 // Pre-render the task <tr> HTML so the JS never
 // builds task-row HTML - single source of truth.
 let html = render::render_task_row(task, false);
 (sse::EV_TASK_CREATED, serde_json::to_string(&serde_json::json!({
 "id": task.id,
 "name": task.name,
 "tracker": task.tracker,
 "working_client": task.working_client,
 "status": task.status,
 "created_at": task.created_at,
 "uploaded": task.uploaded,
 "downloaded": task.downloaded,
 "mode": task.mode,
 "strategy": task.strategy,
 "goal": task.goal,
 "html": html,
 })))
 }
 AppEvent::TaskDeleted { id } => (sse::EV_TASK_DELETED, serde_json::to_string(&serde_json::json!({ "id": id }))),
 AppEvent::TaskStatus { id, status } => (sse::EV_TASK_STATUS, serde_json::to_string(&serde_json::json!({ "id": id, "status": status }))),
 AppEvent::TaskClient { id, working_client } => (sse::EV_TASK_CLIENT, serde_json::to_string(&serde_json::json!({ "id": id, "working_client": working_client }))),
 AppEvent::TaskProgress { id, uploaded, downloaded } => (sse::EV_TASK_PROGRESS, serde_json::to_string(&serde_json::json!({ "id": id, "uploaded": uploaded, "downloaded": downloaded }))),
 AppEvent::TaskUpdated { task } => (sse::EV_TASK_UPDATED, serde_json::to_string(task)),
 AppEvent::ConfigReloaded { config } => (sse::EV_CONFIG_RELOADED, serde_json::to_string(config.as_ref())),
 AppEvent::CaptureProgress { token, status, fingerprint } => (sse::EV_CAPTURE_PROGRESS, serde_json::to_string(&serde_json::json!({ "token": token, "status": status, "fingerprint": fingerprint }))),
 AppEvent::GoalProgress { id, uploaded, downloaded, up_bps, down_bps, eta_secs } => (sse::EV_GOAL_PROGRESS, serde_json::to_string(&serde_json::json!({ "id": id, "uploaded": uploaded, "downloaded": downloaded, "up_bps": up_bps, "down_bps": down_bps, "eta_secs": eta_secs }))),
 AppEvent::GoalCreated { id } => (sse::EV_GOAL_CREATED, serde_json::to_string(&serde_json::json!({ "id": id }))),
 AppEvent::GoalDeleted { id } => (sse::EV_GOAL_DELETED, serde_json::to_string(&serde_json::json!({ "id": id }))),
 AppEvent::GoalUpdated { id } => (sse::EV_GOAL_UPDATED, serde_json::to_string(&serde_json::json!({ "id": id }))),
 };
 let sse_ev = Event::default().event(name).data(json.unwrap_or_default());
 if sse_tx.send(Ok(sse_ev)).await.is_err() {
 break;
 }
 }
 Err(broadcast::error::RecvError::Lagged(_)) => continue,
 Err(broadcast::error::RecvError::Closed) => break,
 }
 }
 });
 // Keep the idle SSE stream alive so intermediaries (proxies, load
 // balancers, browsers) don't drop a quiet connection during periods with
 // no running audits. A comment frame is ignored by the EventSource
 // parser. The interval is tunable via `server.sse_keepalive_secs`.
 let cfg = state.config.load();
 Sse::new(ReceiverStream::new(sse_rx))
 .keep_alive(KeepAlive::new().interval(Duration::from_secs(cfg.server.sse_keepalive_secs)))
}

/// POST /api/parse-torrent - parse a .torrent file body, return metadata JSON.
pub async fn parse_torrent(body: axum::body::Bytes) -> impl IntoResponse {
 match torrent::parse(&body) {
 Ok(meta) => {
 let json = serde_json::json!({
 "announce_url": meta.announce_url,
 "info_hash": crate::bencode::hex_encode(&meta.info_hash),
 "torrent_size": meta.total_size,
 "name": meta.name,
 });
 Json(json).into_response()
 }
 Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
 }
}

/// POST /api/parse-magnet - parse a magnet link, return metadata JSON.
pub async fn parse_magnet(body: String) -> impl IntoResponse {
 match magnet::parse(&body) {
 Ok(meta) => {
 let json = serde_json::json!({
 "announce_url": meta.announce_url,
 "info_hash": crate::bencode::hex_encode(&meta.info_hash),
 "torrent_size": meta.total_size,
 "name": meta.name,
 });
 Json(json).into_response()
 }
 Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
 }
}

// Global goal handlers

#[derive(serde::Deserialize)]
pub struct CreateGoal {
 #[serde(flatten)]
 pub row: db::GoalRowInput,
 #[serde(default)]
 pub task_ids: Vec<i64>,
}

/// Validate goal fields against the wire-name vocab + unit bounds. Reuses the
/// same constants as `config::DefaultsConfig::validate` and
/// `engine::GoalConfig::validate` - single source of truth.
fn validate_goal_fields(name: &str, direction: &str, reached_action: &str,
 upload_target: u64, download_target: u64, target_secs: u64,
) -> Result<(), String> {
 if name.trim().is_empty() {
 return Err("name must not be empty".into());
 }
 let valid_dirs = [crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE, crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE];
 if !valid_dirs.contains(&direction) {
 return Err(format!("direction must be one of {:?}, got {:?}", valid_dirs, direction));
 }
 let valid_actions = [crate::data::vocab::GOAL_REACHED_STOP_WIRE, crate::data::vocab::GOAL_REACHED_CONTINUE_INITIAL_WIRE, crate::data::vocab::GOAL_REACHED_CONTINUE_CUSTOM_WIRE];
 if !valid_actions.contains(&reached_action) {
 return Err(format!("reached_action must be one of {:?}, got {:?}", valid_actions, reached_action));
 }
 if upload_target > crate::data::units::GOAL_MAX_TARGET_BYTES {
 return Err(format!("upload_target must be <= {}", crate::data::units::GOAL_MAX_TARGET_BYTES));
 }
 if download_target > crate::data::units::GOAL_MAX_TARGET_BYTES {
 return Err(format!("download_target must be <= {}", crate::data::units::GOAL_MAX_TARGET_BYTES));
 }
 if target_secs > crate::data::units::GOAL_MAX_TIME_SECS {
 return Err(format!("target_secs must be <= {}", crate::data::units::GOAL_MAX_TIME_SECS));
 }
 let tracks_dl = direction == crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE;
 let has_target = if tracks_dl {
 upload_target > 0 && download_target > 0
 } else {
 upload_target > 0
 };
 if !has_target && target_secs == 0 {
 return Err("goal must have a target or time".into());
 }
 Ok(())
}

/// Sum uploaded/downloaded across a goal's associated running tasks. Returns
/// (total_uploaded, total_downloaded, total_up_bps, total_down_bps, max_elapsed_secs).
async fn sum_goal_counters(state: &SharedState, goal_id: i64) -> (u64, u64, u64, u64, u64) {
 let pool = state.pool.load_full();
 let task_ids = db::get_goal_task_ids(&pool, goal_id).await.unwrap_or_default();
 let running = state.running.read().await;
 let mut up = 0u64;
 let mut dl = 0u64;
 let mut up_bps = 0u64;
 let mut dl_bps = 0u64;
 let mut max_elapsed = 0u64;
 for &tid in &task_ids {
 let peer = db::get_peer_state(&pool, tid).await.unwrap_or_default();
 up += peer.uploaded;
 dl += peer.downloaded;
 max_elapsed = max_elapsed.max(peer.elapsed_secs);
 if let Some(r) = running.get(&tid) {
 up_bps += r.last_up_bps;
 dl_bps += r.last_down_bps;
 }
 }
 (up, dl, up_bps, dl_bps, max_elapsed)
}

/// Compute the ETA for a global goal from its summed counters + speeds.
fn goal_eta_secs(target: u64, current: u64, bps: u64) -> Option<u64> {
 if target == 0 { return None; }
 let remaining = target.saturating_sub(current);
 if remaining == 0 { return Some(0); }
 if bps == 0 { return None; }
 Some(remaining.div_ceil(bps))
}

/// Compute the binding ETA for a goal. Falls back to the deadline countdown
/// (target_secs - max_elapsed) when no target-based ETA is available. The
/// binding ETA is the minimum of the target-based ETA and the deadline
/// countdown (whichever comes first).
fn goal_binding_eta(up_eta: Option<u64>, dl_eta: Option<u64>, target_secs: u64, max_elapsed: u64) -> Option<u64> {
 let target_eta = match (up_eta, dl_eta) {
 (Some(a), Some(b)) => Some(a.max(b)),
 (Some(a), None) => Some(a),
 (None, Some(b)) => Some(b),
 (None, None) => None,
 };
 let deadline_eta = (target_secs > 0).then(|| target_secs.saturating_sub(max_elapsed));
 match (target_eta, deadline_eta) {
 (Some(a), Some(b)) => Some(a.min(b)),
 (Some(a), None) => Some(a),
 (None, Some(b)) => Some(b),
 (None, None) => None,
 }
}

/// Compute + broadcast goal progress for one goal.
async fn broadcast_goal_progress(state: &SharedState, goal_id: i64) {
 let goal = match db::get_goal(&state.pool.load_full(), goal_id).await {
 Ok(Some(g)) => g,
 _ => return,
 };
 if !goal.enabled { return; }
 let (up, dl, up_bps, dl_bps, max_elapsed) = sum_goal_counters(state, goal_id).await;
 let tracks_up = goal.direction == crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE || goal.direction == crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE;
 let tracks_dl = goal.direction == crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE;
 let up_eta = if tracks_up { goal_eta_secs(goal.upload_target, up, up_bps) } else { None };
 let dl_eta = if tracks_dl { goal_eta_secs(goal.download_target, dl, dl_bps) } else { None };
 let binding_eta = goal_binding_eta(up_eta, dl_eta, goal.target_secs, max_elapsed);
 let _ = state.events_tx.send(AppEvent::GoalProgress {
 id: goal_id, uploaded: up, downloaded: dl,
 up_bps, down_bps: dl_bps, eta_secs: binding_eta,
 });
}

/// Broadcast progress for every goal that includes `task_id`.
async fn broadcast_goals_for_task(state: &SharedState, task_id: i64) {
 let goal_ids = db::goal_ids_for_task(&state.pool.load_full(), task_id).await.unwrap_or_default();
 for gid in goal_ids {
 broadcast_goal_progress(state, gid).await;
 }
}

/// Build topbar tiles for every enabled global goal - server-rendered on the
/// initial page load. The JS patches them live from `goal_progress` SSE events.
async fn build_global_goal_tiles(state: &SharedState) -> Vec<render::GlobalGoalTile> {
 let goals = db::list_goals(&state.pool.load_full()).await.unwrap_or_default();
 let mut tiles = Vec::new();
 for g in goals.iter().filter(|g| g.enabled) {
 let (up, dl, up_bps, dl_bps, max_elapsed) = sum_goal_counters(state, g.id).await;
 let tracks_up = g.direction == crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE || g.direction == crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE;
 let tracks_dl = g.direction == crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE;
 let up_eta = if tracks_up { goal_eta_secs(g.upload_target, up, up_bps) } else { None };
 let dl_eta = if tracks_dl { goal_eta_secs(g.download_target, dl, dl_bps) } else { None };
 let binding_eta = goal_binding_eta(up_eta, dl_eta, g.target_secs, max_elapsed);
 let eta_str = match binding_eta {
 Some(secs) => crate::data::units::fmt_duration(secs),
 None => crate::data::labels::EMPTY_DASH.to_string(),
 };
 tiles.push(render::GlobalGoalTile { id: g.id, name: g.name.clone(), eta: eta_str });
 }
 tiles
}

pub async fn list_goals_json(State(state): State<SharedState>) -> Json<Vec<serde_json::Value>> {
 let pool = state.pool.load_full();
 let goals = db::list_goals(&pool).await.unwrap_or_default();
 let mut result = Vec::with_capacity(goals.len());
 for g in goals {
 let task_ids = db::get_goal_task_ids(&pool, g.id).await.unwrap_or_default();
 let mut val = serde_json::to_value(&g).unwrap_or_default();
 val["task_ids"] = serde_json::json!(task_ids);
 result.push(val);
 }
 Json(result)
}

pub async fn get_goal(State(state): State<SharedState>, Path(id): Path<i64>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let pool = state.pool.load_full();
 let goal = db::get_goal(&pool, id).await.map_err(|e| server_err(e, "get_goal"))?
 .ok_or((StatusCode::NOT_FOUND, "goal not found".into()))?;
 let task_ids = db::get_goal_task_ids(&pool, id).await.unwrap_or_default();
 Ok(Json(serde_json::json!({ "goal": goal, "task_ids": task_ids })))
}

/// Validate that none of `task_ids` are already associated with another goal.
/// `exclude_goal_id` is the goal being edited (0 for create - no goal to
/// exclude). At least one task must be selected.
async fn validate_goal_task_ids(pool: &sqlx::SqlitePool, exclude_goal_id: i64, task_ids: &[i64]) -> Result<(), String> {
 if task_ids.is_empty() {
 return Err("at least one task must be selected".into());
 }
 let occupied = db::occupied_tasks(pool, exclude_goal_id).await.map_err(|e| e.to_string())?;
 let occupied_set: std::collections::HashSet<i64> = occupied.into_iter().collect();
 let conflicts: Vec<i64> = task_ids.iter().filter(|t| occupied_set.contains(t)).copied().collect();
 if !conflicts.is_empty() {
 return Err(format!("Tasks already associated with another goal: {:?}", conflicts));
 }
 Ok(())
}

pub async fn create_goal(
 State(state): State<SharedState>,
 Json(body): Json<CreateGoal>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 validate_goal_fields(&body.row.name, &body.row.direction, &body.row.reached_action, body.row.upload_target, body.row.download_target, body.row.target_secs)
 .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
 let pool = state.pool.load_full();
 validate_goal_task_ids(&pool, 0, &body.task_ids).await
 .map_err(|e| (StatusCode::CONFLICT, e))?;
 let id = db::insert_goal(&pool, &body.row)
 .await.map_err(|e| server_err(e, "insert_goal"))?;
 db::set_goal_tasks(&pool, id, &body.task_ids).await.map_err(|e| server_err(e, "set_goal_tasks"))?;
 let _ = state.events_tx.send(AppEvent::GoalCreated { id });
 Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn update_goal(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
 Json(body): Json<CreateGoal>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 validate_goal_fields(&body.row.name, &body.row.direction, &body.row.reached_action, body.row.upload_target, body.row.download_target, body.row.target_secs)
 .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
 let pool = state.pool.load_full();
 db::get_goal(&pool, id).await.map_err(|e| server_err(e, "get_goal"))?
 .ok_or((StatusCode::NOT_FOUND, "goal not found".into()))?;
 validate_goal_task_ids(&pool, id, &body.task_ids).await
 .map_err(|e| (StatusCode::CONFLICT, e))?;
 db::update_goal(&pool, id, &body.row)
 .await.map_err(|e| server_err(e, "update_goal"))?;
 db::set_goal_tasks(&pool, id, &body.task_ids).await.map_err(|e| server_err(e, "set_goal_tasks"))?;
 let _ = state.events_tx.send(AppEvent::GoalUpdated { id });
 broadcast_goal_progress(&state, id).await;
 Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn delete_goal(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let pool = state.pool.load_full();
 db::delete_goal(&pool, id).await.map_err(|e| server_err(e, "delete_goal"))?;
 let _ = state.events_tx.send(AppEvent::GoalDeleted { id });
 Ok(Json(serde_json::json!({ "id": id, "deleted": true })))
}

pub async fn get_goal_tasks(State(state): State<SharedState>, Path(id): Path<i64>) -> Json<Vec<i64>> {
 let pool = state.pool.load_full();
 Json(db::get_goal_task_ids(&pool, id).await.unwrap_or_default())
}

pub async fn set_goal_tasks_handler(
 State(state): State<SharedState>,
 Path(id): Path<i64>,
 Json(task_ids): Json<Vec<i64>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
 let pool = state.pool.load_full();
 db::set_goal_tasks(&pool, id, &task_ids).await.map_err(|e| server_err(e, "set_goal_tasks"))?;
 let _ = state.events_tx.send(AppEvent::GoalUpdated { id });
 broadcast_goal_progress(&state, id).await;
 Ok(Json(serde_json::json!({ "id": id })))
}

/// `GET /html/goals` - returns the pre-rendered goals table HTML fragment.
/// Mirrors `list_audits_html`: the server is the single source of truth for
/// goal-row HTML; the JS never builds it. Used by `loadGoalList` after a
/// reconnect or goal_created/goal_deleted/goal_updated SSE event.
pub async fn list_goals_html(State(state): State<SharedState>) -> Html<String> {
 let pool = state.pool.load_full();
 let goals = db::list_goals(&pool).await.unwrap_or_default();
 Html(render::render_goals_table(&goals))
}

// Capture handlers

/// Start a fingerprint capture session.
///
/// Reads the `Host` header to determine the server's reachable address,
/// generates a dummy `.torrent` with our tracker URL, and returns it as a
/// download. The `X-Capture-Token` response header carries the session token
/// for the UI to poll the capture status.
pub async fn start_capture(
 State(state): State<SharedState>,
 headers: axum::http::HeaderMap,
) -> impl IntoResponse {
 let cfg = state.config.load();
 // ServerConfig::validate guarantees bind_addr parses as a SocketAddr, so it
 // always carries a port - no fallback literal needed. The single source of
 // truth for the HTTP port is server.bind_addr in config.toml.
 let bind_port = cfg
 .server
 .bind_addr
 .rsplit_once(':')
 .map(|(_, p)| p)
 .expect("server.bind_addr validated as SocketAddr");
 let default_host = format!("127.0.0.1:{bind_port}");
 let host = headers
 .get(axum::http::header::HOST)
 .and_then(|h| h.to_str().ok())
 .unwrap_or(&default_host);
 // Capture-torrent announce port: the port the BT client reached us on (Host
 // header), falling back to the configured bind port.
 let announce_port = host.rsplit_once(':').map(|(_, p)| p).unwrap_or(bind_port);
 let nat_mapping = state.nat.load_full();
 // Advertised peer port: the NAT public port if NAT-PMP is active, else the
 // internal port from config (config.tracker.peer_port is the internal bind
 // port; the override is computed at consumption, not stored in config).
 let peer_port = nat_mapping
 .as_ref()
 .map(|m| m.public_port)
 .unwrap_or(cfg.tracker.peer_port);
 // Two separate IPs:
 // - announce_ip: used in the torrent's announce URL. Must be reachable from
 // the BT client. When accessing from 127.0.0.1, this is 127.0.0.1.
 // - peer_ip: advertised in the tracker response as the peer for the BT client
 // to connect to. Must be non-loopback so BT clients don't skip it as a
 // self-connection. Falls back to announce_ip if no LAN IP is available.
 let host_ip = capture::parse_ip_from_host(host);
 let announce_ip = nat_mapping
 .as_ref()
 .map(|m| m.public_ip)
 .or(host_ip)
 .unwrap_or(std::net::Ipv4Addr::LOCALHOST);
 let lan_ip = {
 let gateway = cfg.nat.gateway_ip.trim().parse().ok();
 let ip = capture::detect_lan_ipv4(gateway);
 tracing::debug!(lan_ip = ?ip, "detected LAN IP for peer advertisement");
 ip
 };
 let peer_ip = nat_mapping
 .as_ref()
 .map(|m| m.public_ip)
 .or(lan_ip)
 .unwrap_or(announce_ip);
 // Generate a unique torrent name from a random peer_id - looks like a real
 // BitTorrent download to the client. Uses the first configured client's
 // peer_id prefix so the capture torrent's name is consistent with the
 // emulated client (config guarantees at least one client).
 let capture_prefix = cfg.clients.first().map(|c| c.peer_id_prefix.as_str()).unwrap_or("");
 let random_peer_id = crate::peer_id::generate_peer_id(capture_prefix);
 let torrent_name = String::from_utf8_lossy(&random_peer_id).to_string();
 let (token, torrent_bytes) =
 state
 .capture_store
 .start(announce_port, announce_ip, peer_ip, peer_port, &torrent_name, random_peer_id);
 tracing::info!(token = %token, host, %announce_ip, %peer_ip, peer_port, "capture session started");

 let mut resp_headers = axum::http::HeaderMap::new();
 resp_headers.insert(
 axum::http::header::CONTENT_TYPE,
 crate::data::protocol::MIME_BITTORRENT.parse().unwrap(),
 );
 resp_headers.insert(
 axum::http::header::CONTENT_DISPOSITION,
 format!("attachment; filename=\"{token}.torrent\"").parse().unwrap(),
 );
 if let Ok(hv) = token.parse() {
 resp_headers.insert(crate::data::protocol::X_CAPTURE_TOKEN, hv);
 }
 (resp_headers, torrent_bytes)
}

/// Tracker announce endpoint for capture sessions.
///
/// The real client announces here. We record `peer_id`, `User-Agent`,
/// `numwant`, and the raw query-param order, then respond with a bencoded
/// peers dict containing our IP + peer_port so the client connects to our
/// peer_server for the wire handshake.
pub async fn capture_announce(
 State(state): State<SharedState>,
 Path(token): Path<String>,
 OriginalUri(uri): OriginalUri,
 req_headers: axum::http::HeaderMap,
) -> Response {
 let session = match state.capture_store.get_by_token(&token) {
 Some(s) => s,
 None => {
 tracing::debug!(token = %token, "capture announce: unknown token");
 return StatusCode::NOT_FOUND.into_response();
 }
 };

 let raw_query = uri.query().unwrap_or("");
 let params = capture::parse_query_params(raw_query);

 let mut peer_id = [0u8; crate::data::protocol::PEER_ID_LEN];
 let mut numwant = None;
 let query_param_order: Vec<String> = params.iter().map(|(k, _)| k.clone()).collect();

 for (key, value) in &params {
 match key.as_str() {
 "peer_id" => {
 let decoded = capture::url_decode(value);
 if decoded.len() == crate::data::protocol::PEER_ID_LEN {
 peer_id.copy_from_slice(&decoded);
 }
 }
 "numwant" => {
 numwant = value.parse().ok();
 }
 _ => {}
 }
 }

 let user_agent = req_headers
 .get(axum::http::header::USER_AGENT)
 .and_then(|h| h.to_str().ok())
 .unwrap_or("")
 .to_string();

 // Capture all HTTP headers for the fingerprint (order preserved)
 let http_headers: Vec<(String, String)> = req_headers
 .iter()
 .filter_map(|(name, value)| {
 let name_s = name.as_str().to_string();
 let value_s = value.to_str().ok()?.to_string();
 Some((name_s, value_s))
 })
 .collect();

 let recorded = state.capture_store.record_announce(
 &token,
 capture::AnnounceData {
 peer_id,
 user_agent: user_agent.clone(),
 query_param_order,
 raw_query: raw_query.to_string(),
 numwant,
 http_headers,
 },
 );
 if recorded {
 tracing::info!(
 token = %token,
 peer_id_prefix = ?String::from_utf8_lossy(&peer_id[..8]),
 user_agent = %user_agent,
 numwant,
 "capture: announce recorded"
 );
 } else {
 tracing::debug!(token = %token, "capture: announce rejected - session already locked");
 }

 let cfg = state.config.load();
 let interval = cfg.tracker.default_interval_secs as u64;
 let response_bytes = capture::build_tracker_response(&session, interval);
 tracing::debug!(
 token = %token,
 peer_ip = %session.peer_ip,
 peer_port = session.peer_port,
 response_len = response_bytes.len(),
 "capture: sending tracker response with peer"
 );

 (
 StatusCode::OK,
 [(axum::http::header::CONTENT_TYPE, crate::data::protocol::MIME_BITTORRENT)],
 response_bytes,
 )
 .into_response()
}

/// Scrape endpoint for capture sessions (BEP-48).
///
/// Some clients (Transmission) send a scrape request after announce.
/// Returns a minimal valid scrape response so the client doesn't error.
pub async fn capture_scrape(
 Path(_token): Path<String>,
) -> Response {
 let body = crate::data::protocol::MINIMAL_SCRAPE_RESPONSE.to_vec();
 (
 StatusCode::OK,
 [(axum::http::header::CONTENT_TYPE, crate::data::protocol::MIME_BITTORRENT)],
 body,
 )
 .into_response()
}

/// Poll the status and captured fingerprint of a capture session.
///
/// Returns JSON with the capture status and any fields captured so far.
/// The UI polls this endpoint to show progress and the final fingerprint.
pub async fn capture_status(
 State(state): State<SharedState>,
 Path(token): Path<String>,
) -> Response {
 let status = state.capture_store.get_status(&token);
 let fingerprint = state.capture_store.get_fingerprint(&token);

 match (status, fingerprint) {
 (Some(status), Some(fp)) => {
 let view = fp.to_view();
 Json(serde_json::json!({
 "status": status,
 "fingerprint": view,
 }))
 .into_response()
 }
 _ => StatusCode::NOT_FOUND.into_response(),
 }
}

/// Delete a capture session (cleanup after the user is done viewing the
/// fingerprint).
pub async fn delete_capture(
 State(state): State<SharedState>,
 Path(token): Path<String>,
) -> StatusCode {
 state.capture_store.remove(&token);
 tracing::info!(token = %token, "capture session removed");
 StatusCode::NO_CONTENT
}

/// Build the axum [`Router`] from the shared state. Extracted so the main
/// rebind loop can rebuild the app (with the same state) after a hot-reload
/// of `server.bind_addr` triggers a graceful rebind.
pub fn router(state: SharedState) -> Router {
 Router::new()
 // Pages
 .route("/", get(index))

 // Bootstrap & SSE
 .route("/api/bootstrap", get(bootstrap))
 .route(crate::data::sse::EVENTS_ROUTE, get(sse_global))

 // Tasks (audits)
 .route("/api/audits", get(list_audits_json).post(create_audit))
 .route("/html/audits", get(list_audits_html))
 .route("/api/audits/{id}", get(get_audit).put(update_audit).delete(delete_audit))
 .route("/api/audits/{id}/start", post(start_audit))
 .route("/api/audits/{id}/stop", post(stop_audit))
 .route("/api/audits/{id}/log", get(audit_log_json))
 .route("/html/audits/{id}/log", get(audit_log_html))

 // Goals
 .route("/api/goals", get(list_goals_json).post(create_goal))
 .route("/html/goals", get(list_goals_html))
 .route("/api/goals/{id}", get(get_goal).put(update_goal).delete(delete_goal))
 .route("/api/goals/{id}/tasks", get(get_goal_tasks).put(set_goal_tasks_handler))

 // Settings & clients
 .route("/api/settings", get(get_settings).put(update_settings))
 .route("/api/clients", get(list_clients))

 // Torrent parsing
 .route("/api/parse-torrent", post(parse_torrent))
 .route("/api/parse-magnet", post(parse_magnet))

 // Capture
 .route("/api/capture/start", post(start_capture))
 .route("/api/capture/{token}", get(capture_status).delete(delete_capture))
 .route(crate::data::protocol::CAPTURE_ANNOUNCE_PATH, get(capture_announce))
 .route(crate::data::protocol::CAPTURE_SCRAPE_PATH, get(capture_scrape))

 // Static files
 .nest_service("/static", tower_http::services::ServeDir::new("frontend"))
 .layer(middleware::from_fn(cache_control_layer))
 .layer(CompressionLayer::new())
 .with_state(state)
}

/// Returns the `Cache-Control` directive for a path, or `None` if the route is
/// exempt (the SSE stream carries no Cache-Control - FRONTEND.md §2). Extracted
/// as a pure function so the per-route policy is unit-testable without spinning
/// up a router.
fn cache_control_for_path(path: &str) -> Option<&'static str> {
 // The global SSE stream is a long-lived connection; Cache-Control is
 // nonsensical and forbidden by FRONTEND.md §2.
 if path == crate::data::sse::EVENTS_ROUTE {
 return None;
 }
 if path == "/" {
 // HTML document: always revalidate (it carries the bundle fingerprint).
 Some(crate::data::protocol::CACHE_NO_CACHE)
 } else if path.starts_with("/static/bundle.") && path.ends_with(".js") {
 // Fingerprinted bundle: URL changes on content change → cache forever.
 Some(crate::data::protocol::CACHE_IMMUTABLE)
 } else if path.starts_with("/static/") {
 // Non-fingerprinted assets (favicon, og image): revalidate
 // every visit via Last-Modified → 304 when unchanged.
 Some(crate::data::protocol::CACHE_NO_CACHE)
 } else {
 Some(crate::data::protocol::CACHE_NO_CACHE)
 }
}

/// Cache-Control middleware - sets per-route caching policy so browsers cache
/// aggressively when safe and revalidate when content may have changed.
///
/// - **`/` (HTML document)**: `no-cache` - always revalidate. The HTML carries
///   the current bundle fingerprint; it must be fresh on every visit so the
///   browser picks up the new `<script src>` when the JS is rebuilt.
/// - **`/static/bundle.<hash>.js`**: `public, max-age=31536000, immutable` - the
///   content hash in the filename guarantees the URL changes iff the content
///   changes, so it's safe to cache forever.
/// - **Other `/static/*` (favicon, images)**: `no-cache` - revalidate
///   via `Last-Modified` (ServeDir sets it automatically). The browser gets a
///   free 304 when the file hasn't changed, and a fresh copy when it has.
/// - **`/api/events` (SSE stream)**: exempt - no `Cache-Control` is set. The
///   stream is a long-lived connection; caching is nonsensical (FRONTEND.md §2).
async fn cache_control_layer(
 OriginalUri(uri): OriginalUri,
 request: axum::extract::Request,
 next: middleware::Next,
) -> Response {
 let path = uri.path();
 let mut response = next.run(request).await;
 if let Some(cache_control) = cache_control_for_path(path) {
 response
 .headers_mut()
 .insert(header::CACHE_CONTROL, cache_control.parse().unwrap());
 }
 response
}

// Contract tests
//
// These tests encode the HTML/JSON contract between the backend handlers and
// the frontend JS. If a template change silently breaks what the JS expects
// (e.g., a wrapper element is removed, a data attribute renamed, a `<table>`
// omitted on the empty path), the corresponding test fails - catching the
// "fetch succeeds, DOM update silently no-ops" bug class that is invisible
// in the browser console.

#[cfg(test)]
mod contract_tests {
 use super::*;
 use crate::config::test_helpers;

 async fn test_state() -> SharedState {
 let pool = db::connect("sqlite::memory:", 2).await.expect("in-memory db");
 let (events_tx, _) = broadcast::channel::<AppEvent>(crate::config::BROADCAST_CHANNEL_CAPACITY);
 Arc::new(AppState {
 pool: ArcSwap::from_pointee(pool),
 running: Default::default(),
 config: ArcSwap::from_pointee(test_helpers::app_config()),
 events_tx: events_tx.clone(),
 peer_server: ArcSwap::from_pointee(crate::peer_server::PeerServer::disabled(crate::capture::CaptureStore::new(events_tx.clone()))),
 capture_store: crate::capture::CaptureStore::new(events_tx),
 nat: ArcSwapOption::new(None),
 log_reload: Box::new(|_: &str| {}),
 rebind_notify: Arc::new(tokio::sync::Notify::new()),
 })
 }

 fn test_audit_config() -> AuditConfig {
 let cfg = crate::config::test_helpers::app_config();
 AuditConfig::from_defaults(&cfg.defaults, &cfg.swarm_defaults)
 }

 fn create_body(name: &str) -> CreateAudit {
 CreateAudit {
 name: name.into(),
 announce_url: "http://tracker.example.com/announce".into(),
 info_hash: crate::data::fixtures::SAMPLE_INFO_HASH.into(),
 torrent_size: 1_073_741_824,
 config: test_audit_config(),
 }
 }

 async fn create_one(state: &SharedState, name: &str) -> i64 {
 let Json(v) = create_audit(State(state.clone()), Json(create_body(name)))
 .await
 .expect("create_audit should succeed");
 v["id"].as_i64().expect("response must contain numeric id")
 }

 // cache_control_for_path

 #[test]
 fn cache_control_exempts_sse_stream() {
 // Regression: /api/events must carry NO Cache-Control (FRONTEND.md §2).
 // Before the exemption the middleware set no-cache on every route.
 assert_eq!(cache_control_for_path(crate::data::sse::EVENTS_ROUTE), None);
 }

 #[test]
 fn cache_control_html_document_is_no_cache() {
 assert_eq!(cache_control_for_path("/"), Some(crate::data::protocol::CACHE_NO_CACHE));
 }

 #[test]
 fn cache_control_fingerprinted_bundle_is_immutable() {
 assert_eq!(
 cache_control_for_path("/static/bundle.abc123def456.js"),
 Some(crate::data::protocol::CACHE_IMMUTABLE)
 );
 }

 #[test]
 fn cache_control_other_static_assets_are_no_cache() {
 assert_eq!(
 cache_control_for_path("/static/favicon.svg"),
 Some(crate::data::protocol::CACHE_NO_CACHE)
 );
 }

 #[test]
 fn cache_control_api_routes_are_no_cache() {
 assert_eq!(
 cache_control_for_path("/api/audits"),
 Some(crate::data::protocol::CACHE_NO_CACHE)
 );
 }

 #[test]
 fn cache_control_bundle_path_without_js_suffix_is_not_immutable() {
 // A path that starts with "/static/bundle." but doesn't end in ".js"
 // is not the fingerprinted bundle - it gets no-cache, not immutable.
 assert_eq!(
 cache_control_for_path("/static/bundle.css"),
 Some(crate::data::protocol::CACHE_NO_CACHE)
 );
 }

 // build_config_rows

 #[test]
 fn build_config_rows_download_and_upload_fixed_shows_core_fields() {
 let config = AuditConfig {
 mode: crate::engine::Mode::DownloadAndUpload,
 speed_mode: crate::engine::SpeedMode::Fixed,
 ..test_audit_config()
 };
 let rows = build_config_rows(&config);
 let labels_found: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
 assert!(labels_found.contains(&"Mode"), "must show mode");
 assert!(labels_found.contains(&"Strategy"), "must show strategy");
 assert!(labels_found.contains(&"Upload speed"), "must show upload speed");
 assert!(labels_found.contains(&"Download speed"), "must show download speed in D+U");
 assert!(labels_found.contains(&"Jitter"), "must show jitter");
 assert!(labels_found.contains(&"Ramp-up"), "must show ramp-up");
 assert!(labels_found.contains(&"Start pct"), "must show start pct in D+U");
 assert!(labels_found.contains(&"Freeze 0 leechers"), "must show freeze leechers");
 assert!(labels_found.contains(&"Freeze 0 seeders"), "must show freeze seeders in D+U");
 assert!(!labels_found.contains(&"Swarm multiplier"), "swarm multiplier irrelevant in Fixed");
 assert!(!labels_found.contains(&"Max upload"), "max upload irrelevant in Fixed");
 assert!(!labels_found.contains(&"Max download"), "max download irrelevant in Fixed");
 }

 #[test]
 fn build_config_rows_upload_only_hides_download_artifacts() {
 let config = AuditConfig {
 mode: crate::engine::Mode::UploadOnly,
 speed_mode: crate::engine::SpeedMode::Fixed,
 ..test_audit_config()
 };
 let rows = build_config_rows(&config);
 let labels_found: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
 assert!(labels_found.contains(&"Upload speed"), "must show upload speed");
 assert!(!labels_found.contains(&"Download speed"), "download speed irrelevant in Upload only");
 assert!(!labels_found.contains(&"Start pct"), "start pct irrelevant in Upload only");
 assert!(!labels_found.contains(&"Freeze 0 seeders"), "freeze seeders irrelevant in Upload only");
 }

 #[test]
 fn build_config_rows_dynamic_shows_swarm_fields() {
 let config = AuditConfig {
 speed_mode: crate::engine::SpeedMode::Dynamic,
 ..test_audit_config()
 };
 let rows = build_config_rows(&config);
 let labels_found: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
 assert!(labels_found.contains(&"Swarm multiplier"), "must show swarm multiplier in Dynamic");
 assert!(labels_found.contains(&"Max upload"), "must show max upload in Dynamic");
 assert!(labels_found.contains(&"Max download"), "must show max download in Dynamic");
 }

 // GET /api/audits/{id}/log (JSON)

 #[tokio::test]
 async fn audit_log_json_includes_columns_and_config_rows() {
 let state = test_state().await;
 let id = create_one(&state, "JSON Log Test").await;
 let Json(v) = audit_log_json(State(state), Path(id)).await.unwrap();
 // columns
 assert!(v["columns"].is_object(), "must include columns object");
 assert_eq!(v["columns"]["show_downloaded"].as_bool(), Some(true), "D+U default shows downloaded");
 assert_eq!(v["columns"]["show_left"].as_bool(), Some(true), "D+U default shows left");
 assert_eq!(v["columns"]["show_download_speed"].as_bool(), Some(true), "D+U default shows download speed");
 // audit_info with config_rows
 assert!(v["audit_info"].is_object(), "must include audit_info object");
 assert_eq!(v["audit_info"]["name"].as_str().unwrap(), "JSON Log Test");
 let rows = v["audit_info"]["config_rows"].as_array().expect("config_rows must be an array");
 let labels_found: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap()).collect();
 assert!(labels_found.contains(&"Mode"), "config_rows must include mode");
 assert!(labels_found.contains(&"Upload speed"), "config_rows must include upload speed");
 assert!(labels_found.contains(&"Download speed"), "config_rows must include download speed in D+U");
 }

 #[tokio::test]
 async fn audit_log_json_upload_only_hides_download_columns() {
 let state = test_state().await;
 let cfg = AuditConfig {
 mode: crate::engine::Mode::UploadOnly,
 ..test_audit_config()
 };
 let body = CreateAudit {
 name: "Upload only JSON".into(),
 announce_url: "http://t.com/a".into(),
 info_hash: crate::data::fixtures::SAMPLE_INFO_HASH.into(),
 torrent_size: 1_073_741_824,
 config: cfg,
 };
 let Json(v) = create_audit(State(state.clone()), Json(body)).await.unwrap();
 let id = v["id"].as_i64().unwrap();
 let Json(log) = audit_log_json(State(state), Path(id)).await.unwrap();
 assert_eq!(log["columns"]["show_downloaded"].as_bool(), Some(false), "upload_only must hide downloaded");
 assert_eq!(log["columns"]["show_left"].as_bool(), Some(false), "upload_only must hide left");
 assert_eq!(log["columns"]["show_download_speed"].as_bool(), Some(false), "upload_only must hide download speed");
 let rows = log["audit_info"]["config_rows"].as_array().expect("config_rows must be an array");
 let labels_found: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap()).collect();
 assert!(!labels_found.contains(&"Download speed"), "upload_only config_rows must omit download speed");
 assert!(!labels_found.contains(&"Start pct"), "upload_only config_rows must omit start pct");
 }

 #[tokio::test]
 async fn audit_log_json_dynamic_shows_swarm_config_rows() {
 let state = test_state().await;
 let cfg = AuditConfig {
 speed_mode: crate::engine::SpeedMode::Dynamic,
 ..test_audit_config()
 };
 let body = CreateAudit {
 name: "Dynamic JSON".into(),
 announce_url: "http://t.com/a".into(),
 info_hash: crate::data::fixtures::SAMPLE_INFO_HASH.into(),
 torrent_size: 1_073_741_824,
 config: cfg,
 };
 let Json(v) = create_audit(State(state.clone()), Json(body)).await.unwrap();
 let id = v["id"].as_i64().unwrap();
 let Json(log) = audit_log_json(State(state), Path(id)).await.unwrap();
 let rows = log["audit_info"]["config_rows"].as_array().expect("config_rows must be an array");
 let labels_found: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap()).collect();
 assert!(labels_found.contains(&"Swarm multiplier"), "Dynamic config_rows must include swarm multiplier");
 assert!(labels_found.contains(&"Max upload"), "Dynamic config_rows must include max upload");
 assert!(labels_found.contains(&"Max download"), "Dynamic config_rows must include max download");
 }

 // POST /api/audits
 /// `startAudit` does `if (!data.id) return;` - response must contain a
 /// numeric `id`.
 #[tokio::test]
 async fn create_returns_numeric_id() {
 let state = test_state().await;
 let id = create_one(&state, "Id Contract").await;
 assert!(id > 0, "id must be a positive number, got {id}");
 }

 // GET /api/clients
 /// `refreshClientDropdown` (config_reloaded SSE handler) fetches this and
 /// repopulates `#cfg-client`. It must return the current client labels as
 /// a JSON array, in config order, so added/renamed clients appear live.
 #[tokio::test]
 async fn list_clients_returns_current_labels() {
 let state = test_state().await;
 let Json(pairs) = list_clients(State(state.clone())).await;
 assert_eq!(pairs, vec![("-TC0000-".to_string(), "Test Client - 1.0 (-TC0000-)".to_string())]);
 }

 /// After a config reload swaps in a new client list, `list_clients` must
 /// reflect it immediately (no restart) - this is the "all settings hot"
 /// contract the frontend relies on.
 #[tokio::test]
 async fn list_clients_reflects_reloaded_config() {
 let state = test_state().await;
 // Swap in a config with a different client label.
 let mut cfg = state.config.load_full().as_ref().clone();
 cfg.clients[0].label = "Reloader".into();
 cfg.clients[0].version = "2.0".into();
 state.config.store(std::sync::Arc::new(cfg));
 let Json(pairs) = list_clients(State(state)).await;
 assert_eq!(pairs, vec![("-TC0000-".to_string(), "Reloader - 2.0 (-TC0000-)".to_string())]);
 }

 // GET /api/audits/{id}
 /// `editAudit` reads `data.config` as a structured object (not display
 /// strings) to populate the form inputs. The response must include the
 /// raw config with wire-format enum values (snake_case).
 #[tokio::test]
 async fn get_audit_returns_raw_config() {
 let state = test_state().await;
 let id = create_one(&state, "Get Audit Test").await;
 let Json(v) = get_audit(State(state), Path(id)).await.unwrap();
 assert_eq!(v["id"].as_i64().unwrap(), id);
 assert_eq!(v["name"].as_str().unwrap(), "Get Audit Test");
 assert!(v["config"].is_object(), "config must be a structured object; got: {}", v["config"]);
 // Wire-format enum values (snake_case), not display strings like "D+U"
 assert_eq!(v["config"]["mode"].as_str().unwrap(), "download_and_upload");
 assert_eq!(v["config"]["speed_mode"].as_str().unwrap(), "dynamic");
 }

 #[tokio::test]
 async fn get_audit_nonexistent_returns_404() {
 let state = test_state().await;
 let err = get_audit(State(state), Path(9999)).await.unwrap_err();
 assert_eq!(err.0, StatusCode::NOT_FOUND);
 }

 // PUT /api/audits/{id}
 #[tokio::test]
 async fn update_audit_changes_config() {
 let state = test_state().await;
 let id = create_one(&state, "Edit Test").await;
 let base = AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults);
 let body = UpdateAudit {
 config: AuditConfig {
 mode: crate::engine::Mode::UploadOnly,
 speed_mode: crate::engine::SpeedMode::Dynamic,
 ..base
 },
 };
 let Json(v) = update_audit(State(state.clone()), Path(id), Json(body)).await.unwrap();
 assert_eq!(v["id"].as_i64().unwrap(), id);
 assert!(!v["unchanged"].as_bool().unwrap_or(false), "changed config must not be unchanged");
 // Verify the config was persisted
 let Json(v2) = get_audit(State(state), Path(id)).await.unwrap();
 assert_eq!(v2["config"]["mode"].as_str().unwrap(), "upload_only");
 assert_eq!(v2["config"]["speed_mode"].as_str().unwrap(), "dynamic");
 }

 #[tokio::test]
 async fn update_audit_unchanged_is_no_op() {
 let state = test_state().await;
 let id = create_one(&state, "Unchanged Edit Test").await;
 // Seed peer state + working client so we can detect a reset.
 db::save_peer_state(&state.pool.load_full(), id, db::SavePeerState {
 uploaded: 1_000_000, downloaded: 500_000, left: 250_000,
 lifecycle_phase: "leech", completed_sent: false, elapsed_secs: 10,
 peer_id: "2d7142353232302dabcdef0123456789abcdef01", key: "DEADBEEF",
 }).await.unwrap();
 db::set_working_client(&state.pool.load_full(), id, "Test Client").await.unwrap();
 // Send the exact stored config (identity is locked by the handler).
 let Json(stored) = get_audit(State(state.clone()), Path(id)).await.unwrap();
 let base: AuditConfig = serde_json::from_value(stored["config"].clone()).unwrap();
 let Json(v) = update_audit(State(state.clone()), Path(id), Json(UpdateAudit { config: base }))
 .await
 .unwrap();
 assert!(v["unchanged"].as_bool().unwrap(), "identical config must be a no-op");
 // Peer state + working client must survive an unchanged edit.
 let peer = db::get_peer_state(&state.pool.load_full(), id).await.unwrap();
 assert_eq!(peer.uploaded, 1_000_000, "unchanged edit must not reset peer state");
 assert_eq!(peer.peer_id.as_deref(), Some("2d7142353232302dabcdef0123456789abcdef01"));
 let row = db::get_audit(&state.pool.load_full(), id).await.unwrap().unwrap();
 assert_eq!(row.working_client.as_deref(), Some("Test Client"));
 }

 #[tokio::test]
 async fn update_audit_changed_resets_peer_state_and_events() {
 let state = test_state().await;
 let id = create_one(&state, "Reset Edit Test").await;
 // Seed peer state + an event so we can detect the wipe.
 db::save_peer_state(&state.pool.load_full(), id, db::SavePeerState {
 uploaded: 1_000_000, downloaded: 500_000, left: 250_000,
 lifecycle_phase: "leech", completed_sent: false, elapsed_secs: 10,
 peer_id: "2d7142353232302dabcdef0123456789abcdef01", key: "DEADBEEF",
 }).await.unwrap();
 db::set_working_client(&state.pool.load_full(), id, "Test Client").await.unwrap();
 // Insert one event row.
 let ev = crate::engine::AuditEvent {
 audit_id: id, seq: 0, timestamp: chrono::Utc::now(),
 phase: crate::data::vocab::PHASE_PROBE, client: "Test".into(),
 event: crate::data::vocab::EVENT_PROBE, uploaded: 0, downloaded: 0,
 left: 0, success: true, failure_reason: None, interval: 1800,
 seeders: 0, leechers: 0, peer_count: 0, latency_ms: 0,
 working_client: None, fair_share_bps: 0, dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 };
 db::insert_event(&state.pool.load_full(), &ev).await.unwrap();
 assert!(!db::list_events(&state.pool.load_full(), id, 100).await.unwrap().is_empty());
 // Send a changed config (mode differs).
 let base = AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults);
 let changed = AuditConfig { mode: crate::engine::Mode::UploadOnly, ..base };
 let Json(v) = update_audit(State(state.clone()), Path(id), Json(UpdateAudit { config: changed }))
 .await
 .unwrap();
 assert!(!v["unchanged"].as_bool().unwrap_or(false));
 // Peer state must be zeroed + peer_id/key cleared.
 let peer = db::get_peer_state(&state.pool.load_full(), id).await.unwrap();
 assert_eq!(peer.uploaded, 0, "changed edit must reset uploaded");
 assert_eq!(peer.downloaded, 0, "changed edit must reset downloaded");
 assert_eq!(peer.left, 0, "changed edit must reset left");
 assert!(peer.peer_id.is_none(), "changed edit must clear peer_id");
 assert!(peer.key.is_none(), "changed edit must clear key");
 // Working client must be cleared.
 let row = db::get_audit(&state.pool.load_full(), id).await.unwrap().unwrap();
 assert!(row.working_client.is_none(), "changed edit must clear working_client");
 // Events must be cleared.
 assert!(db::list_events(&state.pool.load_full(), id, 100).await.unwrap().is_empty(), "changed edit must clear events");
 }

 #[tokio::test]
 async fn update_audit_running_stops_first_then_edits() {
 let state = test_state().await;
 let id = create_one(&state, "Running Edit Test").await;
 // Simulate a running task; the edit handler must cancel it and proceed.
 let cancel = CancellationToken::new();
 state.running.write().await.insert(id, RunningAudit {
 cancel: cancel.clone(),
 done: None,
 log_columns: templates::LogColumns { show_downloaded: true, show_left: true, show_download_speed: true },
 last_up_bps: 0,
 last_down_bps: 0,
 });
 // Send a changed config (mode differs) so the handler stops + resets.
 let base = AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults);
 let changed = AuditConfig { mode: crate::engine::Mode::UploadOnly, ..base };
 let Json(v) = update_audit(State(state.clone()), Path(id), Json(UpdateAudit { config: changed }))
 .await
 .expect("edit should stop the running task and succeed");
 assert_eq!(v["id"].as_i64().unwrap(), id);
 assert!(v["restarted"].as_bool().unwrap(), "running task must be restarted after edit");
 // The cancellation token must have been fired by stop_running_task.
 assert!(cancel.is_cancelled(), "running task must be cancelled before editing");
 }

 #[tokio::test]
 async fn delete_audit_running_stops_first_then_deletes() {
 let state = test_state().await;
 let id = create_one(&state, "Running Delete Test").await;
 // Simulate a running task; the delete handler must cancel it and proceed.
 let cancel = CancellationToken::new();
 state.running.write().await.insert(id, RunningAudit {
 cancel: cancel.clone(),
 done: None,
 log_columns: templates::LogColumns { show_downloaded: true, show_left: true, show_download_speed: true },
 last_up_bps: 0,
 last_down_bps: 0,
 });
 let Json(v) = delete_audit(State(state), Path(id)).await.unwrap();
 assert_eq!(v["id"].as_i64().unwrap(), id);
 assert!(v["deleted"].as_bool().unwrap());
 // The cancellation token must have been fired by stop_running_task.
 assert!(cancel.is_cancelled(), "running task must be cancelled before deleting");
 }

 #[tokio::test]
 async fn start_engine_caches_upload_only_log_columns() {
 let state = test_state().await;
 // Create an audit with UploadOnly mode.
 let mut body = create_body("UploadOnly Cols Test");
 body.config = AuditConfig {
 mode: crate::engine::Mode::UploadOnly,
 ..AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults)
 };
 let Json(v) = create_audit(State(state.clone()), Json(body)).await.expect("create should succeed");
 let id = v["id"].as_i64().unwrap();

 // Start the engine - it inserts a RunningAudit with log_columns.
 assert!(start_engine(&state, id).await.unwrap(), "start_engine should start");

 // The cached log_columns must match UploadOnly (download/left/speed hidden).
 let cols = {
 let running = state.running.read().await;
 running.get(&id).expect("running entry must exist").log_columns
 };
 assert!(!cols.show_downloaded, "UploadOnly must hide downloaded column");
 assert!(!cols.show_left, "UploadOnly must hide left column");
 assert!(!cols.show_download_speed, "UploadOnly must hide download speed column");

 // Clean up: stop the engine task.
 stop_running_task(&state, id).await;
 }

 #[tokio::test]
 async fn start_engine_caches_download_and_upload_log_columns() {
 let state = test_state().await;
 let mut body = create_body("DU Cols Test");
 body.config = AuditConfig {
 mode: crate::engine::Mode::DownloadAndUpload,
 ..AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults)
 };
 let Json(v) = create_audit(State(state.clone()), Json(body)).await.expect("create should succeed");
 let id = v["id"].as_i64().unwrap();

 assert!(start_engine(&state, id).await.unwrap(), "start_engine should start");
 let cols = {
 let running = state.running.read().await;
 running.get(&id).expect("running entry must exist").log_columns
 };
 assert!(cols.show_downloaded, "DownloadAndUpload must show downloaded column");
 assert!(cols.show_left, "DownloadAndUpload must show left column");
 assert!(cols.show_download_speed, "DownloadAndUpload must show download speed column");

 stop_running_task(&state, id).await;
 }

 #[tokio::test]
 async fn start_engine_forced_client_records_working_client() {
 // Regression: when a task forces a specific client, the engine skips
 // probing - but the probe event was the only path that persisted
 // working_client and broadcast it to the UI. So a forced-client task
 // ran fine (the engine used the client) yet the task list showed "-"
 // and the log panel showed "probing..." forever. start_engine must
 // record the working-client key + emit TaskClient when the probe is
 // skipped (forced or resumed client).
 let state = test_state().await;
 let mut body = create_body("Forced Client Test");
 body.config.forced_client = Some("-TC0000-".into());
 let Json(v) = create_audit(State(state.clone()), Json(body)).await.expect("create should succeed");
 let id = v["id"].as_i64().unwrap();

 assert!(start_engine(&state, id).await.unwrap(), "start_engine should start");

 let row = db::get_audit(&state.pool.load_full(), id)
 .await
 .expect("db read ok")
 .expect("audit row exists");
 assert_eq!(
 row.working_client.as_deref(),
 Some("-TC0000-"),
 "forced client must be persisted as working_client (was NULL before fix)"
 );

 stop_running_task(&state, id).await;
 }

 #[tokio::test]
 async fn start_engine_forced_client_broadcasts_task_client_sse() {
 // The live UI patches the client cell from the `task_client` SSE event.
 // When the probe is skipped, start_engine must emit it directly (the
 // engine never emits a probe event to carry it).
 let state = test_state().await;
 let mut rx = state.events_tx.subscribe();
 let mut body = create_body("Forced Client SSE Test");
 body.config.forced_client = Some("-TC0000-".into());
 let Json(v) = create_audit(State(state.clone()), Json(body)).await.expect("create should succeed");
 let id = v["id"].as_i64().unwrap();

 assert!(start_engine(&state, id).await.unwrap(), "start_engine should start");

 // Drain the synchronous events start_engine emitted before spawning
 // the engine task (TaskStatus running + TaskClient). The spawned task
 // may emit more later; we only care that a TaskClient{Some} arrived.
 let mut saw_task_client = false;
 for _ in 0..32 {
 match rx.try_recv() {
 Ok(AppEvent::TaskClient { id: eid, working_client: Some(wc) }) if eid == id => {
 assert_eq!(wc, "-TC0000-", "TaskClient SSE must carry the forced prefix");
 saw_task_client = true;
 }
 Ok(_) => {}
 Err(broadcast::error::TryRecvError::Empty) | Err(broadcast::error::TryRecvError::Closed) => break,
 Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
 }
 }
 assert!(saw_task_client, "start_engine must broadcast TaskClient for forced client");

 stop_running_task(&state, id).await;
 }

 #[tokio::test]
 async fn update_audit_nonexistent_returns_404() {
 let state = test_state().await;
 let base = AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults);
 let err = update_audit(State(state), Path(9999), Json(UpdateAudit { config: base }))
 .await
 .unwrap_err();
 assert_eq!(err.0, StatusCode::NOT_FOUND);
 }

 #[tokio::test]
 async fn update_audit_rejects_invalid_config() {
 let state = test_state().await;
 let id = create_one(&state, "Invalid Edit Test").await;
 let base = AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults);
 // jitter_pct > 100 is invalid - validate() rejects it.
 let body = UpdateAudit {
 config: AuditConfig {
 jitter_pct: 200,
 ..base
 },
 };
 let err = update_audit(State(state), Path(id), Json(body)).await.unwrap_err();
 assert_eq!(err.0, StatusCode::BAD_REQUEST);
 assert!(err.1.contains("jitter_pct"));
 }

 /// The torrent identity (announce_url, info_hash, torrent_size) is locked:
 /// even if the request sends different values, the handler overwrites
 /// them with the stored row's values. This prevents tampering with the
 /// torrent via the edit endpoint. The config also changes a non-identity
 /// field (mode) so the save path is exercised.
 #[tokio::test]
 async fn update_audit_locks_torrent_identity() {
 let state = test_state().await;
 let id = create_one(&state, "Lock Identity Test").await;
 let base = AuditConfig::from_defaults(&state.config.load().defaults, &state.config.load().swarm_defaults);
 let body = UpdateAudit {
 config: AuditConfig {
 announce_url: "http://evil.example.com/announce".into(),
 info_hash: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into(),
 torrent_size: 1,
 mode: crate::engine::Mode::UploadOnly, // force a change so the save runs
 ..base
 },
 };
 let Json(_) = update_audit(State(state.clone()), Path(id), Json(body)).await.unwrap();
 // The stored row must still have the original identity.
 let Json(v) = get_audit(State(state), Path(id)).await.unwrap();
 assert_eq!(v["announce_url"].as_str().unwrap(), "http://tracker.example.com/announce");
 assert_eq!(v["info_hash"].as_str().unwrap(), crate::data::fixtures::SAMPLE_INFO_HASH);
 assert_eq!(v["torrent_size"].as_i64().unwrap(), 1_073_741_824);
 }

 // GET /api/settings
 // The settings modal fetches this to populate every field. It must return
 // the live config as JSON with the same structure the PUT handler accepts
 // (serde round-trip).

 #[tokio::test]
 async fn get_settings_returns_current_config() {
 let state = test_state().await;
 let Json(cfg) = get_settings(State(state)).await;
 assert_eq!(cfg.server.bind_addr, "127.0.0.1:0");
 assert_eq!(cfg.engine.tick_interval_secs, 1);
 assert_eq!(cfg.clients.len(), 1);
 assert_eq!(cfg.clients[0].label, "Test Client");
 assert_eq!(cfg.clients[0].version, "1.0");
 }

 #[tokio::test]
 async fn get_settings_serializes_round_trippable() {
 let state = test_state().await;
 let Json(cfg) = get_settings(State(state)).await;
 let json = serde_json::to_string(&cfg).unwrap();
 let parsed: crate::config::AppConfig = serde_json::from_str(&json).unwrap();
 assert_eq!(cfg, parsed, "GET /api/settings JSON must round-trip through serde");
 }

 // PUT /api/settings
 // These tests manipulate the REDSWARM_CONFIG env var so the handler
 // writes to a temp file instead of the real config.toml. A mutex serializes
 // them to avoid cross-test env-var races.
 static SETTINGS_TEST_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

 fn settings_tmp(label: &str) -> std::path::PathBuf {
 std::env::temp_dir().join(format!("rf_settings_{label}_{}.toml", std::process::id()))
 }

 #[tokio::test]
 async fn put_settings_writes_and_reloads() {
 let _guard = SETTINGS_TEST_LOCK.lock().await;
 let tmp = settings_tmp("ok");
 unsafe { std::env::set_var("REDSWARM_CONFIG", &tmp); }
 let state = test_state().await;
 let mut cfg = state.config.load_full().as_ref().clone();
 cfg.engine.tick_interval_secs = 7; // non-structural change
 let result = update_settings(State(state.clone()), Json(cfg.clone())).await;
 unsafe { std::env::remove_var("REDSWARM_CONFIG"); }
 let _ = std::fs::remove_file(&tmp);
 assert!(result.is_ok(), "update_settings should succeed: {:?}", result.err());
 assert_eq!(state.config.load().engine.tick_interval_secs, 7, "config must be swapped");
 }

 #[tokio::test]
 async fn put_settings_rejects_invalid_config() {
 let _guard = SETTINGS_TEST_LOCK.lock().await;
 let tmp = settings_tmp("bad");
 unsafe { std::env::set_var("REDSWARM_CONFIG", &tmp); }
 let state = test_state().await;
 let mut cfg = state.config.load_full().as_ref().clone();
 cfg.tracker.min_interval_secs = 0; // violates >= 1
 let err = update_settings(State(state), Json(cfg)).await.unwrap_err();
 unsafe { std::env::remove_var("REDSWARM_CONFIG"); }
 let _ = std::fs::remove_file(&tmp);
 assert_eq!(err.0, StatusCode::BAD_REQUEST);
 assert!(err.1.contains("min_interval_secs"), "error must name the bad field; got: {}", err.1);
 // The temp file must NOT have been written (validation runs before save).
 assert!(!tmp.exists(), "invalid config must not be written to disk");
 }

 #[tokio::test]
 async fn put_settings_rejects_duplicate_client_prefix() {
 let _guard = SETTINGS_TEST_LOCK.lock().await;
 let tmp = settings_tmp("dup");
 unsafe { std::env::set_var("REDSWARM_CONFIG", &tmp); }
 let state = test_state().await;
 let mut cfg = state.config.load_full().as_ref().clone();
 let dup = cfg.clients[0].clone();
 cfg.clients.push(dup); // duplicate peer_id_prefix
 let err = update_settings(State(state), Json(cfg)).await.unwrap_err();
 unsafe { std::env::remove_var("REDSWARM_CONFIG"); }
 let _ = std::fs::remove_file(&tmp);
 assert_eq!(err.0, StatusCode::BAD_REQUEST);
 assert!(err.1.contains("duplicated"), "error must mention duplicate prefix; got: {}", err.1);
 }

 #[tokio::test]
 async fn put_settings_noop_does_not_broadcast() {
 let _guard = SETTINGS_TEST_LOCK.lock().await;
 let tmp = settings_tmp("noop");
 unsafe { std::env::set_var("REDSWARM_CONFIG", &tmp); }
 let state = test_state().await;
 let cfg = state.config.load_full().as_ref().clone();
 let mut rx = state.events_tx.subscribe();
 // Save the identical config - the reloader short-circuits, no broadcast.
 let Json(_) = update_settings(State(state.clone()), Json(cfg)).await.unwrap();
 unsafe { std::env::remove_var("REDSWARM_CONFIG"); }
 let _ = std::fs::remove_file(&tmp);
 let saw = tokio::time::timeout(std::time::Duration::from_millis(200), async {
 rx.recv().await
 }).await;
 assert!(saw.is_err(), "no-op save must not broadcast ConfigReloaded");
 }
}
