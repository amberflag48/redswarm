// Global SSE - a single EventSource('/api/events') connection drives all
// dynamic UI. Native EventSource auto-reconnects on error (WHATWG §9.2.3); the
// onerror handler just flips the badge to 'reconnecting' - we never call
// close(), which would move to CLOSED and halt all reconnection. The browser
// also re-sends Last-Event-ID on reconnect (WHATWG §9.2.4).

import { state, shouldSuppressToast } from '../state/store.js';
import { addTaskRow, removeTaskRow, setTaskStatus, setTaskClient, setTaskProgress, setTaskUpdated, loadTaskList, loadGoalList, updateGoalFromAudit, loadGlobalGoals, patchGoalTile, removeGoalTile } from '../components/task-list.js';
import { appendLogRow, updateLogStats, loadLog } from '../components/log-panel.js';
import { updateCaptureUI, showCaptureSnippet } from '../components/capture-modal.js';
import { populateSettingsFields, renderSettingsClients, settingsIsDirty, updateSaveButton } from '../components/settings-modal.js';
import { snapshotForm } from '../utils/form.js';
import { clientDisplayName } from '../data/client-schema.js';
import { applyRuntimeSettings } from './runtime.js';
import { refreshClientDropdown } from './clients.js';
import { toast } from '../components/toast.js';
import { formatDuration } from '../utils/format.js';
import { EMPTY_DASH } from '../data/labels.js';

// Update the connection badge in the topbar. `s` is one of
// 'connected' | 'reconnecting'. The native EventSource auto-reconnects on
// error (WHATWG §9.2.3) and we never call close() (which would move to CLOSED
// and halt all reconnection), so there is no permanent "disconnected" state -
// the badge only ever flips between Live and Reconnecting.
function setConnState(s) {
    const badge = document.getElementById('conn-badge');
    if (!badge) return;
    badge.className = 'conn-badge ' + s;
    const label = badge.querySelector('.conn-label');
    const labels = { connected: 'Live', reconnecting: 'Reconnecting' };
    if (label) label.textContent = labels[s] || s;
}

export function connectGlobalSSE() {
    const sse = new EventSource('/api/events');
    // First open just flips the badge to "connected". On a RECONNECT, also
    // reconcile state that may have drifted while the stream was down - SSE
    // has no event-ID replay here, so task created/deleted/status events
    // missed during the drop are recovered by re-fetching the task list and
    // client dropdown (cheap, and only on reconnect).
    let firstOpen = true;
    sse.onopen = () => {
        setConnState('connected');
        if (firstOpen) { firstOpen = false; return; }
        // Reconcile state that may have drifted while the stream was down.
        // loadTaskList re-fetches the task list (missed task_created/deleted/
        // status events). The client dropdown is rebuilt from the cached
        // config - if a config_reloaded was missed, the next one fixes it.
        // The log panel is reloaded to pick up missed audit events.
        loadTaskList();
        loadGoalList();
        loadGlobalGoals();
        if (state.activeLogId) loadLog(state.activeLogId);
        if (state.settingsConfig) {
            refreshClientDropdown(state.settingsConfig.clients.map(c => [c.peer_id_prefix, clientDisplayName(c)]));
        } else {
            refreshClientDropdown();
        }
    };
    sse.addEventListener('audit', e => {
        const ev = JSON.parse(e.data);
        if (ev.audit_id === state.activeLogId) { appendLogRow(ev); updateLogStats(ev); }
        // Keep the topbar goal aggregate live for EVERY task (the server
        // broadcasts all audit events; only the active task updates the log).
        updateGoalFromAudit(ev);
    });
    sse.addEventListener('task_created', e => {
        const task = JSON.parse(e.data);
        addTaskRow(task);
        if (!shouldSuppressToast()) toast('Task created: ' + task.name, 'success');
    });
    sse.addEventListener('task_deleted', e => {
        const { id } = JSON.parse(e.data);
        removeTaskRow(id);
        if (!shouldSuppressToast()) toast('Task #' + id + ' deleted', 'info');
    });
    sse.addEventListener('task_status', e => {
        const { id, status } = JSON.parse(e.data);
        setTaskStatus(id, status);
        if (!shouldSuppressToast()) toast('Task #' + id + ' ' + status, 'info');
    });
    sse.addEventListener('task_client', e => {
        const { id, working_client } = JSON.parse(e.data);
        setTaskClient(id, working_client);
    });
    sse.addEventListener('task_progress', e => {
        const { id, uploaded, downloaded } = JSON.parse(e.data);
        setTaskProgress(id, uploaded, downloaded);
    });
    sse.addEventListener('task_updated', e => {
        const task = JSON.parse(e.data);
        setTaskUpdated(task);
        if (!shouldSuppressToast()) toast('Task #' + task.id + ' updated', 'info');
    });
    // config.toml was hot-reloaded at runtime. The SSE payload carries the
    // full new AppConfig - we update the cache, runtime tunables, client
    // dropdown, and (if open + not dirty) the settings modal, all from the
    // payload with zero re-fetches. No loadTaskList() either - running audits
    // are frozen on their startup config, and client names in existing rows
    // update via task_client SSE events using the refreshed clientMap.
    sse.addEventListener('config_reloaded', e => {
        state.settingsSaveAt = 0;
        const cfg = JSON.parse(e.data);
        state.settingsConfig = cfg;
        applyRuntimeSettings(cfg);
        refreshClientDropdown(cfg.clients.map(c => [c.peer_id_prefix, clientDisplayName(c)]));
        // If the settings modal is open and the user hasn't started editing,
        // surgically update field values + client cards + re-snapshot so the
        // form reflects the new config without clobbering in-progress edits.
        if (state.settingsModalOpen && !settingsIsDirty()) {
            populateSettingsFields();
            renderSettingsClients();
            state.settingsFormSnapshot = snapshotForm(document.getElementById('settings-content'));
            updateSaveButton();
        }
        if (!shouldSuppressToast()) toast('Config reloaded', 'success');
    });
    // Capture progress - drives the capture modal via SSE, no polling. The
    // backend broadcasts on every state-machine transition (announce captured,
    // handshake captured, ext-handshake captured, keepalive measured, connection
    // ended). Only events for the active capture token update the UI.
    sse.addEventListener('capture_progress', e => {
        if (!state.captureToken) return;
        const data = JSON.parse(e.data);
        if (data.token !== state.captureToken) return;
        updateCaptureUI(data);
        if (data.status === 'ext_handshake_captured') {
            showCaptureSnippet(data.fingerprint);
        }
    });
    // Global goals - a new goal appeared, was deleted, or was edited. The UI
    // re-fetches the goals table HTML + re-renders the topbar tiles.
    sse.addEventListener('goal_created', e => {
        loadGoalList();
        loadGlobalGoals();
        if (!shouldSuppressToast()) toast('Goal created', 'success');
    });
    sse.addEventListener('goal_deleted', e => {
        const { id } = JSON.parse(e.data);
        removeGoalTile(id);
        loadGoalList();
        if (!shouldSuppressToast()) toast('Goal deleted', 'info');
    });
    sse.addEventListener('goal_updated', e => {
        loadGoalList();
        loadGlobalGoals();
    });
    // Goal progress - a goal's summed counters advanced. Patches the topbar
    // tile's .val (ETA) in place - no re-fetch, no re-render.
    sse.addEventListener('goal_progress', e => {
        const g = JSON.parse(e.data);
        patchGoalTile(g.id, g.eta_secs === null ? EMPTY_DASH : formatDuration(g.eta_secs));
    });
    sse.onerror = () => setConnState('reconnecting');
}
