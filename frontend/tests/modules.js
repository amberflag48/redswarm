// Single source of truth for the list of frontend library modules under
// frontend/js/. Shared by paths.test.js (dynamic import) and lint.test.js
// (fetch source) so the two never drift.
//
// The order MUST match build.sh's MODULES array exactly - build.sh determines
// the bundle concatenation order (deps before dependents, init.js last), and
// this array must mirror it so a module added to one list but not the other is
// caught by modules-sync.test.js.
//
// `main.js` is intentionally excluded: it is the bootstrap entry that auto-
// runs `init()` on import when the document is already loaded, which requires
// the full app DOM. Its only import (`./app/init.js`) is covered by including
// init.js here, so excluding main.js loses no path coverage.
export const MODULE_PATHS = [
  '../js/state/store.js',
  '../js/state/dom.js',
  '../js/utils/format.js',
  '../js/utils/form.js',
  '../js/utils/dom-helpers.js',
  '../js/utils/buttons.js',
  '../js/utils/net.js',
  '../js/components/toast.js',
  '../js/components/confirm.js',
  '../js/components/modal.js',
  '../js/components/goal-form.js',
  '../js/data/client-schema.js',
  '../js/data/capture-helpers.js',
  '../js/data/labels.js',
  '../js/services/clients.js',
  '../js/services/runtime.js',
  '../js/components/client-card.js',
  '../js/components/task-list.js',
  '../js/components/log-panel.js',
  '../js/components/task-modal.js',
  '../js/components/settings-modal.js',
  '../js/components/goals-modal.js',
  '../js/components/capture-modal.js',
  '../js/services/sse.js',
  '../js/app/init.js',
];
