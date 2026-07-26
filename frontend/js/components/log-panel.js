// Log panel - the server renders the full panel HTML (audit info, stats
// strip, event table). JS only inserts server-pre-rendered `<tr>` fragments
// on SSE `audit` events and updates individual stat tiles via per-tile
// `textContent` (no innerHTML rebuilds). The server is the single source of
// truth for log-panel HTML; this module never builds HTML strings.

import { state } from '../state/store.js';
import { dom, taskRow } from '../state/dom.js';
import { formatBytes, formatSpeedBps, formatDuration, goalEtaSeconds } from '../utils/format.js';
import { fetchRaw, clearPending, pendingSignal } from '../utils/net.js';
import { EMPTY_DASH } from '../data/labels.js';

// Dev assertions
const DEBUG = new URLSearchParams(location.search).has('debug')
 || localStorage.getItem('debug') === '1';
function assertRendered(condition, msg) {
 if (DEBUG && !condition) console.error('[render] ' + msg);
}

// Shared helpers

// Mark a task row as the active log in the task list (adds .active, clears
// others) and set state.activeLogId. Shared by loadLog (fetch path) and
// setLogPanel (inline-HTML path used by task creation).
function setActiveLog(id) {
 state.activeLogId = id;
 document.querySelectorAll('#audit-list tr').forEach(r => r.classList.remove('active'));
 const row = taskRow(id);
 if (row) row.classList.add('active');
}

// Replace the log panel DOM with pre-rendered HTML and recompute row/success
// counters from the new DOM. Shared by loadLog and setLogPanel.
function renderLogPanel(html) {
 dom.log_panel.innerHTML = html;
 state.logTotalRows = dom.log_panel.querySelectorAll('tbody tr').length;
 state.logSuccessCount = dom.log_panel.querySelectorAll('.badge.ok').length;
 assertRendered(dom.log_panel.querySelector('table tbody') !== null,
 'renderLogPanel: HTML has no <table><tbody>');
}

// Set the log panel from pre-rendered HTML without a fetch - used after task
// creation when the POST /api/audits response carries log_html.
export function setLogPanel(id, html) {
 setActiveLog(id);
 renderLogPanel(html);
}

// Check if a log row with the given seq already exists in the DOM. Used by
// appendLogRow to dedup SSE events that race with loadLog fetches.
function hasLogRow(seq) {
 const tbody = dom.log_panel.querySelector('table tbody');
 return tbody ? !!tbody.querySelector('tr[data-seq="' + seq + '"]') : false;
}

// Initial load (fetch the server-rendered HTML fragment)

export function loadLog(id) {
 if (state.activeLogId && state.activeLogId !== id) clearPending('load-log:' + state.activeLogId);
 setActiveLog(id);
 const signal = pendingSignal('load-log:' + id);
 fetchRaw('/html/audits/' + id + '/log', { signal }).then(async r => {
 if (!r.ok) return;
 const html = await r.text();
 if (html && state.activeLogId === id) renderLogPanel(html);
 }).catch(() => {});
}

// Read column-visibility flags from .log-stats (set by the server from LogColumns).
// Single source of truth: templates::LogColumns::for_config decides what's visible;
// the server emits data-show-* attrs, and the SSE handlers consume them.
function logColumns() {
 const el = dom.log_panel.querySelector('.log-stats');
 if (!el) return { showDownloaded: true, showLeft: true, showDownloadSpeed: true };
 const ds = el.dataset;
 return {
 showDownloaded: ds.showDownloaded !== 'false',
 showLeft: ds.showLeft !== 'false',
 showDownloadSpeed: ds.showDownloadSpeed !== 'false',
 };
}

// SSE-driven updates

// SSE audit events prepend the pre-rendered <tr> into the log table's
// <tbody> (newest-first, matching the server's ORDER BY DESC) and update the
// stats strip via updateLogStats. The "No events yet." placeholder is hidden
// when the first row arrives. `logTotalRows` and `logSuccessCount` are
// maintained so the success ratio stat stays accurate.
export function appendLogRow(ev) {
 state.logTotalRows++;
 if (!ev.failure_reason) state.logSuccessCount++;
 const tbody = dom.log_panel.querySelector('table tbody');
 if (tbody && ev.html) {
 // Dedup by seq: if the row already exists (e.g. loadLog fetched it
 // from the server and then the SSE event arrived), skip the insert to
 // prevent duplicate rows and inflated counters.
 if (hasLogRow(ev.seq)) {
 state.logTotalRows--;
 if (!ev.failure_reason) state.logSuccessCount--;
 return;
 }
 tbody.insertAdjacentHTML('afterbegin', ev.html);
 const empty = dom.log_panel.querySelector('.empty');
 if (empty) empty.classList.add('hidden');
 // Prune oldest rows beyond the configured limit to match the server's
 // event_log_limit - keeps the DOM bounded and consistent with what
 // loadLog would render on a refresh.
 if (state.logMaxRows > 0) {
 while (tbody.children.length > state.logMaxRows) {
 const last = tbody.lastElementChild;
 if (!last) break;
 if (last.querySelector('.badge.ok')) state.logSuccessCount--;
 last.remove();
 state.logTotalRows--;
 }
 }
 }
}

// Per-tile stats update - touches only the `.val` textContent of each stat
// (and the phase tile's class). No innerHTML rebuild on the audit-event hot
// path. Mirrors the surgical pattern of setTaskProgress.
export function updateLogStats(ev) {
 const stats = dom.log_panel.querySelector('.log-stats');
 if (!stats) {
 assertRendered(false, 'updateLogStats: .log-stats not found in log_panel');
 return;
 }
 const cols = logColumns();
 if (DEBUG) console.log('[stats] updating from event:', ev.event, 'uploaded:', ev.uploaded, 'up_bps:', ev.fair_share_bps, 'dl_bps:', ev.dynamic_target_bps);
 setStat(stats, 'phase', ev.phase, 'phase-' + ev.phase);
 setStat(stats, 'uploaded', formatBytes(ev.uploaded));
 setStat(stats, 'upload', formatSpeedBps(ev.fair_share_bps));
 if (cols.showDownloadSpeed) setStat(stats, 'download', formatSpeedBps(ev.dynamic_target_bps));
 setStat(stats, 'seeders', String(ev.seeders));
 setStat(stats, 'leechers', String(ev.leechers));
 setStat(stats, 'success', state.logSuccessCount + '/' + state.logTotalRows);
 setStat(stats, 'next-announce', ev.next_announce_in_secs > 0 ? formatDuration(ev.next_announce_in_secs) : EMPTY_DASH);
 // Goal tiles (per-direction progress + binding ETA + required speed) -
 // config from state.goals (set at task load), live counters from this audit
 // event. setStat no-ops when a tile is absent (goal disabled at log-load
 // time). In DownloadAndUpload mode two progress tiles (goal-up + goal-dl)
 // are patched and the ETA is the max of the two.
 const g = state.goals[ev.audit_id];
 if (g && g.enabled) {
 const tracksUp = g.direction === 'upload' || g.direction === 'download_and_upload';
 const tracksDl = g.direction === 'download_and_upload';
 let etas = [];
 if (tracksUp && g.uploadTarget > 0) {
 const remaining = Math.max(0, g.uploadTarget - ev.uploaded);
 const etaSecs = goalEtaSeconds(remaining, ev.fair_share_bps);
 setStat(stats, 'goal-up', formatBytes(ev.uploaded) + ' / ' + formatBytes(g.uploadTarget));
 if (etaSecs !== null) etas.push(etaSecs);
 }
 if (tracksDl && g.downloadTarget > 0) {
 const remaining = Math.max(0, g.downloadTarget - ev.downloaded);
 const etaSecs = goalEtaSeconds(remaining, ev.dynamic_target_bps);
 setStat(stats, 'goal-dl', formatBytes(ev.downloaded) + ' / ' + formatBytes(g.downloadTarget));
 if (etaSecs !== null) etas.push(etaSecs);
 }
 // Binding ETA = max of all active direction ETAs; "-" if any unknown.
 if (tracksUp || tracksDl) {
 const bindingEta = etas.length > 0 ? Math.max(...etas) : null;
 setStat(stats, 'goal-eta', bindingEta === null ? EMPTY_DASH : formatDuration(bindingEta));
 }
 if (g.secs > 0 && g.uploadTarget > 0) {
 setStat(stats, 'goal-required', formatSpeedBps(Math.round(g.uploadTarget / g.secs)));
 }
 }
}

// Update one stat tile's `.val` text (and class when given). No-op if the tile
// is absent or the value is unchanged.
function setStat(stats, key, text, valClass) {
 const tile = stats.querySelector(`[data-stat="${key}"]`);
 if (!tile) return;
 const v = tile.querySelector('.val');
 if (valClass !== undefined && v.className !== 'val ' + valClass) v.className = 'val ' + valClass;
 if (v.textContent !== text) v.textContent = text;
}
