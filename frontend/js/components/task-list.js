// Task list rendering + SSE-driven row updates. Renders the audit table,
// topbar counts, and per-row actions; handles stop/restart/delete/view-log.
// Helpers (taskRow, adjustCount, getBadgeStatus, resolveClientName,
// taskActionsHtml) were previously in _js_shared.html.

import { state } from '../state/store.js';
import { dom, taskRow } from '../state/dom.js';
import { formatBytes, goalEtaSeconds } from '../utils/format.js';
import { fetchJson, postAction, deleteAction, fetchRaw } from '../utils/net.js';
import { escHtml, escAttr } from '../utils/dom-helpers.js';
import { showConfirm, CONFIRM_CANCEL } from './confirm.js';
import { toast } from './toast.js';
import { loadLog } from './log-panel.js';
import { EMPTY_DASH, EMPTY_LOG, STATUS_RUNNING, STATUS_STOPPED } from '../data/labels.js';

// Helpers

// Build a fresh goal-state entry from a row's data-goal-* attrs + its badge
// status. Raw progress (uploaded/downloaded) starts at 0 - the live `audit`
// SSE events populate it (the rendered cells hold formatted strings, not raw
// bytes, so they can't be parsed back). Returns null when the row lacks goal
// attrs (older rows / non-goal tasks).
function goalFromRow(r) {
 const ds = r.dataset;
 if (ds.goalEnabled === undefined) return null;
 return {
 enabled: ds.goalEnabled === 'true',
 direction: ds.goalDirection || 'upload',
 uploadTarget: parseInt(ds.goalUploadTarget) || 0,
 downloadTarget: parseInt(ds.goalDownloadTarget) || 0,
 secs: parseInt(ds.goalSecs) || 0,
 uploaded: 0,
 downloaded: 0,
 upBps: 0,
 downBps: 0,
 lastUpBps: 0,
 lastDownBps: 0,
 status: getBadgeStatus(r),
 };
}

// Write a goal entry from a TaskSummary-shaped object (task_created /
// task_updated SSE payload). Preserves live progress when the entry exists.
function mergeGoalConfig(id, g) {
 if (!g) return;
 const prev = state.goals[id];
 state.goals[id] = {
 enabled: !!g.enabled,
 direction: g.direction || 'upload',
 uploadTarget: g.upload_target ?? 0,
 downloadTarget: g.download_target ?? 0,
 secs: g.target_secs ?? 0,
 uploaded: prev?.uploaded ?? 0,
 downloaded: prev?.downloaded ?? 0,
 upBps: prev?.upBps ?? 0,
 downBps: prev?.downBps ?? 0,
 lastUpBps: prev?.lastUpBps ?? 0,
 lastDownBps: prev?.lastDownBps ?? 0,
 status: prev?.status ?? '',
 };
}

// Refresh a row's data-goal-* attrs from a TaskSummary payload so a later
// loadTaskList re-read matches the live config.
function syncGoalAttrs(row, g) {
 if (!row || !g) return;
 row.dataset.goalEnabled = String(!!g.enabled);
 row.dataset.goalDirection = g.direction || 'upload';
 row.dataset.goalUploadTarget = String(g.upload_target ?? 0);
 row.dataset.goalDownloadTarget = String(g.download_target ?? 0);
 row.dataset.goalSecs = String(g.target_secs ?? 0);
}

// Adjust running/stopped count and update topbar.
function adjustCount(status, delta) {
 if (status === STATUS_RUNNING) state.runningCount += delta;
 else if (status === STATUS_STOPPED) state.stoppedCount += delta;
 updateTopbar();
}

// Extract badge status string from a task row.
function getBadgeStatus(row) {
 const b = row ? row.querySelector('.badge') : null;
 return b ? b.textContent.trim() : '';
}

// Resolve a working_client key to a display name via clientMap.
export function resolveClientName(workingClient) {
 return workingClient ? (state.clientMap[workingClient] || workingClient) : EMPTY_DASH;
}

// Render task action buttons (Edit + Stop/Start + Delete) for the task list.
export function taskActionsHtml(id, status) {
 const running = status === STATUS_RUNNING;
 const mid = running
 ? '<button class="act-stop" data-action="stop" data-id="' + id + '" aria-label="Stop task ' + id + '">Stop</button>'
 : '<button class="act-start" data-action="start" data-id="' + id + '" aria-label="Start task ' + id + '">Start</button>';
 return '<button class="act-edit" data-action="edit" data-id="' + id + '" aria-label="Edit task ' + id + '">Edit</button>'
 + mid
 + '<button class="act-del" data-action="delete" data-id="' + id + '" aria-label="Delete task ' + id + '">Delete</button>';
}

// Task list: one-time initial fetch

// Task list: initial load is server-rendered; SSE drives deltas

export function loadTaskList() {
 fetchRaw('/html/audits').then(async r => {
 if (!r.ok) return;
 const html = await r.text();
 dom.audit_list.innerHTML = html;
 if (state.activeLogId) { const row = taskRow(state.activeLogId); if (row) row.classList.add('active'); }
 initTopbarCounts();
 updateTopbar();
 afterListUpdate();
 });
}

// Fetch the pre-rendered goals table HTML fragment - mirrors loadTaskList.
// Used by the SSE reconnect handler + goal_created/goal_deleted/goal_updated
// events so the goals table stays in sync without polling.
export function loadGoalList() {
 fetchRaw('/html/goals').then(async r => {
 if (!r.ok) return;
 const html = await r.text();
 dom.goal_list.innerHTML = '<div>' + html + '</div>';
 });
}

function afterListUpdate() {
 if (!state.firstLoadDone) {
 state.firstLoadDone = true;
 const saved = localStorage.getItem('activeLogId');
 const savedId = saved ? parseInt(saved) : NaN;
 if (!isNaN(savedId) && taskRow(savedId)) {
 viewLog(savedId);
 } else {
 const firstRow = dom.audit_list.querySelector('tbody tr[data-id]');
 if (firstRow) viewLog(parseInt(firstRow.dataset.id));
 }
 }
}

export function initTopbarCounts() {
 state.runningCount = 0; state.stoppedCount = 0;
 state.goals = {};
 dom.audit_list.querySelectorAll('tbody tr').forEach(r => {
 // Tally without rebuilding the topbar - the server already rendered it,
 // and SSE handlers (adjustCount) maintain it going forward. Only call
 // updateTopbar when the DOM is out of sync (which it isn't on first load).
 const status = getBadgeStatus(r);
 if (status === STATUS_RUNNING) state.runningCount++;
 else if (status === STATUS_STOPPED) state.stoppedCount++;
 const g = goalFromRow(r);
 if (g) state.goals[r.dataset.id ? parseInt(r.dataset.id) : 0] = g;
 });
}

// Per-direction active target + ETA helper. Returns { target, current, bps, eta }
// for one direction, or null if that direction isn't tracked.
function goalDirEta(g, dir) {
 const isUp = dir === 'upload';
 const target = isUp ? g.uploadTarget : g.downloadTarget;
 if (target <= 0) return null;
 const current = isUp ? g.uploaded : g.downloaded;
 const bps = isUp ? g.upBps : g.downBps;
 const last = isUp ? g.lastUpBps : g.lastDownBps;
 const useBps = bps > 0 ? bps : last;
 const remaining = Math.max(0, target - current);
 return goalEtaSeconds(remaining, useBps);
}

// Max ETA (seconds) across running tasks with an active goal, or null when no
// active goal has a known speed. Tracks the upload-direction ETA (the
// download-only direction was removed; download progress in DU mode is
// surfaced per-task in the log panel, not the topbar aggregate).
function goalTopbarEta() {
 let max = null;
 for (const id in state.goals) {
 const g = state.goals[id];
 if (g.status !== STATUS_RUNNING || !g.enabled) continue;
 const tracksUp = g.direction === 'upload' || g.direction === 'download_and_upload';
 if (tracksUp) {
 const eta = goalDirEta(g, 'upload');
 if (eta !== null && (max === null || eta > max)) max = eta;
 }
 }
 return max;
}

function isGoalActive() {
 for (const id in state.globalGoals) {
 if (state.globalGoals[id]) return true;
 }
 return false;
}

// Global goal tiles (server-driven via goal_progress SSE)

/// Fetch all global goals from the API and rebuild the topbar tiles + state.
export function loadGlobalGoals() {
 fetchJson('/api/goals', null, 'Failed to load goals').then(goals => {
 if (!Array.isArray(goals)) return;
 state.globalGoals = {};
 for (const g of goals) {
 state.globalGoals[g.id] = { id: g.id, name: g.name, eta: EMPTY_DASH, task_ids: g.task_ids || [] };
 }
 renderGlobalGoalTiles();
 });
}

/// Render all global goal tiles from state.globalGoals, after the running/
/// stopped tiles. Mirrors render_topbar_stats.
function renderGlobalGoalTiles() {
 const runningEl = dom.topbar_stats.querySelector('.text-green');
 const stoppedEl = dom.topbar_stats.querySelector('.text-muted');
 if (!runningEl || !stoppedEl) return;
 // Remove old goal tiles
 dom.topbar_stats.querySelectorAll('[data-goal-id]').forEach(el => el.remove());
 // Append current goals
 for (const id in state.globalGoals) {
 const g = state.globalGoals[id];
 dom.topbar_stats.insertAdjacentHTML('beforeend',
 `<div class="stat" data-goal-id="${g.id}"><div class="val">${g.eta}</div><div class="lbl" title="${escAttr(g.name)}">${escHtml(g.name)}</div></div>`);
 }
}

/// Patch one goal tile's ETA value (from goal_progress SSE).
export function patchGoalTile(id, etaStr) {
 if (!state.globalGoals || !state.globalGoals[id]) return;
 state.globalGoals[id].eta = etaStr;
 const tile = dom.topbar_stats.querySelector(`[data-goal-id="${id}"] .val`);
 if (tile && tile.textContent !== etaStr) tile.textContent = etaStr;
}

/// Remove a goal tile (from goal_deleted SSE).
export function removeGoalTile(id) {
 if (state.globalGoals) delete state.globalGoals[id];
 const tile = dom.topbar_stats.querySelector(`[data-goal-id="${id}"]`);
 if (tile) tile.remove();
}

function updateTopbar() {
 const total = state.runningCount + state.stoppedCount;
 if (total === 0 && !isGoalActive()) { dom.topbar_stats.innerHTML = ''; return; }
 // Surgical patch - the server already rendered the tile structure on first
 // paint; only update the `.val` textContent values. No HTML string building
 // here (avoids duplicating the server's template - see render_topbar_stats).
 const runningEl = dom.topbar_stats.querySelector('.text-green');
 const stoppedEl = dom.topbar_stats.querySelector('.text-muted');
 if (runningEl) runningEl.textContent = state.runningCount;
 if (stoppedEl) stoppedEl.textContent = state.stoppedCount;
}

// Live update a task's goal progress + speeds from an `audit` SSE event. Runs
// for EVERY task (not just the active log) so the topbar aggregate stays live.
export function updateGoalFromAudit(ev) {
 const g = state.goals[ev.audit_id];
 if (!g) return;
 g.uploaded = ev.uploaded;
 g.downloaded = ev.downloaded;
 g.upBps = ev.fair_share_bps;
 g.downBps = ev.dynamic_target_bps;
 if (ev.fair_share_bps > 0) g.lastUpBps = ev.fair_share_bps;
 if (ev.dynamic_target_bps > 0) g.lastDownBps = ev.dynamic_target_bps;
 updateTopbar();
}

// SSE-driven diff updates (only touch what changed)

export function addTaskRow(task) {
 let tbody = dom.audit_list.querySelector('table tbody');
 // Empty→non-empty transition: the table is always in the DOM (hidden when
 // empty), so detect the transition by checking if the tbody has no rows.
 if (tbody && tbody.children.length === 0) {
 dom.audit_list.querySelector('.empty')?.classList.add('hidden');
 dom.audit_list.querySelector('.task-table')?.classList.remove('hidden');
 }
 // Insert the server-pre-rendered <tr> HTML - the JS never builds row HTML.
 if (tbody) {
 tbody.insertAdjacentHTML('afterbegin', task.html || '');
 mergeGoalConfig(task.id, task.goal);
 if (task.goal && state.goals[task.id]) state.goals[task.id].status = task.status;
 adjustCount(task.status, 1);
 }
}

export function removeTaskRow(id) {
 const row = taskRow(id);
 if (!row) return;
 const badge = row.querySelector('.badge');
 const status = getBadgeStatus(row);
 adjustCount(status, -1);
 delete state.goals[id];
 updateTopbar();
 row.remove();
 if (state.activeLogId === id) { state.activeLogId = null; dom.log_panel.innerHTML = '<div class="empty">' + EMPTY_LOG + '</div>'; }
 const tbody = dom.audit_list.querySelector('table tbody');
 if (tbody && tbody.children.length === 0) {
 // Non-empty→empty transition: hide the table, show the placeholder.
 dom.audit_list.querySelector('.task-table')?.classList.add('hidden');
 dom.audit_list.querySelector('.empty')?.classList.remove('hidden');
 }
}

export function setTaskStatus(id, status) {
 // Update task-list row
 const row = taskRow(id);
 if (row) {
 const badge = row.querySelector('[data-col="status"] .badge');
 if (badge) {
 const old = badge.textContent.trim();
 if (old !== status) {
 badge.className = 'badge ' + status;
 badge.textContent = status;
 adjustCount(old, -1);
 adjustCount(status, 1);
 // Only rebuild action buttons when the status actually changed -
 // the buttons (Edit/Stop|Start/Delete) only differ by running vs
 // stopped, so a no-op status event shouldn't touch the DOM.
 const actions = row.querySelector('[data-col="actions"]');
 if (actions) {
 actions.innerHTML = taskActionsHtml(id, status);
 }
 }
 }
 }
 // A status change re-qualifies the task for the topbar goal aggregate.
 if (state.goals[id]) { state.goals[id].status = status; updateTopbar(); }
 // Update log-panel audit-info badge (same SSE event)
 if (id === state.activeLogId) {
 const logBadge = dom.log_panel.querySelector('[data-col="audit-status"]');
 if (logBadge) { logBadge.className = 'badge ' + status; logBadge.textContent = status; }
 }
}

export function setTaskClient(id, workingClient) {
 const text = resolveClientName(workingClient);
 // Update task-list row
 const row = taskRow(id);
 if (row) {
 const cell = row.querySelector('[data-col="client"]');
 if (cell && cell.textContent !== text) cell.textContent = text;
 }
 // Update log-panel audit-info Client field (same SSE event)
 if (id === state.activeLogId) {
 const logClient = dom.log_panel.querySelector('[data-col="audit-client"]');
 if (logClient && logClient.textContent !== text) logClient.textContent = text;
 }
}

export function setTaskProgress(id, uploaded, downloaded) {
 const row = taskRow(id);
 if (!row) return;
 const upCell = row.querySelector('[data-col="uploaded"]');
 const dlCell = row.querySelector('[data-col="downloaded"]');
 const newUp = formatBytes(uploaded);
 const newDl = formatBytes(downloaded);
 if (upCell && upCell.textContent !== newUp) { upCell.textContent = newUp; flashCell(upCell); }
 if (dlCell && dlCell.textContent !== newDl) { dlCell.textContent = newDl; flashCell(dlCell); }
}

function flashCell(el) {
 // Restart the CSS flash animation without a forced reflow. Removing the
 // class then re-adding it across two animation frames lets the browser
 // process the style removal (style recalc) before re-adding - no
 // `offsetWidth` layout read needed. Respects prefers-reduced-motion (the
 // global `animation: none !important` rule still applies to the class).
 el.classList.remove('flash');
 requestAnimationFrame(() => requestAnimationFrame(() => el.classList.add('flash')));
}

// task_updated - mode/strategy + goal can change via edit (name/tracker are locked).
export function setTaskUpdated(task) {
 const row = taskRow(task.id);
 if (!row) return;
 const modeCell = row.querySelector('[data-col="mode"]');
 const strategyCell = row.querySelector('[data-col="strategy"]');
 if (modeCell && modeCell.textContent !== task.mode) modeCell.textContent = task.mode;
 if (strategyCell && strategyCell.textContent !== task.strategy) strategyCell.textContent = task.strategy;
 mergeGoalConfig(task.id, task.goal);
 syncGoalAttrs(row, task.goal);
 updateTopbar();
}

// Actions
// Optimistic: flip the row's status/remove it before the request resolves so
// the UI feels instant. The matching SSE event is suppressed by
// shouldSuppressToast() (lastTaskActionAt was just set). On failure the row is
// rolled back; the shared net wrapper already toasted the error.

export async function stopAudit(id) {
 const row = taskRow(id);
 const prevStatus = row ? getBadgeStatus(row) : null;
 if (row) setTaskStatus(id, STATUS_STOPPED);
 state.lastTaskActionAt = Date.now();
 const ok = await postAction('/api/audits/' + id + '/stop', 'Failed to stop task #' + id);
 if (ok) toast('Task #' + id + ' stopping', 'info');
 else if (row && prevStatus) setTaskStatus(id, prevStatus);
}

export async function restartAudit(id) {
 const row = taskRow(id);
 const prevStatus = row ? getBadgeStatus(row) : null;
 if (row) setTaskStatus(id, STATUS_RUNNING);
 state.lastTaskActionAt = Date.now();
 const ok = await postAction('/api/audits/' + id + '/start', 'Failed to start task #' + id);
 if (ok) toast('Task #' + id + ' starting', 'info');
 else if (row && prevStatus) setTaskStatus(id, prevStatus);
}

export async function deleteAudit(id) {
 const row = taskRow(id);
 const running = getBadgeStatus(row) === STATUS_RUNNING;
 const msg = running
 ? 'Delete task <strong>#' + id + '</strong>? It will be stopped first. This cannot be undone.'
 : 'Delete task <strong>#' + id + '</strong>? This cannot be undone.';
 const confirmed = await showConfirm('Delete task', msg, [
 CONFIRM_CANCEL,
 { label: 'Delete', class: 'btn-danger', value: 'ok' }
 ]);
 if (confirmed !== 'ok') return;
 state.lastTaskActionAt = Date.now();
 // Optimistic: remove the row immediately. On failure, re-fetch the
 // server-rendered list to restore the row + counts (the net wrapper already
 // toasted the error).
 if (row) removeTaskRow(id);
 const ok = await deleteAction('/api/audits/' + id, 'Failed to delete task #' + id);
 if (ok) toast('Task #' + id + ' deleted', 'info');
 else loadTaskList();
}

export function viewLog(id) {
 if (state.activeLogId === id) return;
 localStorage.setItem('activeLogId', id);
 loadLog(id);
}

// Table tab switching (generic - data-driven)
// Drives content visibility from [data-tab-content] and the "New task"
// button label from the active tab button's [data-new-label]. Adding a new
// tab = add a button (with data-tab + data-new-label) + a content div (with
// data-tab-content) in index.html + a loader in init.js - no change to this
// function or dom.js needed.

export function switchTableTab(tab) {
 state.activeTab = tab;
 const activeBtn = document.querySelector(`#table-tabs button[data-tab="${tab}"]`);
 document.querySelectorAll('#table-tabs button[data-tab]').forEach(b => {
 const active = b.dataset.tab === tab;
 b.classList.toggle('active', active);
 b.setAttribute('aria-checked', active);
 b.setAttribute('tabindex', active ? '0' : '-1');
 });
 // Toggle tab panels via .inactive (grid-rows 0fr, no display:none) - avoids CLS.
 document.querySelectorAll('[data-tab-content]').forEach(el => {
 el.classList.toggle('inactive', el.dataset.tabContent !== tab);
 });
 const label = document.getElementById('new-audit-btn-label');
 if (label) label.textContent = (activeBtn && activeBtn.dataset.newLabel) || 'New task';
}
