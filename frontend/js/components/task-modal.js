// New-task / edit-task modal - open/close, tab switching, torrent queue,
// mode + speed-strategy controls, form→config collection, and create/edit
// submit. Shared mutable state lives in `state` (single source of truth).

import { state } from '../state/store.js';
import { dom } from '../state/dom.js';
import { setSpeedField, getSpeedBps, formatBytes } from '../utils/format.js';
import { populateGoalFields, collectGoalFields, updateGoalFieldVisibility } from './goal-form.js';

// Goal field IDs for the task modal (cfg-goal-* convention).
const TASK_GOAL_IDS = {
 enable: 'cfg-goal-enable',
 direction: 'cfg-goal-direction',
 uploadTargetVal: 'cfg-goal-target-val',
 uploadTargetUnit: 'cfg-goal-target-unit',
 downloadTargetVal: 'cfg-goal-download-val',
 downloadTargetUnit: 'cfg-goal-download-unit',
 timeVal: 'cfg-goal-time-val',
 timeUnit: 'cfg-goal-time-unit',
 action: 'cfg-goal-action',
 reachedVal: 'cfg-goal-reached-val',
 reachedUnit: 'cfg-goal-reached-unit',
 collapsibleBlock: 'cfg-goal-fields',
 swapHint: 'cfg-goal-target-hint',
};
import { clamp, snapshotForm, isFormDirty } from '../utils/form.js';
import { focusFirst, setSegmented, escHtml, SEG_TRANSFER_MODE, SEG_SPEED_STRATEGY } from '../utils/dom-helpers.js';
import { fetchJson, putJson, postJson, postAction, clearPending, pendingSignal } from '../utils/net.js';
import { btnLoading, btnReset } from '../utils/buttons.js';
import { openModalEl, registerModal } from './modal.js';
import { setLogPanel } from './log-panel.js';
import { toast } from './toast.js';
import { NEW_TASK, EDIT_TASK, START_TASK, SAVE_CHANGES } from '../data/labels.js';

export function openModal() {
 state.editingId = null;
 state.editConfig = null;
 state.editFormSnapshot = "";
 dom.torrent_input_section.classList.remove('hidden');
 dom.modal_title.textContent = NEW_TASK;
 dom.start_audit_btn.textContent = START_TASK;
 dom.start_audit_btn.disabled = true; // no torrents yet - re-enabled by showMetas
 openModalEl(dom.modal, closeModal);
 // Reset all form fields to their HTML default values. Without this, the
 // browser's autofill or values left from a previous edit session persist
 // into a new-task creation - e.g. jitter showing 120 (a stale autofill)
 // instead of its default 20.
 dom.config_section.querySelectorAll('input[type="number"]').forEach(el => { el.value = el.defaultValue; });
 dom.config_section.querySelectorAll('input[type="checkbox"]').forEach(el => { el.checked = el.defaultChecked; });
 dom.config_section.querySelectorAll('select').forEach(el => { el.value = el.querySelector('option[selected]')?.value || el.options[0]?.value || ''; });
 const d = state.settingsConfig?.defaults || {};
 const sd = state.settingsConfig?.swarm_defaults || {};
 setSpeedField('cfg-upload-val', 'cfg-upload-unit', d.upload_bps ?? 0);
 setSpeedField('cfg-download-val', 'cfg-download-unit', d.download_bps ?? 0);
 setSpeedField('cfg-max-upload-val', 'cfg-max-upload-unit', sd.max_upload_bps ?? 0);
 setSpeedField('cfg-max-download-val', 'cfg-max-download-unit', sd.max_download_bps ?? 0);
 // Goal defaults from [defaults] - shared populateGoalFields from goal-form.js.
 populateGoalFields(TASK_GOAL_IDS, {
 enabled: !!d.goal_enabled,
 direction: d.goal_direction || 'upload',
 upload_target: d.goal_upload_target ?? 0,
 download_target: d.goal_download_target ?? 0,
 target_secs: d.goal_target_secs ?? 0,
 reached_action: d.goal_reached_action || 'stop',
 reached_bps: d.goal_reached_bps ?? 0,
 }, { timeAsPicker: true });
 updateGoalFieldVisibility(TASK_GOAL_IDS, { timeAsPicker: true });
 focusFirst(dom.modal);
 setMode('download_and_upload');
 setSpeedMode('dynamic');
}

// Reset the new/edit modal to its initial state after the overlay is closed.
function resetTaskModal() {
 dom.meta_preview.classList.add('hidden');
 dom.config_section.classList.add('hidden');
 btnReset(dom.start_audit_btn);
 dom.start_audit_btn.textContent = START_TASK;
 dom.modal_title.textContent = NEW_TASK;
 dom.torrent_input_section.classList.remove('hidden');
 dom.meta_preview.classList.add('section-divider');
 dom.file_input.value = '';
 dom.magnet_input.value = '';
 state.pendingMetas = [];
 if (state.parsedLinks) state.parsedLinks.clear();
 state.editingId = null;
 state.editConfig = null;
 state.editFormSnapshot = '';
 clearPending('submit-task');
 clearPending('edit-audit');
}

export const closeModal = registerModal({ isDirty: editIsDirty, noun: 'changes', reset: resetTaskModal });

// Tabs
export function switchTab(tab) {
 dom.tab_file.classList.toggle('active', tab === 'file');
 dom.tab_magnet.classList.toggle('active', tab === 'magnet');
 dom.tab_file.setAttribute('aria-selected', tab === 'file');
 dom.tab_magnet.setAttribute('aria-selected', tab === 'magnet');
 dom.input_file.classList.toggle('hidden', tab !== 'file');
 dom.input_magnet.classList.toggle('hidden', tab !== 'magnet');
}

// Parse each .torrent file and append to the queue. Files are parsed in
// parallel; each successful parse adds one entry to pendingMetas.
export function handleFiles(files) {
 Array.from(files).forEach(file => {
 file.arrayBuffer().then(buf => fetchJson('/api/parse-torrent', { method: 'POST', body: buf }, 'Parse error (' + file.name + ')'))
 .then(meta => { if (meta) { state.pendingMetas.push(meta); showMetas(); } });
 });
}

// Check if a meta's info_hash + announce_url already exists in the task list
// DOM (server-rendered, kept live by SSE) or earlier in the current batch.
// Returns the duplicate task id (if from DOM) or -1 (if within-batch dup).
function findDuplicate(meta, batchIdx) {
 // Check existing task rows in the DOM - zero fetch, instant.
 const rows = document.querySelectorAll('#audit-list tbody tr[data-info-hash]');
 for (const row of rows) {
 if (row.dataset.infoHash === meta.info_hash && row.dataset.announceUrl === meta.announce_url) {
 const rowId = parseInt(row.dataset.id);
 if (state.editingId !== null && rowId === state.editingId) continue;
 return rowId;
 }
 }
 // Check earlier entries in the current batch.
 for (let i = 0; i < batchIdx; i++) {
 const m = state.pendingMetas[i];
 if (m.info_hash === meta.info_hash && m.announce_url === meta.announce_url) return -1;
 }
 return null;
}

// Render the torrent queue. For a single torrent, show full details (name,
// tracker, hash, size). For multiple, show a compact list with remove buttons.
// Duplicates (same info_hash + announce_url as an existing task or an earlier
// entry in the batch) are flagged with a warning and block submission.
export function showMetas() {
 dom.meta_preview.classList.remove('hidden');
 dom.config_section.classList.remove('hidden');
 const n = state.pendingMetas.length;
 let dupCount = 0;
 // Build per-meta duplicate flags.
 const dups = state.pendingMetas.map((m, i) => {
 const dup = findDuplicate(m, i);
 if (dup !== null) dupCount++;
 return dup;
 });
 if (n === 1) {
 const m = state.pendingMetas[0];
 const dupRow = dups[0] !== null
 ? '<div class="meta-row meta-dup"><span class="meta-label">Duplicate</span><span class="meta-value">' + (dups[0] > 0 ? 'Already task #' + dups[0] : 'Already in this batch') + '</span></div>'
 : '';
 dom.meta_preview.innerHTML = '<div class="meta-row"><span class="meta-label">Name</span><span class="meta-value">' + escHtml(m.name || '(unknown)') + '</span></div><div class="meta-row"><span class="meta-label">Tracker</span><span class="meta-value">' + escHtml(m.announce_url) + '</span></div><div class="meta-row"><span class="meta-label">Hash</span><span class="meta-value">' + escHtml(m.info_hash) + '</span></div><div class="meta-row"><span class="meta-label">Size</span><span class="meta-value">' + formatBytes(m.torrent_size) + '</span></div>' + dupRow;
 } else if (n > 1) {
 dom.meta_preview.innerHTML = '<div class="meta-row"><span class="meta-label">' + escHtml(String(n)) + ' torrents queued</span></div>' + state.pendingMetas.map((m, i) => {
 const dupTag = dups[i] !== null ? ' <span class="meta-dup-tag">' + (dups[i] > 0 ? escHtml('#' + dups[i]) : 'dup') + '</span>' : '';
 return '<div class="meta-row' + (dups[i] !== null ? ' meta-dup' : '') + '"><span class="meta-value">' + escHtml(m.name || '(unknown)') + '</span>' + dupTag + '<button class="meta-remove" data-idx="' + i + '" aria-label="Remove">\u00d7</button></div>';
 }).join('');
 }
 // Disable start button in create mode if duplicates detected or no metas.
 if (state.editingId === null) {
 dom.start_audit_btn.disabled = state.pendingMetas.length === 0 || dupCount > 0;
 }
}

// Mode switching
const MODE_HINTS = {
 download_and_upload: 'Leech then seed - simulates download progress before growing upload. Stealthier.',
 upload_only: 'Ghost seed - starts as seeder immediately. Faster but easier to detect.',
};

const STRATEGY_HINTS = {
 dynamic: 'Adapts upload speed to live seeder/leecher counts from announce responses.',
 fixed: 'Constant manual upload speed. Simpler but less adaptive.',
};

export function setMode(mode) {
 state.currentMode = mode;
 setSegmented(SEG_TRANSFER_MODE, mode);
 document.getElementById('cfg-mode-hint').textContent = MODE_HINTS[mode] || '';
 updateVisibility();
 updateEditSaveButton();
}

export function setSpeedMode(mode) {
 state.currentSpeedMode = mode;
 setSegmented(SEG_SPEED_STRATEGY, mode);
 document.getElementById('cfg-strategy-hint').textContent = STRATEGY_HINTS[mode] || '';
 dom.dynamic_fields.classList.toggle('open', mode === 'dynamic');
 updateVisibility();
 updateEditSaveButton();
}

function updateVisibility() {
 dom.download_speed_field.classList.toggle('hidden', state.currentMode === 'upload_only' || state.currentSpeedMode === 'dynamic');
 dom.start_pct_field.classList.toggle('hidden', state.currentMode === 'upload_only');
 dom.upload_speed_field.classList.toggle('hidden', state.currentSpeedMode === 'dynamic');
 document.getElementById('cfg-freeze-download-row').classList.toggle('hidden', state.currentMode === 'upload_only');
 updateGoalFieldVisibility(TASK_GOAL_IDS, { timeAsPicker: true });
}

// Re-export the shared visibility driver so init.js's change listener can call
// it without importing goal-form.js directly (single import path per module).
export function updateGoalVisibility() {
 updateGoalFieldVisibility(TASK_GOAL_IDS, { timeAsPicker: true });
}

// Build the AuditConfig object from the form. In edit mode, non-editable
// swarm fields (avg_leecher_download_bps, seed_share_factor) are preserved
// from the stored editConfig; in create mode they use the hardcoded defaults.
function buildConfigFromForm() {
 const swarmBase = (state.editingId !== null && state.editConfig)
 ? state.editConfig.swarm
 : { avg_leecher_download_bps: state.settingsConfig?.swarm_defaults?.avg_leecher_download_bps ?? 0, seed_share_factor: state.settingsConfig?.swarm_defaults?.seed_share_factor ?? 0 };
 return {
 announce_url: '',
 info_hash: '',
 torrent_size: 0,
 upload_bps: getSpeedBps('cfg-upload-val','cfg-upload-unit'),
 jitter_pct: clamp(parseInt(document.getElementById('cfg-jitter').value) || (state.settingsConfig?.defaults?.jitter_pct ?? 20), 0, 100),
 ramp_up_secs: clamp(parseInt(document.getElementById('cfg-ramp').value) || (state.settingsConfig?.defaults?.ramp_up_secs ?? 120), 0, 86400),
 mode: state.currentMode,
 download_bps: getSpeedBps('cfg-download-val','cfg-download-unit'),
 freeze_on_zero_leechers: document.getElementById('cfg-freeze-upload').checked,
 freeze_on_zero_seeders: document.getElementById('cfg-freeze-download').checked,
 start_download_pct: clamp(parseInt(document.getElementById('cfg-start-pct').value) || (state.settingsConfig?.defaults?.start_download_pct ?? 0), 0, 100),
 speed_mode: state.currentSpeedMode,
 swarm: {
 ...swarmBase,
 fair_share_multiplier: clamp(parseFloat(document.getElementById('cfg-fair-share-mult').value) || (state.settingsConfig?.swarm_defaults?.fair_share_multiplier ?? 1.0), 0.1, 5.0),
 max_upload_bps: getSpeedBps('cfg-max-upload-val','cfg-max-upload-unit'),
 max_download_bps: getSpeedBps('cfg-max-download-val','cfg-max-download-unit'),
 },
 goal: collectGoalFields(TASK_GOAL_IDS, { timeAsPicker: true }),
 forced_client: document.getElementById('cfg-client').value || null,
 };
}

// Submit (create+start or edit+save)
// In edit mode, the Save button is disabled until the form diverges from the
// DOM snapshot taken when the modal opened. Reverting a field back to its
// original value re-disables Save - no diff, no save.
export function updateEditSaveButton() {
 if (state.editingId === null || !state.editFormSnapshot) return;
 dom.start_audit_btn.disabled = snapshotForm(dom.config_section) === state.editFormSnapshot;
}

function editIsDirty() {
 return isFormDirty(dom.config_section, state.editFormSnapshot);
}

export function submitTask(btn) {
 if (state.pendingMetas.length === 0 || btn.disabled) return;
 btnLoading(btn);
 const signal = pendingSignal('submit-task');
 if (state.editingId !== null) {
 // Edit mode - PUT config only, no auto-start
 state.lastTaskActionAt = Date.now();
 putJson('/api/audits/' + state.editingId, { config: buildConfigFromForm() }, 'Failed to save task', signal)
 .then(data => {
 if (!data) { btnReset(btn); return; }
 const savedId = state.editingId;
 // The save succeeded - the form's current state IS the saved
 // state. Re-sync the dirty-detection snapshot so closeModal's
 // confirmDiscardIfDirty sees a clean form and skips the "unsaved
 // changes" prompt. Without this, a successful save still prompts
 // to discard (the form diverged from the edit-load snapshot the
 // moment the user enabled Save by changing a field).
 state.editFormSnapshot = snapshotForm(dom.config_section);
 closeModal();
 if (savedId === state.activeLogId && data.log_html) setLogPanel(savedId, data.log_html);
 toast(data.unchanged ? 'No changes' : (data.restarted ? 'Task saved and restarted' : 'Task saved'), data.unchanged ? 'info' : 'success');
 });
 } else {
 // Create mode - POST + auto-start one task per torrent, all sharing
 // the same config. Identity fields (announce_url, info_hash,
 // torrent_size) come from each meta; the config's identity fields
 // are left empty and filled by the backend.
 const config = buildConfigFromForm();
 state.lastTaskActionAt = Date.now();
 const promises = state.pendingMetas.map(meta => {
 const body = {
 name: meta.name || 'Unnamed',
 announce_url: meta.announce_url, info_hash: meta.info_hash, torrent_size: meta.torrent_size,
 config,
 };
 return postJson('/api/audits', body, 'Failed to create task', signal)
 .then(data => {
 if (data && data.id) {
 return postAction('/api/audits/' + data.id + '/start', 'Failed to start task #' + data.id, signal)
 .then(() => data);
 }
 return null;
 });
 });
 Promise.all(promises).then(results => {
 closeModal();
 const valid = results.filter(r => r !== null);
 if (valid.length === 1) {
 setLogPanel(valid[0].id, valid[0].log_html);
 toast('Task started', 'success');
 } else if (valid.length > 1) {
 toast('Started ' + valid.length + ' tasks', 'success');
 }
 });
 }
}

// Edit task - load existing config into the modal in edit mode
export function editAudit(id) {
 const signal = pendingSignal('edit-audit');
 fetchJson('/api/audits/' + id, { signal }, 'Failed to load task')
 .then(data => {
 if (!data || !data.config) return;
 clearPending('edit-audit');
 state.editingId = id;
 state.editConfig = data.config;
 state.pendingMetas = [{ name: data.name, announce_url: data.announce_url, info_hash: data.info_hash, torrent_size: data.torrent_size }];
 // Lock the torrent identity - hide the picker, show meta read-only
 dom.torrent_input_section.classList.add('hidden');
 dom.meta_preview.classList.remove('section-divider');
 showMetas();
 // Populate config fields from the stored config
 setMode(data.config.mode);
 setSpeedMode(data.config.speed_mode);
 setSpeedField('cfg-upload-val', 'cfg-upload-unit', data.config.upload_bps);
 setSpeedField('cfg-download-val', 'cfg-download-unit', data.config.download_bps);
 document.getElementById('cfg-jitter').value = data.config.jitter_pct;
 document.getElementById('cfg-ramp').value = data.config.ramp_up_secs;
 document.getElementById('cfg-start-pct').value = data.config.start_download_pct;
 document.getElementById('cfg-fair-share-mult').value = data.config.swarm.fair_share_multiplier;
 setSpeedField('cfg-max-upload-val', 'cfg-max-upload-unit', data.config.swarm.max_upload_bps);
 setSpeedField('cfg-max-download-val', 'cfg-max-download-unit', data.config.swarm.max_download_bps);
 document.getElementById('cfg-freeze-upload').checked = data.config.freeze_on_zero_leechers;
 document.getElementById('cfg-freeze-download').checked = data.config.freeze_on_zero_seeders;
 document.getElementById('cfg-client').value = data.config.forced_client || '';
 // Goal fields - shared populateGoalFields from goal-form.js.
 const g = data.config.goal || {};
 populateGoalFields(TASK_GOAL_IDS, {
 enabled: g.enabled,
 direction: g.direction,
 upload_target: g.upload_target,
 download_target: g.download_target,
 target_secs: g.target_secs,
 reached_action: g.reached_action,
 reached_bps: g.reached_bps,
 }, { timeAsPicker: true });
 updateGoalVisibility();
 // Switch modal to edit mode
 dom.modal_title.textContent = EDIT_TASK;
 dom.start_audit_btn.textContent = SAVE_CHANGES;
 dom.start_audit_btn.disabled = true; // grayed out until a field changes
 openModalEl(dom.modal, closeModal);
 // Snapshot the form state for dirty-state tracking. Must be captured
 // AFTER all fields are populated so buildConfigFromForm reflects the
 // loaded config exactly.
 state.editFormSnapshot = snapshotForm(dom.config_section);
 });
}
