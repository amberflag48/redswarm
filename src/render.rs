//! Server-side HTML renderers for the task list and log panel.
//!
//! Each function produces the same HTML its JS counterpart does, so the
//! server-rendered initial paint is byte-identical to what the JS would build.
//! The JS then skips re-rendering on first load (it just reads state from the
//! DOM) and only takes over for SSE-driven deltas. Both sides share the same
//! labels (`data::labels`), formatters (`data::units`), CSS classes, and
//! `data-*` hooks - the rendering is a thin layer over the shared data.
//!
//! Tests pin the exact output for representative inputs so a Rust↔JS drift
//! fails loudly.

use crate::data::labels;
use crate::data::units;
use crate::engine::{GoalConfig, GoalDirection, TaskSummary};
use crate::templates::{AuditInfoView, EventView, LogColumns};

/// Render a single log `<tr>` from a raw `AuditEvent` (the SSE wire shape),
/// formatting fields on the fly. Mirrors the JS `appendLogRow` row builder -
/// the only place that should ever build a log `<tr>` HTML string.
pub fn render_log_row_from_audit(
 ev: &crate::engine::AuditEvent,
 cols: &LogColumns,
) -> String {
 let view = crate::templates::EventView::from_event(ev, cols.show_download_speed);
 render_log_row(&view, cols)
}

// Escaping

/// Escape a string for HTML text content. Mirrors the JS `escHtml` - the
/// characters that break out of a text node are `<` `>` `&`. Quotes are safe
/// in text content, so they are left alone (matching the JS behavior).
pub fn esc_html(s: &str) -> String {
 s.replace('&', "&amp;")
 .replace('<', "&lt;")
 .replace('>', "&gt;")
}

/// Escape a string for an HTML attribute value. Mirrors the JS `escAttr` -
/// escapes `&` `"` `<` `>` so the string can't break out of a double-quoted
/// attribute.
pub fn esc_attr(s: &str) -> String {
 s.replace('&', "&amp;")
 .replace('"', "&quot;")
 .replace('<', "&lt;")
 .replace('>', "&gt;")
}

// Topbar stats

/// One global goal's topbar tile view - name + ETA.
#[derive(Debug, Clone)]
pub struct GlobalGoalTile {
 pub id: i64,
 pub name: String,
 /// "-" when no speed known yet; otherwise a fmt_duration string.
 pub eta: String,
}

/// Render the `#topbar-stats` inner HTML. Mirrors the JS `updateTopbar` -
/// empty when there are no tasks and no global goals, running/stopped tiles
/// otherwise, plus one tile per global goal (ETA + name).
pub fn render_topbar_stats(running: usize, stopped: usize, goals: &[GlobalGoalTile]) -> String {
 if running + stopped == 0 && goals.is_empty() {
 return String::new();
 }
 let mut html = format!(
 r#"<div class="stat"><div class="val text-green">{running}</div><div class="lbl">{running_lbl}</div></div><div class="stat"><div class="val text-muted">{stopped}</div><div class="lbl">{stopped_lbl}</div></div>"#,
 running = running,
 running_lbl = crate::data::vocab::STATUS_RUNNING,
 stopped = stopped,
 stopped_lbl = crate::data::vocab::STATUS_STOPPED,
 );
 for g in goals {
 html.push_str(&format!(
 r#"<div class="stat" data-goal-id="{id}"><div class="val">{eta}</div><div class="lbl" title="{name_attr}">{name}</div></div>"#,
 id = g.id, eta = esc_html(&g.eta), name = esc_html(&g.name), name_attr = esc_attr(&g.name),
 ));
 }
 html
}

// Task list

/// Render the `#goal-list` inner HTML - the goals table for the Goals tab.
/// Mirrors `render_task_list`: always contains both the empty placeholder
/// and the table (one hidden via the `hidden` class), so the JS can toggle
/// between them on empty↔non-empty transitions without re-fetching.
pub fn render_goals_table(goals: &[crate::db::GoalRow]) -> String {
 let empty_hidden = if goals.is_empty() { "" } else { " hidden" };
 let table_hidden = if goals.is_empty() { " hidden" } else { "" };
 let rows: String = goals.iter().map(|g| {
 let dir_display = match g.direction.as_str() {
 crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE => "Upload",
 crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE => "D+U",
 _ => "-",
 };
 let targets = if g.upload_target > 0 && g.download_target > 0 {
 format!("{} / {}", units::fmt_bytes(g.upload_target), units::fmt_bytes(g.download_target))
 } else if g.upload_target > 0 {
 units::fmt_bytes(g.upload_target)
 } else if g.download_target > 0 {
 units::fmt_bytes(g.download_target)
 } else {
 "-".to_string()
 };
 let action_display = match g.reached_action.as_str() {
 crate::data::vocab::GOAL_REACHED_STOP_WIRE => labels::GOAL_REACHED_STOP,
 crate::data::vocab::GOAL_REACHED_CONTINUE_INITIAL_WIRE => labels::GOAL_REACHED_CONTINUE_INITIAL,
 crate::data::vocab::GOAL_REACHED_CONTINUE_CUSTOM_WIRE => labels::GOAL_REACHED_CONTINUE_CUSTOM,
 _ => "-",
 };
 format!(
 r#"<tr data-goal-id="{id}"><td class="name-cell">{name}</td><td>{enabled}</td><td>{dir}</td><td class="mono num">{targets}</td><td class="mono num">{secs}</td><td>{action}</td><td class="mono num">{created}</td><td class="actions"><button class="act-edit" data-action="edit-goal" data-id="{id}" aria-label="Edit goal {id}">Edit</button><button class="act-del" data-action="delete-goal" data-id="{id}" aria-label="Delete goal {id}">Delete</button></td></tr>"#,
 id = g.id,
 name = esc_html(&g.name),
 enabled = if g.enabled { "✓" } else { "-" },
 dir = dir_display,
 targets = esc_html(&targets),
 secs = if g.target_secs > 0 { units::fmt_duration(g.target_secs) } else { "ETA only".into() },
 action = esc_html(action_display),
 created = esc_html(&g.created_at),
 )
 }).collect();
 format!(
 r#"<div class="empty{empty_hidden}">{empty_text}</div><table class="task-table{table_hidden}"><thead><tr><th>Name</th><th>Enabled</th><th>Direction</th><th class="num">Target(s)</th><th class="num">Time</th><th>On reached</th><th class="num">Created</th><th>Actions</th></tr></thead><tbody>{rows}</tbody></table>"#,
 empty_text = labels::EMPTY_GOALS,
 empty_hidden = empty_hidden,
 table_hidden = table_hidden,
 rows = rows,
 )
}

// Task list rendering

/// Resolve a `working_client` key to a display name, falling back to an em
/// dash. Mirrors the JS `resolveClientName`.
fn resolve_client_name(working_client: Option<&str>) -> String {
 match working_client {
 Some(c) if !c.is_empty() => c.to_string(),
 _ => labels::EMPTY_DASH.to_string(),
 }
}

/// Render the action buttons for a task row. Mirrors the JS `taskActionsHtml`.
fn render_task_actions(id: i64, status: &str) -> String {
 let mid = if status == crate::data::vocab::STATUS_RUNNING {
 format!(
 r#"<button class="act-stop" data-action="stop" data-id="{id}" aria-label="Stop task {id}">Stop</button>"#,
 id = id,
 )
 } else {
 format!(
 r#"<button class="act-start" data-action="start" data-id="{id}" aria-label="Start task {id}">Start</button>"#,
 id = id,
 )
 };
 format!(
 r#"<button class="act-edit" data-action="edit" data-id="{id}" aria-label="Edit task {id}">Edit</button>{mid}<button class="act-del" data-action="delete" data-id="{id}" aria-label="Delete task {id}">Delete</button>"#,
 id = id,
 mid = mid,
 )
}

/// Render a single task-list `<tr>`. Mirrors the JS `taskRowHtml`.
pub fn render_task_row(task: &TaskSummary, is_active: bool) -> String {
 let client = resolve_client_name(task.working_client.as_deref());
 let active = if is_active { " active" } else { "" };
 // Goal config rides on the row as data-goal-* so the JS topbar aggregator
 // (initGoals) can read it without an extra fetch. Emitted for every row
 // (even disabled goals) so toggling a goal on edit updates the topbar.
 let goal_dir = match task.goal.direction {
 GoalDirection::Upload => crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE,
 GoalDirection::DownloadAndUpload => crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE,
 };
 format!(
 r#"<tr data-id="{id}" class="{cls}" data-info-hash="{ih}" data-announce-url="{au}" data-goal-enabled="{ge}" data-goal-direction="{gd}" data-goal-upload-target="{gut}" data-goal-download-target="{gdt}" data-goal-secs="{gs}"><td class="name-cell" title="{name_attr}">{name}</td><td class="mono" data-label="Tracker" title="{tracker_attr}">{tracker}</td><td data-label="Client" data-col="client">{client}</td><td data-label="Mode" data-col="mode">{mode}</td><td data-label="Strategy" data-col="strategy">{strategy}</td><td class="mono num" data-label="Uploaded" data-col="uploaded">{uploaded}</td><td class="mono num" data-label="Downloaded" data-col="downloaded">{downloaded}</td><td data-label="Status" data-col="status"><span class="badge {status_attr}">{status_text}</span></td><td class="mono num" data-label="Created" title="{created_attr}">{created}</td><td class="actions" data-col="actions">{actions}</td></tr>"#,
 id = task.id,
 cls = active.trim_start(),
 ih = esc_attr(&task.info_hash),
 au = esc_attr(&task.announce_url),
 ge = task.goal.enabled,
 gd = goal_dir,
 gut = task.goal.upload_target,
 gdt = task.goal.download_target,
 gs = task.goal.target_secs,
 name = esc_html(&task.name),
 name_attr = esc_attr(&task.name),
 tracker = esc_html(&task.tracker),
 tracker_attr = esc_attr(&task.tracker),
 client = esc_html(&client),
 mode = esc_html(&task.mode),
 strategy = esc_html(&task.strategy),
 uploaded = units::fmt_bytes(task.uploaded),
 downloaded = units::fmt_bytes(task.downloaded),
 status_attr = esc_attr(&task.status),
 status_text = esc_html(&task.status),
 created = esc_html(&task.created_at),
 created_attr = esc_attr(&task.created_at),
 actions = render_task_actions(task.id, &task.status),
 )
}

/// Render the full `#audit-list` inner HTML - always contains both the empty
/// placeholder and the table (with header + rows), one hidden via the `hidden`
/// class. This lets the JS toggle between them on empty↔non-empty transitions
/// without re-fetching HTML from the server.
pub fn render_task_list(summaries: &[TaskSummary], active_log_id: i64) -> String {
 let empty_hidden = if summaries.is_empty() { "" } else { " hidden" };
 let table_hidden = if summaries.is_empty() { " hidden" } else { "" };
 let rows: String = summaries
 .iter()
 .map(|t| render_task_row(t, t.id == active_log_id))
 .collect();
 format!(
 r#"<div class="empty{empty_hidden}">{empty_text}</div><table class="task-table{table_hidden}"><thead><tr><th>Name</th><th>Tracker</th><th>Client</th><th>Mode</th><th>Strategy</th><th class="num">Uploaded</th><th class="num">Downloaded</th><th>Status</th><th class="num">Created</th><th>Actions</th></tr></thead><tbody>{rows}</tbody></table>"#,
 empty_text = labels::EMPTY_TASKS,
 empty_hidden = empty_hidden,
 table_hidden = table_hidden,
 rows = rows,
 )
}

// Log panel

/// Render a single log `<tr>` from a pre-formatted `EventView`. Mirrors the
/// JS `logRowHtml`.
pub fn render_log_row(ev: &EventView, cols: &LogColumns) -> String {
 let fail_html = match &ev.failure_reason {
 Some(reason) => format!(
 r#"<span class="badge fail" title="{reason}">FAIL</span>"#,
 reason = esc_attr(reason),
 ),
 None => r#"<span class="badge ok">OK</span>"#.to_string(),
 };
 let mut cells = format!(
 r#"<td class="phase-{phase_attr}">{phase}</td><td>{event}</td><td class="mono num">{uploaded}</td>"#,
 phase_attr = esc_attr(&ev.phase),
 phase = esc_html(&ev.phase),
 event = esc_html(&ev.event),
 uploaded = esc_html(&ev.uploaded_display),
 );
 if cols.show_downloaded {
 cells.push_str(&format!(
 r#"<td class="mono num">{}</td>"#,
 esc_html(&ev.downloaded_display)
 ));
 }
 if cols.show_left {
 cells.push_str(&format!(
 r#"<td class="mono num">{}</td>"#,
 esc_html(&ev.left_display)
 ));
 }
 cells.push_str(&format!(
 r#"<td>{fail}</td><td class="mono" title="{seeders} seeders / {leechers} leechers">{seeders}S/{leechers}L</td><td class="mono num">{speed}</td>"#,
 fail = fail_html,
 seeders = ev.seeders,
 leechers = ev.leechers,
 speed = esc_html(&ev.speed_cell_display),
 ));
 format!(r#"<tr data-seq="{seq}">{cells}</tr>"#, seq = ev.seq, cells = cells)
}

/// Render the `.log-stats` strip. Mirrors the stats section of the JS
/// `renderLog`. Each tile carries a `data-stat` key consumed by the JS
/// `updateLogStats` per-tile updater. When `goal` is `Some` and enabled, three
/// goal tiles (progress / ETA / required speed) are appended - the JS keeps
/// them live from the `audit` SSE event.
pub fn render_log_stats(
 events: &[EventView],
 cols: &LogColumns,
 total_uploaded: &str,
 success_count: usize,
 goal: Option<&GoalConfig>,
) -> String {
 let has = !events.is_empty();
 let e0 = events.first();
 let phase = if has { e0.unwrap().phase.as_str() } else { labels::EMPTY_DASH };
 let uploaded = if has { total_uploaded } else { &units::fmt_bytes(0) };
 let upload = if has { &e0.unwrap().fair_share_display } else { &units::fmt_speed_bps(0) };
 let seeders = if has { e0.unwrap().seeders } else { 0 };
 let leechers = if has { e0.unwrap().leechers } else { 0 };
 let success = if has {
 format!("{}/{}", success_count, events.len())
 } else {
 "0/0".to_string()
 };
 let next = if has {
 &e0.unwrap().next_announce_display
 } else {
 labels::EMPTY_DASH
 };

 let phase_cls = if has {
 format!("phase-{}", esc_attr(phase))
 } else {
 String::new()
 };

 let mut html = format!(
 r#"<div class="stat" data-stat="phase"><div class="val {phase_cls}">{phase}</div><div class="lbl">phase</div></div><div class="stat" data-stat="uploaded"><div class="val">{uploaded}</div><div class="lbl">uploaded</div></div><div class="stat" data-stat="upload"><div class="val">{upload}</div><div class="lbl">upload</div></div>"#,
 phase_cls = phase_cls,
 phase = esc_html(phase),
 uploaded = esc_html(uploaded),
 upload = esc_html(upload),
 );
 if cols.show_download_speed {
 let dl = if has {
 &e0.unwrap().target_speed_display
 } else {
 &units::fmt_speed_bps(0)
 };
 html.push_str(&format!(
 r#"<div class="stat" data-stat="download"><div class="val">{}</div><div class="lbl">download</div></div>"#,
 esc_html(dl)
 ));
 }
 html.push_str(&format!(
 r#"<div class="stat" data-stat="seeders"><div class="val">{seeders}</div><div class="lbl">seeders</div></div><div class="stat" data-stat="leechers"><div class="val">{leechers}</div><div class="lbl">leechers</div></div><div class="stat" data-stat="success"><div class="val">{success}</div><div class="lbl">success</div></div><div class="stat" data-stat="next-announce"><div class="val">{next}</div><div class="lbl">next announce</div></div>"#,
 seeders = seeders,
 leechers = leechers,
 success = esc_html(&success),
 next = esc_html(next),
 ));

 // Goal tiles: per-direction progress + binding ETA + required speed. Only
 // when enabled with at least one non-zero target OR a non-zero deadline.
 // The JS `updateLogStats` patches these live from the `audit` SSE event;
 // the server renders the initial snapshot here. In DownloadAndUpload mode
 // two progress tiles (goal-up + goal-dl) are emitted and the ETA is the
 // max of the two. For time-only goals (no target, just deadline), only the
 // ETA tile is emitted (countdown from deadline).
 if let Some(goal) = goal.filter(|g| g.enabled && (g.upload_target > 0 || g.download_target > 0 || g.target_secs > 0)) {
 let ev0 = e0.cloned();
 let up_t = crate::engine::goal_upload_target(goal);
 let dl_t = crate::engine::goal_download_target(goal);

 // Per-direction ETA (0 = reached; None = no speed → "-").
 let dir_eta = |target: u64, current: u64, speed: u64| -> Option<u64> {
 if target == 0 { return None; }
 let remaining = target.saturating_sub(current);
 if remaining == 0 { return Some(0); }
 (speed != 0).then(|| remaining.div_ceil(speed))
 };
 let up_eta = match ev0.as_ref() {
 Some(e) => dir_eta(up_t, e.uploaded, e.fair_share_bps),
 None => dir_eta(up_t, 0, 0),
 };
 let dl_eta = match ev0.as_ref() {
 Some(e) => dir_eta(dl_t, e.downloaded, e.dynamic_target_bps),
 None => dir_eta(dl_t, 0, 0),
 };

 // Progress tiles: one per tracked direction.
 if up_t > 0 {
 let cur = ev0.as_ref().map(|e| e.uploaded).unwrap_or(0);
 html.push_str(&format!(
 r#"<div class="stat" data-stat="goal-up"><div class="val">{progress}</div><div class="lbl">up</div></div>"#,
 progress = esc_html(&format!("{} / {}", units::fmt_bytes(cur), units::fmt_bytes(up_t))),
 ));
 }
 if dl_t > 0 {
 let cur = ev0.as_ref().map(|e| e.downloaded).unwrap_or(0);
 html.push_str(&format!(
 r#"<div class="stat" data-stat="goal-dl"><div class="val">{progress}</div><div class="lbl">dl</div></div>"#,
 progress = esc_html(&format!("{} / {}", units::fmt_bytes(cur), units::fmt_bytes(dl_t))),
 ));
 }

 // Binding ETA = min of target-based ETA and deadline countdown,
 // whichever comes first. For time-only goals (no target), only the
 // deadline countdown applies.
 let target_eta = match (up_eta, dl_eta) {
 (Some(a), Some(b)) => Some(a.max(b)),
 (Some(a), None) => Some(a),
 (None, Some(b)) => Some(b),
 (None, None) => None,
 };
 let elapsed = ev0.as_ref().map(|e| e.elapsed_secs).unwrap_or(0);
 let deadline_eta = (goal.target_secs > 0).then(|| goal.target_secs.saturating_sub(elapsed));
 let binding_eta = match (target_eta, deadline_eta) {
 (Some(a), Some(b)) => Some(a.min(b)),
 (Some(a), None) => Some(a),
 (None, Some(b)) => Some(b),
 (None, None) => None,
 };
 let eta_str = match binding_eta {
 Some(0) => units::fmt_duration(0),
 Some(secs) => units::fmt_duration(secs),
 None => labels::EMPTY_DASH.to_string(),
 };
 html.push_str(&format!(
 r#"<div class="stat" data-stat="goal-eta"><div class="val">{eta}</div><div class="lbl">{eta_lbl}</div></div>"#,
 eta = esc_html(&eta_str),
 eta_lbl = labels::GOAL_ETA_LABEL,
 ));
 // Required (planned average) speed - reverse mode only (target_secs > 0).
 // Shows the upload target's average rate (the primary ratio-building
 // counter).
 if let Some(required) = (goal.target_secs != 0 && up_t > 0).then(|| up_t / goal.target_secs) {
 html.push_str(&format!(
 r#"<div class="stat" data-stat="goal-required"><div class="val">{req}</div><div class="lbl">{req_lbl}</div></div>"#,
 req = esc_html(&units::fmt_speed_bps(required)),
 req_lbl = labels::GOAL_REQUIRED_LABEL,
 ));
 }
 }
 html
}

/// Render the full `#log-panel` inner HTML - audit info + stats + table.
/// Mirrors the JS `renderLog`.
pub fn render_log_panel(
 events: &[EventView],
 audit_info: Option<&AuditInfoView>,
 cols: &LogColumns,
 total_uploaded: &str,
 success_count: usize,
) -> String {
 let mut html = String::new();

 // Audit info panel
 if let Some(ai) = audit_info.filter(|ai| !ai.name.is_empty()) {
 html.push_str(r#"<div class="audit-info">"#);
 html.push_str(&format!(
 r#"<div class="audit-info-head"><strong class="audit-info-name">{name}</strong><span class="badge {status}" data-col="audit-status">{status}</span></div>"#,
 name = esc_html(&ai.name),
 status = esc_attr(&ai.status),
 ));
 let client = match ai.working_client.as_deref() {
 Some(c) if !c.is_empty() => c.to_string(),
 None if ai.status == crate::data::vocab::STATUS_RUNNING => "probing...".to_string(),
 _ => labels::EMPTY_DASH.to_string(),
 };
 html.push_str(&format!(
 r#"<div class="audit-info-client-row"><span class="text-muted">Client:</span> <span data-col="audit-client">{client}</span></div>"#,
 client = esc_html(&client),
 ));
 if !ai.torrent_info.is_empty() {
 html.push_str(r#"<div class="info-grid">"#);
 for (k, v) in &ai.torrent_info {
 html.push_str(&format!(
 r#"<div><span class="text-muted">{k}:</span> {v}</div>"#,
 k = esc_html(k),
 v = esc_html(v),
 ));
 }
 html.push_str("</div>");
 }
 if !ai.config_rows.is_empty() {
 html.push_str(r#"<div class="config-rows"><div class="config-rows-header">Configuration</div><div class="info-grid">"#);
 for (k, v) in &ai.config_rows {
 html.push_str(&format!(
 r#"<div><span class="text-muted">{k}:</span> {v}</div>"#,
 k = esc_html(k),
 v = esc_html(v),
 ));
 }
 html.push_str("</div></div>");
 }
 html.push_str("</div>");
 }

 // Stats strip
 let goal = audit_info.map(|ai| &ai.goal);
 html.push_str(&format!(
 r#"<div class="log-stats" data-show-downloaded="{sd}" data-show-left="{sl}" data-show-download-speed="{ss}">{stats}</div>"#,
 sd = cols.show_downloaded,
 sl = cols.show_left,
 ss = cols.show_download_speed,
 stats = render_log_stats(events, cols, total_uploaded, success_count, goal),
 ));

 // Table
 html.push_str(r#"<div class="table-wrap"><table><thead><tr><th>Phase</th><th>Event</th><th class="num">Uploaded</th>"#);
 if cols.show_downloaded {
 html.push_str(r#"<th class="num">Downloaded</th>"#);
 }
 if cols.show_left {
 html.push_str("<th>Left</th>");
 }
 html.push_str(r#"<th>Result</th><th>Swarm</th><th class="num">Speed</th></tr></thead><tbody>"#);
 // Newest-first (the DB returns newest-first via ORDER BY DESC). SSE
 // prepends new rows at the top of the <tbody> to maintain newest-first
 // order. The stats strip always reflects the latest event (events[0]).
 for ev in events {
 html.push_str(&render_log_row(ev, cols));
 }
 html.push_str("</tbody></table></div>");

 if events.is_empty() {
 html.push_str(&format!(r#"<div class="empty">{}</div>"#, labels::EMPTY_EVENTS));
 }

 html
}

// Byte-unit select options

/// Render `<option>` elements for a speed-unit `<select>` (e.g. `KiB/s`).
/// `selected` is the unit value (1024, 1048576, etc.) that should be
/// pre-selected. Used by speed fields (upload/download speed, max caps,
/// goal reached speed). Mirrors the JS `byteUnitOptions`.
pub fn render_byte_unit_options(selected: &str) -> String {
 use crate::data::units::{BYTE_UNIT_B, BYTE_UNIT_KIB, BYTE_UNIT_MIB, BYTE_UNIT_GIB, UNIT_B, UNIT_KIB, UNIT_MIB, UNIT_GIB, speed_unit_label};
 let units: &[(u64, String)] = &[
 (BYTE_UNIT_B, speed_unit_label(UNIT_B)),
 (BYTE_UNIT_KIB, speed_unit_label(UNIT_KIB)),
 (BYTE_UNIT_MIB, speed_unit_label(UNIT_MIB)),
 (BYTE_UNIT_GIB, speed_unit_label(UNIT_GIB)),
 ];
 units.iter().map(|(v, label)| {
 let sel = if v.to_string() == selected { " selected" } else { "" };
 format!(r#"<option value="{v}"{sel}>{label}</option>"#, v = v, sel = sel, label = label)
 }).collect()
}

/// Render `<option>` elements for a byte-amount `<select>` (e.g. `MiB`, no
/// `/s`). Used by goal-target fields, which are total byte counts, not
/// speeds. `selected` is the unit value that should be pre-selected. Mirrors
/// the JS `byteAmountOptions`.
pub fn render_byte_amount_options(selected: &str) -> String {
 use crate::data::units::{BYTE_UNIT_B, BYTE_UNIT_KIB, BYTE_UNIT_MIB, BYTE_UNIT_GIB, UNIT_B, UNIT_KIB, UNIT_MIB, UNIT_GIB};
 let units: &[(u64, &str)] = &[
 (BYTE_UNIT_B, UNIT_B),
 (BYTE_UNIT_KIB, UNIT_KIB),
 (BYTE_UNIT_MIB, UNIT_MIB),
 (BYTE_UNIT_GIB, UNIT_GIB),
 ];
 units.iter().map(|(v, label)| {
 let sel = if v.to_string() == selected { " selected" } else { "" };
 format!(r#"<option value="{v}"{sel}>{label}</option>"#, v = v, sel = sel, label = label)
 }).collect()
}

// Client dropdown

/// Render `<option>` elements for the `#cfg-client` dropdown. Includes the
/// "Auto (probe all)" option at the top. Mirrors the JS `refreshClientDropdown`.
pub fn render_client_dropdown(clients: &[(String, String)]) -> String {
 let mut html = r#"<option value="">Auto (probe all)</option>"#.to_string();
 for (value, display) in clients {
 html.push_str(&format!(
 r#"<option value="{val}">{disp}</option>"#,
 val = esc_attr(value),
 disp = esc_html(display),
 ));
 }
 html
}

// Settings modal

// A settings field definition - mirrors the JS `SETTINGS_SCHEMA` entry.
struct SettingsField {
 key: &'static str,
 label: &'static str,
 ty: &'static str, // "text" | "number" | "bool" | "select"
 hint: &'static str,
 req: bool,
 min: Option<&'static str>,
 max: Option<&'static str>,
 step: Option<&'static str>,
 options: &'static [(&'static str, &'static str)], // for "select"
}

/// The settings sections - mirrors the JS `SETTINGS_SECTIONS`.
const SETTINGS_SECTIONS: &[(&str, &str)] = &[
 ("server", "Server"),
 ("tracker", "Tracker"),
 ("engine", "Engine"),
 ("defaults", "Defaults"),
 ("swarm_defaults", "Swarm"),
 ("peer_server", "Peer server"),
 ("clients", "Clients"),
];

const MODE_OPTIONS: &[(&str, &str)] = &[
 (crate::data::vocab::MODE_DU_WIRE, labels::MODE_DU_FULL),
 (crate::data::vocab::MODE_UO_WIRE, labels::MODE_UO_FULL),
];

const STRATEGY_OPTIONS: &[(&str, &str)] = &[
 (crate::data::vocab::SPEED_FIXED_WIRE, labels::STRATEGY_FIXED),
 (crate::data::vocab::SPEED_DYNAMIC_WIRE, labels::STRATEGY_DYNAMIC),
];

const GOAL_DIRECTION_OPTIONS: &[(&str, &str)] = &[
 (crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE, labels::GOAL_DIRECTION_UPLOAD),
 (crate::data::vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE, labels::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD),
];

const GOAL_REACHED_ACTION_OPTIONS: &[(&str, &str)] = &[
 (crate::data::vocab::GOAL_REACHED_STOP_WIRE, labels::GOAL_REACHED_STOP),
 (crate::data::vocab::GOAL_REACHED_CONTINUE_INITIAL_WIRE, labels::GOAL_REACHED_CONTINUE_INITIAL),
 (crate::data::vocab::GOAL_REACHED_CONTINUE_CUSTOM_WIRE, labels::GOAL_REACHED_CONTINUE_CUSTOM),
];

const SERVER_FIELDS: &[SettingsField] = &[
 SettingsField { key: "bind_addr", label: "Bind address", ty: "text", hint: "Socket address for the web UI", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "db_url", label: "Database URL", ty: "text", hint: "SQLx connection string", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "log_filter", label: "Log filter", ty: "text", hint: "tracing env-filter directive", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "rebind_retry_secs", label: "Rebind retry (s)", ty: "number", hint: "Seconds to wait before retrying bind on failure", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "sse_keepalive_secs", label: "SSE keepalive (s)", ty: "number", hint: "Interval for SSE keep-alive frames (prevents idle connection drops)", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "timeout_secs", label: "HTTP timeout (s)", ty: "number", hint: "Tracker HTTP request timeout", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "max_connections", label: "DB max connections", ty: "number", hint: "SQLite connection pool size", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "event_log_limit", label: "Event log limit", ty: "number", hint: "Max events stored per audit", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "gateway_ip", label: "NAT gateway IP", ty: "text", hint: "For NAT-PMP port mapping - empty = disabled", req: false, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "debounce_ms", label: "Config watcher debounce (ms)", ty: "number", hint: "Delay before hot-reloading after config.toml changes", req: true, min: Some("1"), max: Some(units::DEBOUNCE_MS_MAX_STR), step: Some("1"), options: &[] },
];

const TRACKER_FIELDS: &[SettingsField] = &[
 SettingsField { key: "peer_port", label: "Peer port", ty: "number", hint: "Port advertised in tracker announces", req: true, min: Some("1"), max: Some(crate::data::protocol::MAX_PORT_STR), step: Some("1"), options: &[] },
 SettingsField { key: "default_interval_secs", label: "Default interval (s)", ty: "number", hint: "Announce interval when tracker specifies none", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "min_interval_secs", label: "Min interval (s)", ty: "number", hint: "Minimum announce interval enforced locally", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "max_interval_secs", label: "Max interval (s)", ty: "number", hint: "Maximum announce interval (clamps tracker values)", req: true, min: Some("2"), max: None, step: Some("1"), options: &[] },
];

const ENGINE_FIELDS: &[SettingsField] = &[
 SettingsField { key: "tick_interval_secs", label: "Tick interval (s)", ty: "number", hint: "Core engine loop tick - drives announce scheduling", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "stat_interval_secs", label: "Stat interval (s)", ty: "number", hint: "How often stats are computed and stored", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "announce_jitter_pct", label: "Announce jitter %", ty: "number", hint: "Random jitter added to announce interval to avoid sync", req: true, min: Some("0"), max: Some(units::PERCENT_STR), step: Some("0.1"), options: &[] },
 SettingsField { key: "leech_upload_factor", label: "Leech upload factor", ty: "number", hint: "Fraction of upload speed used while leeching", req: true, min: Some("0"), max: Some("1"), step: Some("0.05"), options: &[] },
 SettingsField { key: "burst_choke_probability", label: "Burst choke probability", ty: "number", hint: "Chance of choking a peer each tick in burst mode", req: true, min: Some("0"), max: Some("1"), step: Some("0.05"), options: &[] },
 SettingsField { key: "stop_grace_secs", label: "Stop grace (s)", ty: "number", hint: "Grace period before force-stopping an audit", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
];

const DEFAULTS_FIELDS: &[SettingsField] = &[
 SettingsField { key: "mode", label: "Mode", ty: "select", hint: "Download+upload leeches then seeds; upload-only skips download", req: true, min: None, max: None, step: None, options: MODE_OPTIONS },
 SettingsField { key: "speed_mode", label: "Speed mode", ty: "select", hint: "Fixed = constant speed; dynamic = adapts to swarm", req: true, min: None, max: None, step: None, options: STRATEGY_OPTIONS },
 SettingsField { key: "upload_bps", label: "Upload (B/s)", ty: "number", hint: "Upload speed in bytes per second", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "download_bps", label: "Download (B/s)", ty: "number", hint: "Download speed in bytes per second", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "jitter_pct", label: "Jitter %", ty: "number", hint: "Random variation applied to speeds each tick", req: true, min: Some("0"), max: Some(units::PERCENT_STR), step: Some("1"), options: &[] },
 SettingsField { key: "ramp_up_secs", label: "Ramp up (s)", ty: "number", hint: "Seconds to ramp from 0 to target speed", req: true, min: Some("0"), max: Some(units::SECS_PER_DAY_STR), step: Some("1"), options: &[] },
 SettingsField { key: "start_download_pct", label: "Start download %", ty: "number", hint: "Initial download progress (0 = from scratch, 100 = seeder)", req: true, min: Some("0"), max: Some(units::PERCENT_STR), step: Some("1"), options: &[] },
 SettingsField { key: "freeze_on_zero_leechers", label: "Freeze on zero leechers", ty: "bool", hint: "Stop uploading when no leechers are present", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "freeze_on_zero_seeders", label: "Freeze on zero seeders", ty: "bool", hint: "Stop downloading when no seeders are present", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "goal_enabled", label: "Goal enabled", ty: "bool", hint: "Track a target or deadline; the engine adjusts speed to hit it in time", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "goal_direction", label: "Goal direction", ty: "select", hint: "Which counter grows toward the target", req: true, min: None, max: None, step: None, options: GOAL_DIRECTION_OPTIONS },
 SettingsField { key: "goal_upload_target", label: "Goal upload target", ty: "amount", hint: "Upload bytes to reach (upload / download+upload)", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "goal_download_target", label: "Goal download target", ty: "amount", hint: "Download bytes to reach (download / download+upload)", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "goal_target_secs", label: "Goal time (s)", ty: "number", hint: "Deadline from start; 0 = ETA-only (no speed adjustment)", req: true, min: Some("0"), max: Some(units::GOAL_MAX_TIME_SECS_STR), step: Some("1"), options: &[] },
 SettingsField { key: "goal_reached_action", label: "On goal reached", ty: "select", hint: "Stop the task, continue at the initial speed, or switch to a custom speed", req: true, min: None, max: None, step: None, options: GOAL_REACHED_ACTION_OPTIONS },
 SettingsField { key: "goal_reached_bps", label: "Reached speed", ty: "speed", hint: "Custom speed after the goal is reached (continue_custom only); 0 = freeze", req: true, min: None, max: None, step: None, options: &[] },
];

const SWARM_FIELDS: &[SettingsField] = &[
 SettingsField { key: "avg_leecher_download_bps", label: "Avg leecher download (B/s)", ty: "number", hint: "Estimated average download speed of real leechers", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "seed_share_factor", label: "Seed share factor", ty: "number", hint: "Fraction of swarm upload allocated to us", req: true, min: Some("0.01"), max: Some(units::SEED_SHARE_FACTOR_MAX_STR), step: Some("0.01"), options: &[] },
 SettingsField { key: "fair_share_multiplier", label: "Fair share multiplier", ty: "number", hint: "Multiplier for fair-share bandwidth allocation", req: true, min: Some("0"), max: None, step: Some("0.1"), options: &[] },
 SettingsField { key: "max_upload_bps", label: "Max upload (B/s)", ty: "number", hint: "0 = unlimited", req: true, min: Some("0"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "max_download_bps", label: "Max download (B/s)", ty: "number", hint: "0 = unlimited", req: true, min: Some("0"), max: None, step: Some("1"), options: &[] },
];

const PEER_SERVER_FIELDS: &[SettingsField] = &[
 SettingsField { key: "enabled", label: "Enabled", ty: "bool", hint: "Accept incoming peer-wire connections", req: true, min: None, max: None, step: None, options: &[] },
 SettingsField { key: "max_connections", label: "Max connections", ty: "number", hint: "Global limit on concurrent peer connections", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "max_per_ip", label: "Max per IP", ty: "number", hint: "Limit on connections from a single IP address", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "handshake_timeout_secs", label: "Handshake timeout (s)", ty: "number", hint: "Drop peer if handshake not received within this time", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "write_timeout_secs", label: "Write timeout (s)", ty: "number", hint: "Drop peer if a write takes longer than this", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "idle_timeout_secs", label: "Idle timeout (s)", ty: "number", hint: "Drop peer if no messages received for this long", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "body_read_timeout_secs", label: "Body read timeout (s)", ty: "number", hint: "Timeout for reading a peer message body", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "accept_error_backoff_ms", label: "Accept error backoff (ms)", ty: "number", hint: "Pause before retrying accept after an error", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
 SettingsField { key: "capture_keepalive_secs", label: "Capture keepalive (s)", ty: "number", hint: "Keepalive interval advertised during fingerprint capture", req: true, min: Some("1"), max: None, step: Some("1"), options: &[] },
];

fn settings_fields_for(section: &str) -> &'static [SettingsField] {
 match section {
 "server" => SERVER_FIELDS,
 "tracker" => TRACKER_FIELDS,
 "engine" => ENGINE_FIELDS,
 "defaults" => DEFAULTS_FIELDS,
 "swarm_defaults" => SWARM_FIELDS,
 "peer_server" => PEER_SERVER_FIELDS,
 _ => &[],
 }
}

fn settings_field_id(section: &str, key: &str) -> String {
 format!("set-{section}-{key}", section = section, key = key)
}

/// Render a single settings field's HTML structure (label + input + hint,
/// no value - the JS fills in the value when the modal opens). Mirrors the
/// JS `settingsFieldHtml`. Supported `ty`: "text", "number", "bool",
/// "select", "speed" (value + byte-unit dropdown pair, populated by the JS
/// via `setSpeedField`/`getSpeedBps`).
fn render_settings_field(section: &str, f: &SettingsField) -> String {
 let id = settings_field_id(section, f.key);
 let req_mark = if f.req { r#"<span class="req">*</span>"# } else { "" };
 let hint = if !f.hint.is_empty() {
 format!(r#"<div class="hint">{}</div>"#, esc_html(f.hint))
 } else { String::new() };
 match f.ty {
 "bool" => format!(
 r#"<div class="field"><label class="switch-row" for="{id}"><span>{label}{req}</span><span class="switch"><input type="checkbox" id="{id}"><span class="track"><span class="thumb"></span></span></span></label>{hint}</div>"#,
 id = id, label = esc_html(f.label), req = req_mark, hint = hint,
 ),
 "select" => {
 let opts: String = f.options.iter().map(|(v, l)| {
 format!(r#"<option value="{val}">{lbl}</option>"#, val = esc_attr(v), lbl = esc_html(l))
 }).collect();
 format!(
 r#"<div class="field"><label for="{id}">{label}{req}</label><select id="{id}">{opts}</select>{hint}</div>"#,
 id = id, label = esc_html(f.label), req = req_mark, opts = opts, hint = hint,
 )
 },
 "speed" => {
 let units = render_byte_unit_options(&crate::data::units::BYTE_UNIT_MIB.to_string());
 format!(
 r#"<div class="field"><label for="{id}-val">{label}{req}</label><div class="input-group"><input type="number" id="{id}-val" min="0" step="0.1"><select id="{id}-unit">{units}</select></div>{hint}</div>"#,
 id = id, label = esc_html(f.label), req = req_mark, units = units, hint = hint,
 )
 },
 "amount" => {
 let units = render_byte_amount_options(&crate::data::units::BYTE_UNIT_MIB.to_string());
 format!(
 r#"<div class="field"><label for="{id}-val">{label}{req}</label><div class="input-group"><input type="number" id="{id}-val" min="0" step="0.1"><select id="{id}-unit">{units}</select></div>{hint}</div>"#,
 id = id, label = esc_html(f.label), req = req_mark, units = units, hint = hint,
 )
 },
 _ => {
 let min = f.min.map(|m| format!(r#" min="{m}""#, m = m)).unwrap_or_default();
 let max = f.max.map(|m| format!(r#" max="{m}""#, m = m)).unwrap_or_default();
 let step = f.step.map(|s| format!(r#" step="{s}""#, s = s)).unwrap_or_default();
 format!(
 r#"<div class="field"><label for="{id}">{label}{req}</label><input type="{ty}" id="{id}"{min}{max}{step}>{hint}</div>"#,
 id = id, label = esc_html(f.label), req = req_mark, ty = f.ty, min = min, max = max, step = step, hint = hint,
 )
 }
 }
}

/// Render the settings nav buttons. Mirrors the JS `renderSettingsNav`.
pub fn render_settings_nav() -> String {
 SETTINGS_SECTIONS.iter().map(|(key, label)| {
 format!(r#"<button data-section="{key}">{lbl}</button>"#, key = key, lbl = esc_html(label))
 }).collect()
}

/// Render the settings panes (all sections except clients - that's dynamic).
/// Mirrors the JS `renderSettingsPanes`.
pub fn render_settings_panes() -> String {
 let mut html = String::new();
 for (section, _) in SETTINGS_SECTIONS {
 if *section == "clients" {
 // Clients pane - the header with Add/Capture buttons + the
 // #settings-clients container (filled by JS from the config).
 html.push_str(r#"<div class="settings-pane" data-section="clients"><div class="settings-clients-header"><span class="hint">At least one client required. Labels must be unique.</span><span class="settings-clients-actions"><button class="btn btn-secondary" data-action="add-settings-client">Add manually</button><button class="btn btn-secondary" data-action="open-capture-modal">Auto capture</button></span></div><div id="settings-clients"></div></div>"#);
 } else {
 let fields_html: String = settings_fields_for(section)
 .iter()
 .map(|f| render_settings_field(section, f))
 .collect();
 html.push_str(&format!(
 r#"<div class="settings-pane" data-section="{section}">{fields}</div>"#,
 section = section, fields = fields_html,
 ));
 }
 }
 html
}

// Tests

#[cfg(test)]
mod tests {
 use super::*;

 fn task(id: i64, status: &str) -> TaskSummary {
 TaskSummary {
 id,
 name: format!("task-{}", id),
 tracker: "tracker.example".into(),
 announce_url: "http://tracker.example/announce".into(),
 info_hash: "abcdef0123456789abcdef0123456789abcdef01".into(),
 working_client: Some("-qB5220-".into()),
 status: status.into(),
 created_at: "2026-01-01 00:00:00".into(),
 uploaded: 1024,
 downloaded: 2048,
 mode: "Upload".into(),
 strategy: "Dynamic".into(),
 goal: crate::engine::GoalConfig { enabled: false, direction: crate::engine::GoalDirection::Upload, upload_target: 0, download_target: 0, target_secs: 0, reached_action: crate::engine::GoalReachedAction::Stop, reached_bps: 0 },
 }
 }

 #[test]
 fn topbar_stats_empty_when_no_tasks() {
 assert_eq!(render_topbar_stats(0, 0, &[]), "");
 }

 #[test]
 fn byte_unit_options_use_speed_labels() {
  // Speed fields (upload/download speed, max caps, goal reached speed)
  // carry the /s suffix - a rate, not a total.
  let html = render_byte_unit_options("");
  assert!(html.contains("KiB/s"), "speed-unit options must include KiB/s; got: {html}");
  assert!(html.contains("MiB/s"), "speed-unit options must include MiB/s; got: {html}");
  assert!(!html.contains(r#">B/s<"#) || html.contains("B/s"), "B/s present");
 }

 #[test]
 fn byte_unit_options_mib_selected() {
  let html = render_byte_unit_options(&crate::data::units::BYTE_UNIT_MIB.to_string());
  assert!(
   html.contains(r#"value="1048576" selected"#),
   "MiB (value=1048576) must be selected; got: {html}"
  );
 }

 #[test]
 fn byte_amount_options_have_no_per_second() {
  // Goal targets are total byte counts, not speeds - the option labels
  // must NOT carry the /s suffix (regression: they used to show "MiB/s").
  let html = render_byte_amount_options("");
  assert!(!html.contains("/s<"), "amount options must not contain /s in labels; got: {html}");
  assert!(html.contains("MiB"), "amount options must include MiB; got: {html}");
  assert!(html.contains("KiB"), "amount options must include KiB; got: {html}");
  assert!(html.contains("GiB"), "amount options must include GiB; got: {html}");
  assert!(html.contains(r#">B<"#), "amount options must include B; got: {html}");
 }

 #[test]
 fn byte_amount_options_mib_selected() {
  let html = render_byte_amount_options(&crate::data::units::BYTE_UNIT_MIB.to_string());
  assert!(
   html.contains(r#"value="1048576" selected"#),
   "MiB (value=1048576) must be selected by default; got: {html}"
  );
 }

 #[test]
 fn settings_amount_field_uses_amount_options() {
  // The goal_upload_target / goal_download_target settings fields are
  // ty="amount" and must render byte-amount options (no /s), not speed
  // options. Find the <select> for goal_upload_target and verify.
  let panes = render_settings_panes();
  assert!(panes.contains("set-defaults-goal_upload_target-unit"), "upload target unit select exists");
  let id_pos = panes.find("set-defaults-goal_upload_target-unit").unwrap();
  let select_start = panes[..id_pos].rfind("<select").unwrap();
  let after = &panes[select_start..];
  let select_end = select_start + after.find("</select>").unwrap() + "</select>".len();
  let block = &panes[select_start..select_end];
  assert!(!block.contains("/s<"), "goal_upload_target options must be amount units (no /s in labels); got: {block}");
  assert!(block.contains("MiB"), "goal_upload_target options must include MiB; got: {block}");
 }

 #[test]
 fn topbar_stats_shows_counts() {
 let html = render_topbar_stats(3, 2, &[]);
 assert!(html.contains("text-green\">3</div>"), "running count");
 assert!(html.contains("text-muted\">2</div>"), "stopped count");
 }

 #[test]
 fn topbar_stats_shows_global_goal_tile() {
 let goals = [GlobalGoalTile { id: 7, name: "Seed 100GiB".into(), eta: "1d 4h".into() }];
 let html = render_topbar_stats(1, 0, &goals);
 assert!(html.contains(r#"data-goal-id="7""#), "global goal tile with goal id");
 assert!(html.contains(">1d 4h</div>"), "eta value");
 assert!(html.contains("Seed 100GiB"), "goal name as label");
 }

 #[test]
 fn topbar_stats_shows_multiple_goal_tiles() {
 let goals = [
 GlobalGoalTile { id: 1, name: "Goal A".into(), eta: "30m 0s".into() },
 GlobalGoalTile { id: 2, name: "Goal B".into(), eta: "-".into() },
 ];
 let html = render_topbar_stats(2, 1, &goals);
 assert!(html.contains(r#"data-goal-id="1""#), "goal 1 tile");
 assert!(html.contains(r#"data-goal-id="2""#), "goal 2 tile");
 assert!(html.contains(">-</div>"), "unknown eta shows dash");
 }

 #[test]
 fn topbar_stats_omits_goals_when_empty() {
 let html = render_topbar_stats(1, 0, &[]);
 assert!(!html.contains("data-goal-id"), "no goal tiles when empty");
 }

 #[test]
 fn task_list_empty_shows_placeholder() {
 let html = render_task_list(&[], 0);
 assert!(html.contains("No tasks yet"), "got: {}", html);
 assert!(html.contains(r#"<div class="empty">"#), "empty div visible");
 assert!(html.contains(r#"<table class="task-table hidden">"#), "table hidden");
 }

 #[test]
 fn task_list_renders_table_and_rows() {
 let tasks = vec![task(1, "running"), task(2, "stopped")];
 let html = render_task_list(&tasks, 1);
 assert!(html.contains(r#"<table class="task-table">"#), "table visible");
 assert!(html.contains(r#"<div class="empty hidden">"#), "empty div hidden");
 assert!(html.contains(r#"data-id="1""#), "row 1");
 assert!(html.contains(r#"data-id="2""#), "row 2");
 assert!(html.contains(r#"class="active""#), "active row");
 }

 #[test]
 fn task_row_running_has_stop_button() {
 let html = render_task_row(&task(1, "running"), false);
 assert!(html.contains(r#"data-action="stop""#), "stop button");
 assert!(!html.contains(r#"data-action="start""#), "no start button");
 }

 #[test]
 fn task_row_stopped_has_start_button() {
 let html = render_task_row(&task(1, "stopped"), false);
 assert!(html.contains(r#"data-action="start""#), "start button");
 assert!(!html.contains(r#"data-action="stop""#), "no stop button");
 }

 #[test]
 fn task_row_escapes_name() {
 let mut t = task(1, "running");
 t.name = "<script>alert(1)</script>".into();
 let html = render_task_row(&t, false);
 assert!(!html.contains("<script>"), "no live script tag");
 assert!(html.contains("&lt;script&gt;"), "escaped");
 }

 #[test]
 fn task_row_no_client_shows_dash() {
 let mut t = task(1, "running");
 t.working_client = None;
 let html = render_task_row(&t, false);
    assert!(html.contains("-"), "empty dash for no client");
 }

 #[test]
 fn log_row_with_failure_shows_fail_badge() {
 let ev = EventView { seq: 1,
 phase: "attack".into(),
 event: "started".into(),
 uploaded_display: "1.00 KiB".into(),
 downloaded_display: "0 B".into(),
 left_display: "0 B".into(),
 success: false,
 failure_reason: Some("tracker rejected".into()),
 seeders: 1,
 leechers: 0,
 fair_share_display: "512.00 KiB/s".into(),
 target_speed_display: "0 B/s".into(),
 speed_cell_display: "512.00 KiB/s ↑".into(),
 next_announce_display: "1m 0s".into(),
 uploaded: 1024, downloaded: 0, fair_share_bps: 524_288, dynamic_target_bps: 0, elapsed_secs: 0,
 };
 let cols = LogColumns { show_downloaded: true, show_left: true, show_download_speed: true };
 let html = render_log_row(&ev, &cols);
 assert!(html.contains(r#"badge fail"#), "fail badge");
 assert!(html.contains(r#"title="tracker rejected""#), "failure reason in title");
 }

 #[test]
 fn log_row_success_shows_ok_badge() {
 let ev = EventView { seq: 1,
 phase: "attack".into(),
 event: "tick".into(),
 uploaded_display: "0 B".into(),
 downloaded_display: "0 B".into(),
 left_display: "0 B".into(),
 success: true,
 failure_reason: None,
 seeders: 0,
 leechers: 0,
 fair_share_display: "0 B/s".into(),
 target_speed_display: "0 B/s".into(),
 speed_cell_display: "0 B/s ↑".into(),
 next_announce_display: "-".into(),
 uploaded: 0, downloaded: 0, fair_share_bps: 0, dynamic_target_bps: 0, elapsed_secs: 0,
 };
 let cols = LogColumns { show_downloaded: false, show_left: false, show_download_speed: false };
 let html = render_log_row(&ev, &cols);
 assert!(html.contains(r#"badge ok"#), "ok badge");
 // No downloaded/left columns when hidden
 assert!(!html.contains("Downloaded"), "no downloaded column");
 }

 #[test]
 fn log_stats_has_data_stat_keys() {
 let ev = EventView { seq: 1,
 phase: "attack".into(),
 event: "tick".into(),
 uploaded_display: "1.00 KiB".into(),
 downloaded_display: "0 B".into(),
 left_display: "0 B".into(),
 success: true,
 failure_reason: None,
 seeders: 5,
 leechers: 3,
 fair_share_display: "512.00 KiB/s".into(),
 target_speed_display: "1.00 MiB/s".into(),
 speed_cell_display: "512.00 KiB/s ↑ 1.00 MiB/s ↓".into(),
 next_announce_display: "30s".into(),
 uploaded: 1024, downloaded: 1_048_576, fair_share_bps: 524_288, dynamic_target_bps: 1_048_576, elapsed_secs: 0,
 };
 let cols = LogColumns { show_downloaded: true, show_left: true, show_download_speed: true };
 let html = render_log_stats(&[ev], &cols, "1.00 KiB", 1, None);
 for key in ["phase", "uploaded", "upload", "download", "seeders", "leechers", "success", "next-announce"] {
 assert!(html.contains(&format!(r#"data-stat="{}""#, key)), "missing data-stat={}", key);
 }
 assert!(html.contains("5</div>"), "seeders value");
 assert!(html.contains("1/1"), "success ratio");
 }

 #[test]
 fn log_panel_empty_shows_no_events() {
 let cols = LogColumns { show_downloaded: true, show_left: true, show_download_speed: true };
 let html = render_log_panel(&[], None, &cols, "0 B", 0);
 assert!(html.contains("No events yet"), "empty placeholder");
 }

 #[test]
 fn log_panel_renders_audit_info() {
 let ai = AuditInfoView {
 name: "my-torrent".into(),
 status: "running".into(),
 working_client: Some("qBittorrent 5.2.2".into()),
 torrent_info: vec![("Announce URL".into(), "https://tracker.example/announce".into())],
 config_rows: vec![("Mode".into(), "Upload only".into())],
 goal: crate::engine::GoalConfig { enabled: false, direction: GoalDirection::Upload, upload_target: 0, download_target: 0, target_secs: 0, reached_action: crate::engine::GoalReachedAction::Stop, reached_bps: 0 },
 };
 let cols = LogColumns { show_downloaded: true, show_left: true, show_download_speed: true };
 let html = render_log_panel(&[], Some(&ai), &cols, "0 B", 0);
 assert!(html.contains("audit-info-name"), "audit info panel");
 assert!(html.contains("my-torrent"), "audit name");
 assert!(html.contains("data-col=\"audit-status\""), "status badge hook");
 assert!(html.contains("data-col=\"audit-client\""), "client hook");
 assert!(html.contains("Announce URL"), "torrent info");
 assert!(html.contains("Configuration"), "config rows header");
 }

 #[test]
 fn log_panel_shows_probing_for_running_task_without_client() {
 let ai = AuditInfoView {
 name: "probing-task".into(),
 status: crate::data::vocab::STATUS_RUNNING.into(),
 working_client: None,
 torrent_info: vec![],
 config_rows: vec![],
 goal: crate::engine::GoalConfig { enabled: false, direction: GoalDirection::Upload, upload_target: 0, download_target: 0, target_secs: 0, reached_action: crate::engine::GoalReachedAction::Stop, reached_bps: 0 },
 };
 let cols = LogColumns { show_downloaded: true, show_left: true, show_download_speed: true };
 let html = render_log_panel(&[], Some(&ai), &cols, "0 B", 0);
 assert!(
 html.contains(r#"data-col="audit-client">probing...</span>"#),
 "running task without a resolved client must show 'probing...'; got: {html}"
 );
 }

 #[test]
 fn log_panel_shows_dash_for_stopped_task_without_client() {
 // Regression: a stopped task whose probes all failed (working_client
 // NULL) used to show "probing..." - misleading, since it is no longer
 // probing. It must show the empty dash, matching the task list and the
 // JS live-update path (resolveClientName → EMPTY_DASH).
 let ai = AuditInfoView {
 name: "failed-probe-task".into(),
 status: crate::data::vocab::STATUS_STOPPED.into(),
 working_client: None,
 torrent_info: vec![],
 config_rows: vec![],
 goal: crate::engine::GoalConfig { enabled: false, direction: GoalDirection::Upload, upload_target: 0, download_target: 0, target_secs: 0, reached_action: crate::engine::GoalReachedAction::Stop, reached_bps: 0 },
 };
 let cols = LogColumns { show_downloaded: true, show_left: true, show_download_speed: true };
 let html = render_log_panel(&[], Some(&ai), &cols, "0 B", 0);
 assert!(
 !html.contains("probing..."),
 "stopped task must not show 'probing...'; got: {html}"
 );
 assert!(
 html.contains(&format!(r#"data-col="audit-client">{}</span>"#, labels::EMPTY_DASH)),
 "stopped task without client must show the empty dash; got: {html}"
 );
 }

 #[test]
 fn esc_html_escapes_angle_brackets_and_amp() {
 assert_eq!(esc_html("<script>&"), "&lt;script&gt;&amp;");
 }

 #[test]
 fn esc_attr_escapes_quotes() {
 assert_eq!(esc_attr(r#"a"b"#), "a&quot;b");
 }
}
