import { test, suite, assert, assertEq } from './harness.js';

// Cross-language label sync: frontend/js/data/labels.js mirrors
// src/data/labels.rs. If a label changes in Rust without updating JS (or vice
// versa), the UI shows inconsistent text between server-rendered fragments and
// client-built DOM. This suite imports the JS module and asserts each value
// matches the Rust label (the canonical value). It also verifies the JS
// values appear in the rendered page so a stale template is caught too.
// The expected values below are the Rust labels (src/data/labels.rs):
// EMPTY_DASH = "-"
// MODE_DU_FULL = "Download + Upload"
// MODE_UO_FULL = "Upload only"
// STRATEGY_FIXED = "Fixed"
// STRATEGY_DYNAMIC= "Dynamic"
// And the modal/button strings the JS owns:
// NEW_TASK = "New task", EDIT_TASK = "Edit task",
// START_TASK = "Start task", SAVE_CHANGES = "Save changes"

// Extract the human-readable label from a [value, label] option pair.
function optionLabels(options) {
 return options.map(o => (Array.isArray(o) ? o[1] : o));
}

suite('labels sync', () => {
 test('JS label values match Rust label constants', async () => {
 const labels = await import('../js/data/labels.js');

 assertEq(labels.EMPTY_DASH, '-',
 'EMPTY_DASH must equal "-" (mirrors labels::EMPTY_DASH)');

 const modeLabels = optionLabels(labels.MODE_OPTIONS);
 assert(modeLabels.includes('Download + Upload'),
 'MODE_OPTIONS must contain "Download + Upload" (mirrors labels::MODE_DU_FULL) - got: ' + JSON.stringify(modeLabels));
 assert(modeLabels.includes('Upload only'),
 'MODE_OPTIONS must contain "Upload only" (mirrors labels::MODE_UO_FULL) - got: ' + JSON.stringify(modeLabels));

 const strategyLabels = optionLabels(labels.STRATEGY_OPTIONS);
 assert(strategyLabels.includes('Dynamic'),
 'STRATEGY_OPTIONS must contain "Dynamic" (mirrors labels::STRATEGY_DYNAMIC) - got: ' + JSON.stringify(strategyLabels));
 assert(strategyLabels.includes('Fixed'),
 'STRATEGY_OPTIONS must contain "Fixed" (mirrors labels::STRATEGY_FIXED) - got: ' + JSON.stringify(strategyLabels));

 assertEq(labels.NEW_TASK, 'New task', 'NEW_TASK must equal "New task"');
 assertEq(labels.EDIT_TASK, 'Edit task', 'EDIT_TASK must equal "Edit task"');
 assertEq(labels.START_TASK, 'Start task', 'START_TASK must equal "Start task"');
 assertEq(labels.SAVE_CHANGES, 'Save changes', 'SAVE_CHANGES must equal "Save changes"');

 // Goal labels mirror src/data/labels.rs (GOAL_DIRECTION_*).
 const goalDirLabels = optionLabels(labels.GOAL_DIRECTION_OPTIONS);
 assertEq(goalDirLabels[0], 'Upload',
 'first GOAL_DIRECTION option must be "Upload" (mirrors labels::GOAL_DIRECTION_UPLOAD)');
 assertEq(goalDirLabels[1], 'Download + Upload',
 'second GOAL_DIRECTION option must be "Download + Upload" (mirrors labels::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD)');
 // Wire names (option values) mirror src/data/vocab.rs.
 assertEq(labels.GOAL_DIRECTION_OPTIONS[0][0], 'upload',
 'GOAL_DIRECTION upload wire name must be "upload" (mirrors vocab::GOAL_DIRECTION_UPLOAD_WIRE)');
 assertEq(labels.GOAL_DIRECTION_OPTIONS[1][0], 'download_and_upload',
 'GOAL_DIRECTION download+upload wire name must be "download_and_upload" (mirrors vocab::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE)');
 assertEq(labels.GOAL_DIRECTION_OPTIONS[1][1], 'Download + Upload',
 'GOAL_DIRECTION download+upload label must be "Download + Upload"');

 // GoalReachedAction options mirror src/data/labels.rs + src/data/vocab.rs.
 assertEq(labels.GOAL_REACHED_ACTION_OPTIONS.length, 3, 'three goal-reached actions');
 assertEq(labels.GOAL_REACHED_ACTION_OPTIONS[0][0], 'stop',
 'first action wire name must be "stop" (mirrors vocab::GOAL_REACHED_STOP_WIRE)');
 assertEq(labels.GOAL_REACHED_ACTION_OPTIONS[0][1], 'Stop',
 'first action label must be "Stop" (mirrors labels::GOAL_REACHED_STOP)');
 assertEq(labels.GOAL_REACHED_ACTION_OPTIONS[1][0], 'continue_initial',
 'second action wire name must be "continue_initial"');
 assertEq(labels.GOAL_REACHED_ACTION_OPTIONS[1][1], 'Continue (initial speed)',
 'second action label must be "Continue (initial speed)"');
 assertEq(labels.GOAL_REACHED_ACTION_OPTIONS[2][0], 'continue_custom',
 'third action wire name must be "continue_custom"');
 assertEq(labels.GOAL_REACHED_ACTION_OPTIONS[2][1], 'Continue (custom speed)',
 'third action label must be "Continue (custom speed)"');
 });

 test('JS label values appear in the rendered page (template ↔ JS sync)', async () => {
 const r = await fetch('/', { cache: 'no-store' });
 assert(r.ok, 'served page must be reachable');
 const html = await r.text();
 const labels = await import('../js/data/labels.js');

 // These strings are always present in the static template (modal titles,
 // button labels, segmented-control option text).
 assert(html.includes(labels.NEW_TASK),
 `rendered page must contain NEW_TASK ("${labels.NEW_TASK}")`);
 assert(html.includes(labels.START_TASK),
 `rendered page must contain START_TASK ("${labels.START_TASK}")`);
 const strategyLabels = optionLabels(labels.STRATEGY_OPTIONS);
 for (const label of strategyLabels) {
 assert(html.includes(label),
 `rendered page must contain STRATEGY_OPTIONS label "${label}"`);
 }
 const modeLabels = optionLabels(labels.MODE_OPTIONS);
 for (const label of modeLabels) {
 assert(html.includes(label),
 `rendered page must contain MODE_OPTIONS label "${label}"`);
 }
 });

 // Regression: "Download + Upload" vs "Download + upload" casing
 // Rust labels.rs defines MODE_DU_FULL = "Download + Upload" (capital U on
 // the second word, since both halves are coordinate operation names) and
 // MODE_UO_FULL = "Upload only" (sentence case - "only" is not a proper
 // noun). The JS labels.js and the template both must match exactly. This
 // test pins the exact casing and would have caught a drift bug.
 test('MODE options use the exact Rust casing (Download + Upload, Upload only)', async () => {
 const labels = await import('../js/data/labels.js');
 const modeLabels = optionLabels(labels.MODE_OPTIONS);
 assertEq(modeLabels[0], 'Download + Upload',
 'first MODE option must be "Download + Upload" (capital U, mirrors labels::MODE_DU_FULL)');
 assertEq(modeLabels[1], 'Upload only',
 'second MODE option must be "Upload only" (sentence case, mirrors labels::MODE_UO_FULL)');
 // Explicitly reject the Title Case variant that drifts from sentence case.
 assert(!modeLabels.includes('Upload Only'),
 'Title Case "Upload Only" is a casing bug - must be "Upload only" (sentence case)');
 });
});
