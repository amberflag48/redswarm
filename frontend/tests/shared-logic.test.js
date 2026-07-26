import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Shared-logic enforcement: patterns that should be centralized in a helper
// must not be reimplemented across modules. Each rule catches a specific DRY
// violation the project rules forbid. Rule A (bare fetch) and Rule E (inline
// style= in JS HTML) are already covered by dry.test.js and are not
// duplicated here.

async function fetchText(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

// Map of module path → source text, fetched once.
async function moduleSources() {
  const out = new Map();
  for (const path of MODULE_PATHS) out.set(path, await fetchText(path));
  return out;
}

// Count regex matches in a string (global regex; resets lastIndex).
function countMatches(src, re) {
  let n = 0;
  re.lastIndex = 0;
  let m;
  while ((m = re.exec(src))) n++;
  return n;
}

const ABORT = /new\s+AbortController\b/g;
const PENDING = /\b(?:withPending|pendingSignal|clearPending)\b/g;
const DATE_NOW_STATE = /Date\.now\(\)\s*-\s*state\./g;

// Close functions that must guard against unsaved changes before closing.
// Each should reference confirmDiscardIfDirty (the discard guard) or
// registerModal (the modal framework that centralizes close handling).
const CLOSE_FUNCS = ['closeModal', 'closeSettingsModal', 'closeCaptureModal'];

suite('shared logic', () => {
  test('every AbortController is paired with a pending signal helper', async () => {
    // If a module creates an AbortController it must also reference the shared
    // pending-signal helpers - a bare controller without the shared bookkeeping
    // is a hand-rolled copy of the request-cancellation pattern.
    const failures = [];
    for (const [path, src] of await moduleSources()) {
      const controllers = countMatches(src, ABORT);
      const helpers = countMatches(src, PENDING);
      if (controllers > helpers) {
        failures.push(`${path}: ${controllers} new AbortController but only ${helpers} pending helper refs`);
      }
    }
    assert(failures.length === 0,
      'AbortController without withPending/pendingSignal/clearPending:\n  ' + failures.join('\n  '));
  });

  test('Date.now() - state.* timestamp checks are centralized (≤ 1 occurrence total)', async () => {
    // The "suppress toast if recent action" pattern (Date.now() - state.last* >
    // NNNN) must live in a single shouldSuppressToast() helper. Scattering it
    // means the threshold can drift between call sites.
    const sources = await moduleSources();
    let total = 0;
    const sites = [];
    for (const [path, src] of sources) {
      const n = countMatches(src, DATE_NOW_STATE);
      if (n > 0) { total += n; sites.push(`${path}: ${n}`); }
    }
    assert(total <= 1,
      `Date.now() - state.* appears ${total} times across modules (should be ≤ 1, centralized in shouldSuppressToast):\n  ` +
      sites.join('\n  '));
  });

  test('every modal close function guards via confirmDiscardIfDirty or registerModal', async () => {
    // A close function that doesn't confirm-discard or go through the modal
    // framework can silently drop unsaved edits. For each known close function
    // name, find the module that defines it and assert the same module
    // references the guard. Missing function names are skipped (the function
    // may have been refactored/renamed).
    const sources = await moduleSources();
    const failures = [];
    for (const fn of CLOSE_FUNCS) {
      let definingModule = null;
      for (const [path, src] of sources) {
        if (new RegExp(`export\\s+(?:async\\s+)?(?:function|const)\\s+${fn}\\b`).test(src)) {
          definingModule = [path, src];
          break;
        }
      }
      if (!definingModule) continue; // refactored away - skip
      const [, src] = definingModule;
      if (!/\b(?:confirmDiscardIfDirty|registerModal)\b/.test(src)) {
        failures.push(`${fn} (in ${definingModule[0]}) does not reference confirmDiscardIfDirty or registerModal`);
      }
    }
    assert(failures.length === 0,
      'modal close functions missing discard guard:\n  ' + failures.join('\n  '));
  });
});
