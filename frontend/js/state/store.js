// Central mutable state - single source of truth for all app state.
// Every component imports { state } and reads/writes fields directly.
export const state = {
  activeLogId: null,
  activeTab: 'tasks',
  editingId: null,
  editConfig: null,
  editFormSnapshot: '',
  pendingMetas: [],
  clientMap: {},
  currentMode: 'download_and_upload',
  currentSpeedMode: 'dynamic',
  settingsConfig: null,
  settingsModalOpen: false,
  settingsActiveSection: 'server',
  settingsSaveAt: 0,
  settingsFormSnapshot: '',
  runningCount: 0,
  stoppedCount: 0,
  // Global goals - server-driven via /api/goals + goal_progress SSE.
  // Map id → { id, name, eta }. Populated by loadGlobalGoals on bootstrap +
  // reconnect; patched live by goal_progress SSE events.
  globalGoals: {},
  // Per-task goal state for the per-task log panel tiles. Map id →
  // { enabled, direction, uploadTarget, downloadTarget, secs, uploaded,
  //   downloaded, upBps, downBps, lastUpBps, lastDownBps, status }.
  goals: {},
  logSuccessCount: 0,
  logTotalRows: 0,
  firstLoadDone: false,
  logMaxRows: 1000,
  captureToken: null,
  capturedFingerprint: null,
  captureFormSnapshot: '',
  magnetTimer: null,
  parsedLinks: null,
  lastTaskActionAt: 0,
};

// Suppress SSE-driven toasts for actions the user just triggered herself.
// Both `lastTaskActionAt` (stop/start/delete/edit/submit) and `settingsSaveAt`
// (settings save / add-captured-client / config reload) are routed through
// this single check so the suppression window lives in one place.
const SUPPRESS_WINDOW_MS = 2000;
export function shouldSuppressToast() {
  const lastAction = Math.max(state.lastTaskActionAt || 0, state.settingsSaveAt || 0);
  return Date.now() - lastAction < SUPPRESS_WINDOW_MS;
}
