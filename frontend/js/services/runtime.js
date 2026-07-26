// Runtime settings service - applies /api/settings-derived tunables
// (logMaxRows, currentMode, currentSpeedMode) to the shared state. Called on
// bootstrap and on config_reloaded.

import { state } from '../state/store.js';

export function applyRuntimeSettings(cfg) {
  if (!cfg) return;
  state.logMaxRows = cfg.ui?.event_log_limit || 1000;
  state.currentMode = cfg.defaults?.mode || 'download_and_upload';
  state.currentSpeedMode = cfg.defaults?.speed_mode || 'dynamic';
}
