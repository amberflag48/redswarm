// App bootstrap - the single entry point. Called once on DOMContentLoaded by
// main.js. Wires every event listener (replacing the inline onclick handlers
// that lived in index.html), populates byte-unit selects, then kicks off the
// initial data fetches + the global SSE connection.

import { state } from '../state/store.js';
import { dom, cacheDom } from '../state/dom.js';
import { clampNumberOnBlur } from '../utils/form.js';
import { fetchJson, clearAllPending } from '../utils/net.js';
import { openSettingsModal, saveSettings, closeSettingsModal, switchSettingsSection, addSettingsClient, removeSettingsClient, updateSaveButton, updateSettingsGoalVisibility } from '../components/settings-modal.js';
import { openCaptureModal, closeCaptureModal, newCapture, copyCaptureSnippet, addCapturedClient } from '../components/capture-modal.js';
import { openGoalsModal, editGoal, deleteGoal, submitGoal, updateGoalsVisibility, closeGoalsModal, filterTaskPicker, pickAllTasks, pickNoTasks, updateTaskCount, validateGoalForm } from '../components/goals-modal.js';
import { openModal, closeModal, switchTab, handleFiles, showMetas, setMode, setSpeedMode, submitTask, editAudit, updateEditSaveButton, updateGoalVisibility } from '../components/task-modal.js';
import { stopAudit, restartAudit, deleteAudit, viewLog, initTopbarCounts, loadGlobalGoals, switchTableTab } from '../components/task-list.js';
import { resolveConfirm } from '../components/confirm.js';
import { wireModalClose } from '../components/modal.js';
import { toggleClientCard } from '../components/client-card.js';
import { connectGlobalSSE } from '../services/sse.js';
import { applyRuntimeSettings } from '../services/runtime.js';
import { wireSegmented, SEG_TRANSFER_MODE, SEG_SPEED_STRATEGY } from '../utils/dom-helpers.js';

export function init() {
 cacheDom();

 // Cancel any in-flight fetches when the page is hidden/unloaded so a
 // stale response can never update DOM that's already going away.
 window.addEventListener('pagehide', clearAllPending);

 // Byte-unit selects + client dropdown are server-rendered - no JS
 // option-building needed. JS only wires event listeners + reads state.

 // Topbar
 document.getElementById('settings-btn').addEventListener('click', openSettingsModal);
 document.getElementById('new-audit-btn').addEventListener('click', () => {
 if (state.activeTab === 'goals') openGoalsModal();
 else openModal();
 });

 // Table tabs (Tasks / Goals)
 document.getElementById('table-tabs').addEventListener('click', e => {
 const btn = e.target.closest('button[data-tab]');
 if (!btn) return;
 switchTableTab(btn.dataset.tab);
 });

 // New-task modal
 wireModalClose(dom.modal, closeModal);
 dom.tab_file.addEventListener('click', () => switchTab('file'));
 dom.tab_magnet.addEventListener('click', () => switchTab('magnet'));
 dom.start_audit_btn.addEventListener('click', () => submitTask(dom.start_audit_btn));
 // Segmented controls - data-value drives the action
 wireSegmented(SEG_SPEED_STRATEGY, setSpeedMode);
 wireSegmented(SEG_TRANSFER_MODE, setMode);

 // Capture modal
 const captureModal = document.getElementById('capture-modal');
 captureModal.addEventListener('click', e => { if (e.target === e.currentTarget) closeCaptureModal(); });
 captureModal.querySelector('.modal-close').addEventListener('click', closeCaptureModal);
 const captureSecondary = document.querySelectorAll('#capture-snippet-section button.btn-secondary');
 if (captureSecondary[0]) captureSecondary[0].addEventListener('click', newCapture);
 if (captureSecondary[1]) captureSecondary[1].addEventListener('click', copyCaptureSnippet);
 document.getElementById('capture-add-client-btn').addEventListener('click', addCapturedClient);

 // Settings modal
 const settingsModal = document.getElementById('settings-modal');
 wireModalClose(settingsModal, closeSettingsModal);
 const saveBtn = document.getElementById('save-settings-btn');
 saveBtn.addEventListener('click', () => saveSettings(saveBtn));

 // Confirm modal
 const confirmModal = document.getElementById('confirm-modal');
 confirmModal.addEventListener('click', e => { if (e.target === e.currentTarget) resolveConfirm('cancel'); });
 confirmModal.querySelector('.modal-close').addEventListener('click', () => resolveConfirm('cancel'));

 // Goals modal
 const goalsModal = document.getElementById('goals-modal');
 wireModalClose(goalsModal, closeGoalsModal);
 const saveGoalBtn = document.getElementById('save-goal-btn');
 saveGoalBtn.addEventListener('click', () => submitGoal(saveGoalBtn));
 document.getElementById('goal-direction').addEventListener('change', updateGoalsVisibility);
 document.getElementById('goal-action').addEventListener('change', updateGoalsVisibility);
 document.getElementById('goal-name').addEventListener('input', validateGoalForm);
 document.getElementById('goal-upload-target-val').addEventListener('input', validateGoalForm);
 document.getElementById('goal-download-target-val').addEventListener('input', validateGoalForm);
 document.getElementById('goal-target-secs').addEventListener('input', validateGoalForm);
 document.getElementById('goal-task-filter').addEventListener('input', filterTaskPicker);
 document.getElementById('goal-task-list').addEventListener('change', e => { if (e.target.matches('input[data-task-id]')) { updateTaskCount(); validateGoalForm(); } });
 goalsModal.querySelector('[data-pick="all"]').addEventListener('click', pickAllTasks);
 goalsModal.querySelector('[data-pick="none"]').addEventListener('click', pickNoTasks);

 // Goals table delegation (edit/delete from the goals tab)
 dom.goal_list.addEventListener('click', e => {
 const btn = e.target.closest('button[data-action]');
 if (!btn) return;
 const id = parseInt(btn.dataset.id);
 const action = btn.dataset.action;
 if (action === 'edit-goal') editGoal(id);
 else if (action === 'delete-goal') deleteGoal(id);
 });

 // Drop zone
 const dz = dom.dropzone;
 const fi = dom.file_input;
 dz.addEventListener('click', () => fi.click());
 dz.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') fi.click(); });
 dz.addEventListener('dragover', e => { e.preventDefault(); dz.classList.add('drag'); });
 dz.addEventListener('dragleave', () => dz.classList.remove('drag'));
 dz.addEventListener('drop', e => { e.preventDefault(); dz.classList.remove('drag'); if (e.dataTransfer.files.length) handleFiles(e.dataTransfer.files); });
 fi.addEventListener('change', e => { if (e.target.files.length) handleFiles(e.target.files); });

 // Magnet input - debounced parse, one task per magnet line
 state.parsedLinks = new Set();
 dom.magnet_input.addEventListener('input', function() {
 clearTimeout(state.magnetTimer);
 const uri = this.value.trim();
 if (!uri.toLowerCase().startsWith('magnet:?')) return;
 state.magnetTimer = setTimeout(() => {
 const links = uri.split(/\n+/).map(s => s.trim()).filter(s => s.toLowerCase().startsWith('magnet:?'));
 links.forEach(link => {
 if (state.parsedLinks.has(link)) return;
 state.parsedLinks.add(link);
 fetchJson('/api/parse-magnet', { method: 'POST', body: link }, 'Magnet parse failed')
 .then(meta => { if (meta) { state.pendingMetas.push(meta); showMetas(); } });
 });
 }, 400);
 });

 // Audit list - event delegation for row + action buttons
 dom.audit_list.addEventListener('click', e => {
 const btn = e.target.closest('button[data-action]');
 if (btn) {
 e.stopPropagation();
 const id = parseInt(btn.dataset.id);
 const action = btn.dataset.action;
 if (action === 'stop') stopAudit(id);
 else if (action === 'start') restartAudit(id);
 else if (action === 'delete') deleteAudit(id);
 else if (action === 'edit') editAudit(id);
 return;
 }
 const row = e.target.closest('tr[data-id]');
 if (row) {
 const id = parseInt(row.dataset.id);
 viewLog(id);
 }
 });

 // Meta preview - remove a torrent from the create-mode queue
 dom.meta_preview.addEventListener('click', e => {
 const btn = e.target.closest('button.meta-remove');
 if (!btn) return;
 const idx = parseInt(btn.dataset.idx);
 state.pendingMetas.splice(idx, 1);
 showMetas();
 });

 // Arrow-key navigation for segmented controls (radiogroup pattern)
 document.querySelectorAll('.segmented[role="radiogroup"]').forEach(group => {
 group.addEventListener('keydown', e => {
 const buttons = Array.from(group.querySelectorAll('button[role="radio"]'));
 const currentIdx = buttons.indexOf(document.activeElement);
 if (currentIdx === -1) return;
 let newIdx = null;
 if (e.key === 'ArrowRight' || e.key === 'ArrowDown') newIdx = (currentIdx + 1) % buttons.length;
 else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') newIdx = (currentIdx - 1 + buttons.length) % buttons.length;
 if (newIdx !== null) { e.preventDefault(); buttons[newIdx].focus(); buttons[newIdx].click(); }
 });
 });

 // Config form (new-task modal) - dirty-state tracking + clamping
 dom.config_section.addEventListener('input', updateEditSaveButton);
 dom.config_section.addEventListener('change', (e) => {
 updateEditSaveButton();
 if (e.target && (e.target.id === 'cfg-goal-enable' || e.target.id === 'cfg-goal-action' || e.target.id === 'cfg-goal-direction')) updateGoalVisibility();
 });
 dom.config_section.addEventListener('blur', clampNumberOnBlur, true);

 // Settings form - dirty-state tracking + clamping
 const settingsContent = document.getElementById('settings-content');
 settingsContent.addEventListener('input', updateSaveButton);
 settingsContent.addEventListener('change', (e) => {
 updateSaveButton();
 if (e.target && ['set-defaults-goal_enabled', 'set-defaults-goal_direction', 'set-defaults-goal_reached_action'].includes(e.target.id)) updateSettingsGoalVisibility();
 });
 settingsContent.addEventListener('blur', clampNumberOnBlur, true);

 // Settings nav - section switching via data-section
 document.getElementById('settings-nav').addEventListener('click', e => {
 const btn = e.target.closest('button[data-section]');
 if (btn) switchSettingsSection(btn.dataset.section);
 });

 // Settings clients - event delegation for card + pane actions
 settingsContent.addEventListener('click', e => {
 const target = e.target.closest('[data-action]');
 if (!target) return;
 const action = target.dataset.action;
 if (action === 'add-settings-client') { addSettingsClient(); }
 else if (action === 'open-capture-modal') { openCaptureModal(); }
 else if (action === 'toggle-client-card') {
 const card = target.closest('.client-card');
 if (card) toggleClientCard(card.dataset.idx);
 } else if (action === 'remove-client') {
 e.stopPropagation();
 const card = target.closest('.client-card');
 if (card) removeSettingsClient(card.dataset.idx);
 }
 });

 // Initial load
 // The server already rendered the full task list, topbar stats, and log
 // panel into the HTML - first paint IS the final DOM. JS only reads
 // state from the existing DOM + bootstrap JSON; it never rebuilds the
 // initial view (no hydration swap, no layout shift, no forced reflow).
 // SSE-driven new rows carry a pre-rendered `html` field from the server,
 // so the JS never builds row HTML either - single source of truth.
 connectGlobalSSE();
 const data = window.__BOOTSTRAP__;
 if (!data) { fetchJson('/api/bootstrap', null, 'Bootstrap failed').then(readStateFromDom); return; }
 readStateFromDom(data);

 // Read state from the server-rendered DOM. The task list, topbar stats,
 // and log panel are already present - we just populate the client dropdown
 // (not server-rendered), apply runtime tunables, and read the counts /
 // active-log-id / log totals the SSE handlers need.
 function readStateFromDom(data) {
 if (!data) return;
 // Client dropdown is server-rendered - just build clientMap from the
 // existing <option> elements (no fetch, no rebuild).
 state.clientMap = {};
 document.querySelectorAll('#cfg-client option').forEach(opt => {
 if (opt.value) state.clientMap[opt.value] = opt.textContent;
 });
 // Runtime tunables (logMaxRows / currentMode / currentSpeedMode).
 if (data.settings) { state.settingsConfig = data.settings; applyRuntimeSettings(data.settings); }
 // Topbar counts from the server-rendered `.stat .val` tiles.
 initTopbarCounts();
 // Global goals from /api/goals (the topbar tiles are server-rendered
 // for first paint, but the JS needs the state to patch them live).
 loadGlobalGoals();
 // Active log id from the server-rendered `.active` row.
 const activeRow = dom.audit_list.querySelector('tbody tr.active');
 if (activeRow) {
 state.activeLogId = parseInt(activeRow.dataset.id);
 localStorage.setItem('activeLogId', state.activeLogId);
 }
 // Log totals from the server-rendered log table (SSE appendLogRow
 // maintains these going forward).
 state.logTotalRows = dom.log_panel.querySelectorAll('tbody tr').length;
 state.logSuccessCount = dom.log_panel.querySelectorAll('.badge.ok').length;
 // Mark first load done so afterListUpdate doesn't re-fetch the log.
 state.firstLoadDone = true;
 }
}
