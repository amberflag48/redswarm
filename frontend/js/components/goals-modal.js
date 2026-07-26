// Goals modal - create/edit/delete global goals with task association.
// Mirrors the task-modal pattern: registerModal for dirty-state confirm,
// fetchJson/putJson/postJson for CRUD, setSpeedField/getSpeedBps for the
// speed fields. Reuses the same goal form structure as the task modal.

import { state } from '../state/store.js';
import { populateGoalFields, collectGoalFields, updateGoalFieldVisibility } from './goal-form.js';

// Goal field IDs for the goals modal (goal-* convention).
const GOALS_MODAL_IDS = {
  enable: 'goal-enable',
  direction: 'goal-direction',
  uploadTargetVal: 'goal-upload-target-val',
  uploadTargetUnit: 'goal-upload-target-unit',
  downloadTargetVal: 'goal-download-target-val',
  downloadTargetUnit: 'goal-download-target-unit',
  targetSecs: 'goal-target-secs',
  action: 'goal-action',
  reachedVal: 'goal-reached-val',
  reachedUnit: 'goal-reached-unit',
};
import { focusFirst, escHtml, escAttr } from '../utils/dom-helpers.js';
import { fetchJson, putJson, postJson, deleteAction, clearPending, pendingSignal } from '../utils/net.js';
import { btnLoading, btnReset } from '../utils/buttons.js';
import { openModalEl, registerModal } from './modal.js';
import { showConfirm, CONFIRM_CANCEL } from './confirm.js';
import { toast } from './toast.js';

let editingGoalId = null;

function resetGoalsModal() {
    editingGoalId = null;
    clearPending('submit-goal');
    clearPending('edit-goal');
    const btn = document.getElementById('save-goal-btn');
    if (btn) btnReset(btn);
}

export const closeGoalsModal = registerModal({ isDirty: () => false, noun: 'changes', reset: resetGoalsModal });

// Check whether the goal form has all required fields filled. The save
// button stays disabled until: (1) name is non-empty, (2) the target/time
// matching the direction is satisfied (upload: upload_target>0 or secs>0;
// download+upload: both targets>0 or secs>0), (3) at least 1 task selected.
export function validateGoalForm() {
    const btn = document.getElementById('save-goal-btn');
    if (!btn) return;
    const name = document.getElementById('goal-name').value.trim();
    const goal = collectGoalFields(GOALS_MODAL_IDS, { timeAsPicker: false });
    const hasTarget = goal.direction === 'upload'
        ? (goal.upload_target > 0 || goal.target_secs > 0)
        : ((goal.upload_target > 0 && goal.download_target > 0) || goal.target_secs > 0);
    const hasTasks = collectTaskIds().length > 0;
    btn.disabled = !name || !hasTarget || !hasTasks;
}

export function openGoalsModal() {
    editingGoalId = null;
    document.getElementById('goals-modal-title').textContent = 'New goal';
    document.getElementById('save-goal-btn').textContent = 'Create goal';
    document.getElementById('goal-name').value = '';
    populateGoalFields(GOALS_MODAL_IDS, {
      enabled: true, direction: 'upload', upload_target: 0, download_target: 0,
      target_secs: 0, reached_action: 'stop', reached_bps: 0,
    }, { timeAsPicker: false });
    renderTaskPicker([]);
    updateGoalsVisibility();
    validateGoalForm();
    openModalEl(document.getElementById('goals-modal'), closeGoalsModal);
    focusFirst(document.getElementById('goals-modal'));
}

export function editGoal(id) {
    const signal = pendingSignal('edit-goal');
    fetchJson('/api/goals/' + id, { signal }, 'Failed to load goal')
        .then(data => {
            if (!data || !data.goal) return;
            clearPending('edit-goal');
            editingGoalId = id;
            const g = data.goal;
            document.getElementById('goals-modal-title').textContent = 'Edit goal';
            document.getElementById('save-goal-btn').textContent = 'Save changes';
            document.getElementById('goal-name').value = g.name;
            populateGoalFields(GOALS_MODAL_IDS, {
              enabled: g.enabled, direction: g.direction,
              upload_target: g.upload_target, download_target: g.download_target,
              target_secs: g.target_secs, reached_action: g.reached_action,
              reached_bps: g.reached_bps,
            }, { timeAsPicker: false });
            renderTaskPicker(data.task_ids || []);
            updateGoalsVisibility();
            validateGoalForm();
            openModalEl(document.getElementById('goals-modal'), closeGoalsModal);
            focusFirst(document.getElementById('goals-modal'));
        });
}

export async function deleteGoal(id) {
    const confirmed = await showConfirm('Delete goal', 'Delete this goal? This cannot be undone.', [
        CONFIRM_CANCEL, { label: 'Delete', class: 'btn-danger', value: 'ok' }
    ]);
    if (confirmed !== 'ok') return;
    state.lastTaskActionAt = Date.now();
    const ok = await deleteAction('/api/goals/' + id, 'Failed to delete goal');
    if (ok) toast('Goal deleted', 'info');
}

// Read the task list from the already-rendered DOM (#audit-list) - no fetch
// needed. The server renders all task rows on first paint, and SSE keeps
// them live (addTaskRow/removeTaskRow). This avoids a network round-trip
// every time the goals modal opens.
//
// Tasks already associated with another goal are disabled (grayed out).
function renderTaskPicker(checkedIds) {
    const container = document.getElementById('goal-task-list');
    if (!container) return;
    const filter = document.getElementById('goal-task-filter');
    if (filter) filter.value = '';
    const rows = document.querySelectorAll('#audit-list tbody tr[data-id]');
    const checked = new Set(checkedIds);
    // Compute occupied tasks from other goals (exclude the one being edited).
    const occupied = new Set();
    for (const gid in state.globalGoals) {
        if (parseInt(gid) === editingGoalId) continue;
        const g = state.globalGoals[gid];
        if (g.task_ids) g.task_ids.forEach(t => occupied.add(t));
    }
    if (!rows.length) { container.innerHTML = '<div class="hint">No tasks available.</div>'; updateTaskCount(); return; }
    container.innerHTML = Array.from(rows).map(r => {
        const id = parseInt(r.dataset.id);
        const nameCell = r.querySelector('.name-cell');
        const name = nameCell?.textContent ?? ('#' + r.dataset.id);
        const title = nameCell?.getAttribute('title') ?? name;
        const isRunning = r.querySelector('[data-col="status"] .badge')?.classList.contains('running') ?? false;
        const tracker = r.querySelector('td[data-label="Tracker"]')?.textContent?.trim() ?? '';
        const isOccupied = occupied.has(id);
        return '<label class="task-pick" data-name="' + escAttr(name.toLowerCase()) + '">' +
            '<input type="checkbox" data-task-id="' + r.dataset.id + '"' +
            (checked.has(id) ? ' checked' : '') + (isOccupied ? ' disabled' : '') + '>' +
            '<span class="task-pick-dot' + (isRunning ? ' running' : '') + '"></span>' +
            '<span class="task-pick-name" title="' + escAttr(title) + '">' + escHtml(name) + '</span>' +
            '<span class="task-pick-tracker">' + escHtml(tracker) + '</span>' +
            '</label>';
    }).join('');
    // Apply the .disabled class to occupied rows via classList (not string
    // concat) so the dead-state-css test recognizes the application site.
    document.querySelectorAll('#goal-task-list input[data-task-id][disabled]').forEach(input => {
        input.closest('.task-pick').classList.add('disabled');
    });
    updateTaskCount();
}

export function updateTaskCount() {
    const el = document.getElementById('goal-task-count');
    if (!el) return;
    const n = document.querySelectorAll('#goal-task-list input[data-task-id]:checked').length;
    el.textContent = n + ' selected';
}

export function filterTaskPicker() {
    const q = (document.getElementById('goal-task-filter')?.value ?? '').toLowerCase().trim();
    document.querySelectorAll('#goal-task-list .task-pick').forEach(el => {
        const match = !q || (el.dataset.name ?? '').includes(q);
        el.classList.toggle('hidden', !match);
    });
}

export function pickAllTasks() {
    document.querySelectorAll('#goal-task-list .task-pick:not(.hidden) input[data-task-id]:not([disabled])').forEach(el => { el.checked = true; });
    updateTaskCount();
    validateGoalForm();
}

export function pickNoTasks() {
    document.querySelectorAll('#goal-task-list .task-pick:not(.hidden) input[data-task-id]:not([disabled])').forEach(el => { el.checked = false; });
    updateTaskCount();
    validateGoalForm();
}

function collectTaskIds() {
    const ids = [];
    document.querySelectorAll('#goal-task-list input[data-task-id]:checked').forEach(el => {
        ids.push(parseInt(el.dataset.taskId));
    });
    return ids;
}

function buildGoalBody() {
    const goal = collectGoalFields(GOALS_MODAL_IDS, { timeAsPicker: false });
    return {
        name: document.getElementById('goal-name').value.trim(),
        ...goal,
        task_ids: collectTaskIds(),
    };
}

export function submitGoal(btn) {
    if (btn.disabled) return;
    btnLoading(btn);
    const signal = pendingSignal('submit-goal');
    const body = buildGoalBody();
    state.lastTaskActionAt = Date.now();
    if (editingGoalId !== null) {
        putJson('/api/goals/' + editingGoalId, body, 'Failed to save goal', signal)
            .then(data => {
                if (!data) { btnReset(btn); return; }
                closeGoalsModal();
                toast('Goal saved', 'success');
                loadGlobalGoals();
            });
    } else {
        postJson('/api/goals', body, 'Failed to create goal', signal)
            .then(data => {
                if (!data) { btnReset(btn); return; }
                closeGoalsModal();
                toast('Goal created', 'success');
                loadGlobalGoals();
            });
    }
}

// Shared visibility driver - delegates to goal-form.js with the goals-modal
// prefix. Re-exported so init.js can call it from the change listener.
export function updateGoalsVisibility() {
    updateGoalFieldVisibility(GOALS_MODAL_IDS, { timeAsPicker: false });
    validateGoalForm();
}

// Re-exported so init.js can call it from the table-tabs click handler.
import { loadGlobalGoals } from './task-list.js';
