// UI labels rendered client-side. Mirrors `src/data/labels.rs` for the strings
// the JS builds dynamically (task-list rows, settings option lists, empty
// states). Server-rendered strings still come from labels.rs via Askama; this
// module is the single source for the client-only ones so a rename touches one
// place instead of scattered JS literals.

export const EMPTY_DASH = '-';
export const EMPTY_LOG = 'Select a task to view its log.';

// Mode / Speed-mode option lists for the settings modal + new-task form.
export const MODE_OPTIONS = [['download_and_upload', 'Download + Upload'], ['upload_only', 'Upload only']];
export const STRATEGY_OPTIONS = [['fixed', 'Fixed'], ['dynamic', 'Dynamic']];
// Goal-direction option list (mirrors src/data/vocab.rs wire names +
// src/data/labels.rs display values) for the settings + new-task forms.
export const GOAL_DIRECTION_OPTIONS = [['upload', 'Upload'], ['download_and_upload', 'Download + Upload']];
// Goal-reached-action option list (mirrors src/data/vocab.rs wire names +
// src/data/labels.rs display values).
export const GOAL_REACHED_ACTION_OPTIONS = [
  ['stop', 'Stop'],
  ['continue_initial', 'Continue (initial speed)'],
  ['continue_custom', 'Continue (custom speed)'],
];

// New-task / edit-task modal titles + the primary action button label. The
// template ships matching static text (pre-hydration); the JS re-sets these on
// open/close so the strings must come from one place.
export const NEW_TASK = 'New task';
export const EDIT_TASK = 'Edit task';
export const START_TASK = 'Start task';
export const SAVE_CHANGES = 'Save changes';

export const STATUS_RUNNING = 'running';
export const STATUS_STOPPED = 'stopped';
