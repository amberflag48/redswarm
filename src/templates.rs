//! Askama template structs.

use askama::Template;

use crate::data::labels;
use crate::data::units::{fmt_bytes, fmt_duration, fmt_speed_bps, fmt_speed_cell};
use crate::engine::{AuditEvent, Mode, SpeedMode};

/// The Askama template carries pre-built HTML strings (rendered by
/// `render::*`) plus the bootstrap JSON. The template itself is trivial -
/// just injects the strings with `|safe`. All rendering logic lives in
/// testable Rust functions, not in template conditionals.
#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
 pub bootstrap_json: String,
 pub topbar_stats_html: String,
 pub task_list_html: String,
 pub goal_list_html: String,
 pub log_panel_html: String,
 pub byte_unit_options_mib: String,
 pub byte_amount_options_mib: String,
 pub client_dropdown_html: String,
 pub settings_nav_html: String,
 pub settings_panes_html: String,
}

/// Single source of truth for which event-log columns and stats are visible.
///
/// Computed once from the audit's mode/strategy, consumed by both the
/// Askama template (server render) and the JS SSE handlers (live updates
/// via `data-show-*` attributes on `.log-stats`). Adding a new mode or a
/// strategy-dependent column only requires changing `for_config`.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct LogColumns {
 pub show_downloaded: bool,
 pub show_left: bool,
 pub show_download_speed: bool,
}

impl LogColumns {
 /// Compute column/stat visibility from the audit's mode and strategy.
 /// This is the ONLY place that maps mode/strategy → visible columns.
 pub fn for_config(mode: Mode, _speed_mode: SpeedMode) -> Self {
 let upload_only = mode == Mode::UploadOnly;
 Self {
 show_downloaded: !upload_only,
 show_left: !upload_only,
 show_download_speed: !upload_only,
 }
 }
}

#[derive(serde::Serialize)]
pub struct AuditInfoView {
 pub name: String,
 pub status: String,
 pub working_client: Option<String>,
 pub torrent_info: Vec<(String, String)>,
 pub config_rows: Vec<(String, String)>,
 /// The audit's goal config - drives the goal stat tiles (progress / ETA /
 /// required speed) in both the server-rendered panel and the live JS
 /// updater. Copied from `AuditConfig::goal`.
 pub goal: crate::engine::GoalConfig,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventView {
 pub seq: u64,
 pub phase: String,
 pub event: String,
 pub uploaded_display: String,
 pub downloaded_display: String,
 pub left_display: String,
 pub success: bool,
 pub failure_reason: Option<String>,
 pub seeders: i64,
 pub leechers: i64,
 pub fair_share_display: String,
 pub target_speed_display: String,
 /// Pre-formatted speed table cell (e.g. "512.00 KiB/s ↑ 1.00 MiB/s ↓").
 /// Empty when both speeds are zero - no "-" placeholder in the table.
 pub speed_cell_display: String,
 /// Pre-formatted countdown to the next announce (e.g. "4m 30s"), or "-" when 0.
 pub next_announce_display: String,
 /// Raw cumulative uploaded bytes - used by the goal-progress/ETA tiles.
 pub uploaded: u64,
 /// Raw cumulative downloaded bytes - used by the goal-progress/ETA tiles.
 pub downloaded: u64,
 /// Raw achieved upload speed (bytes/sec) - used by the goal ETA tile.
 pub fair_share_bps: u64,
 /// Raw achieved download speed (bytes/sec) - used by the goal ETA tile.
 pub dynamic_target_bps: u64,
 /// Seconds since the task started - used for time-only goal deadline countdown.
 pub elapsed_secs: u64,
}

impl EventView {
 pub fn from_event(ev: &AuditEvent, show_download_speed: bool) -> Self {
 Self {
 seq: ev.seq,
 phase: ev.phase.to_string(),
 event: ev.event.to_string(),
 uploaded_display: fmt_bytes(ev.uploaded),
 downloaded_display: fmt_bytes(ev.downloaded),
 left_display: fmt_bytes(ev.left),
 success: ev.success,
 failure_reason: ev.failure_reason.clone(),
 seeders: ev.seeders,
 leechers: ev.leechers,
 fair_share_display: fmt_speed_bps(ev.fair_share_bps),
 target_speed_display: fmt_speed_bps(ev.dynamic_target_bps),
 speed_cell_display: fmt_speed_cell(ev.fair_share_bps, ev.dynamic_target_bps, show_download_speed),
 next_announce_display: if ev.next_announce_in_secs > 0 { fmt_duration(ev.next_announce_in_secs) } else { labels::EMPTY_DASH.to_string() },
 uploaded: ev.uploaded,
 downloaded: ev.downloaded,
 fair_share_bps: ev.fair_share_bps,
 dynamic_target_bps: ev.dynamic_target_bps,
 elapsed_secs: ev.elapsed_secs,
 }
 }
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::data::units::fmt_bytes_i64;
 use crate::engine::AuditEvent;

 fn base_event() -> AuditEvent {
 AuditEvent {
 audit_id: 1,
 seq: 1,
 timestamp: chrono::Utc::now(),
 phase: "attack",
 client: "Transmission 4.0".into(),
 event: "started",
 uploaded: 0,
 downloaded: 0,
 left: 0,
 success: true,
 failure_reason: None,
 interval: 1800,
 seeders: 1,
 leechers: 1,
 peer_count: 2,
 latency_ms: 10,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 }
 }

 // LogColumns::for_config

 #[test]
 fn log_columns_download_and_upload_shows_everything() {
 let c = LogColumns::for_config(Mode::DownloadAndUpload, SpeedMode::Fixed);
 assert!(c.show_downloaded, "D+U must show Downloaded");
 assert!(c.show_left, "D+U must show Left");
 assert!(c.show_download_speed, "D+U must show download speed");
 }

 #[test]
 fn log_columns_upload_only_hides_download_artifacts() {
 let c = LogColumns::for_config(Mode::UploadOnly, SpeedMode::Dynamic);
 assert!(!c.show_downloaded, "upload_only must hide Downloaded");
 assert!(!c.show_left, "upload_only must hide Left");
 assert!(!c.show_download_speed, "upload_only must hide download speed");
 }

 #[test]
 fn log_columns_strategy_does_not_affect_visibility() {
 // Fixed and Dynamic produce the same column set for a given mode.
 let fixed = LogColumns::for_config(Mode::DownloadAndUpload, SpeedMode::Fixed);
 let dynamic = LogColumns::for_config(Mode::DownloadAndUpload, SpeedMode::Dynamic);
 assert_eq!(fixed.show_downloaded, dynamic.show_downloaded);
 assert_eq!(fixed.show_left, dynamic.show_left);
 assert_eq!(fixed.show_download_speed, dynamic.show_download_speed);
 }

 // fmt_bytes: boundaries

 #[test]
 fn fmt_bytes_zero() { assert_eq!(fmt_bytes(0), "0 B"); }

 #[test]
 fn fmt_bytes_one() { assert_eq!(fmt_bytes(1), "1 B"); }

 #[test]
 fn fmt_bytes_1023() { assert_eq!(fmt_bytes(1023), "1023 B"); }

 #[test]
 fn fmt_bytes_1024() { assert_eq!(fmt_bytes(1024), "1.00 KiB"); }

 #[test]
 fn fmt_bytes_kib_boundary_minus_one() { assert_eq!(fmt_bytes(1_048_575), "1024.00 KiB"); }

 #[test]
 fn fmt_bytes_mib() { assert_eq!(fmt_bytes(1_048_576), "1.00 MiB"); }

 #[test]
 fn fmt_bytes_gib() { assert_eq!(fmt_bytes(1_073_741_824), "1.00 GiB"); }

 #[test]
 fn fmt_bytes_large() { assert_eq!(fmt_bytes(10 * 1_073_741_824), "10.00 GiB"); }

 #[test]
 fn fmt_bytes_u64_max_does_not_panic() {
 let s = fmt_bytes(u64::MAX);
 assert!(s.ends_with("GiB"), "got {s}");
 }

 // fmt_bytes_i64

 #[test]
 fn fmt_bytes_i64_positive() { assert_eq!(fmt_bytes_i64(1024), "1.00 KiB"); }

 #[test]
 fn fmt_bytes_i64_zero() { assert_eq!(fmt_bytes_i64(0), "0 B"); }

 #[test]
 fn fmt_bytes_i64_negative_casts_to_u64_max() {
 assert_eq!(fmt_bytes_i64(-1), fmt_bytes(u64::MAX));
 }

 // EventView::from_event

 #[test]
 fn from_event_with_fair_share_shows_speed() {
 let mut ev = base_event();
 ev.fair_share_bps = 524_288;
 let view = EventView::from_event(&ev, true);
 assert_eq!(view.fair_share_display, "512.00 KiB/s");
 }

 #[test]
 fn from_event_with_zero_fair_share_shows_zero() {
 let ev = base_event();
 let view = EventView::from_event(&ev, true);
 assert_eq!(view.fair_share_display, "0 B/s");
 }

 #[test]
 fn from_event_with_dynamic_target_shows_speed() {
 let mut ev = base_event();
 ev.dynamic_target_bps = 1_048_576;
 let view = EventView::from_event(&ev, true);
 assert_eq!(view.target_speed_display, "1.00 MiB/s");
 }

 #[test]
 fn from_event_with_zero_dynamic_target_shows_zero() {
 let ev = base_event();
 let view = EventView::from_event(&ev, true);
 assert_eq!(view.target_speed_display, "0 B/s");
 }

 #[test]
 fn from_event_with_failure_reason() {
 let mut ev = base_event();
 ev.success = false;
 ev.failure_reason = Some("tracker rejected".into());
 let view = EventView::from_event(&ev, true);
 assert!(!view.success);
 assert_eq!(view.failure_reason.as_deref(), Some("tracker rejected"));
 }

 #[test]
 fn from_event_success_no_failure() {
 let ev = base_event();
 let view = EventView::from_event(&ev, true);
 assert!(view.success);
 assert!(view.failure_reason.is_none());
 }

 // No #[allow(...)] attributes in source code
 //
 // #[allow(...)] silences compiler/clippy warnings instead of fixing the
 // root cause. This test walks every .rs file under src/ and fails if any
 // #[allow(...)] attribute is found. If code is dead, delete it; if clippy
 // warns, fix the code.

 #[test]
 fn no_allow_attributes_in_source() {
 let mut files: Vec<std::path::PathBuf> = Vec::new();
 collect_rs_files(std::path::Path::new("src"), &mut files);
 assert!(!files.is_empty(), "found no .rs files under src/ - wrong CWD?");
 for path in &files {
 let content = std::fs::read_to_string(path)
 .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
 for (lineno, line) in content.lines().enumerate() {
 let trimmed = line.trim_start();
 if trimmed.starts_with("#[allow(") {
 panic!(
 "{}:{} contains an #[allow(...)] attribute - fix the root cause instead of silencing the compiler",
 path.display(),
 lineno + 1
 );
 }
 }
 }
 }

 fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
 let Ok(entries) = std::fs::read_dir(dir) else { return };
 for entry in entries.flatten() {
 let path = entry.path();
 if path.is_dir() {
 collect_rs_files(&path, out);
 } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
 out.push(path);
 }
 }
 }
}
