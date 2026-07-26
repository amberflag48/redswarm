import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Dead state-class detection: CSS state classes that are toggled by JS (e.g.
// `.connected`, `.reconnecting`, `.disconnected`, `.drag`, `.open`, `.loading`)
// must have an actual application site - a `classList.add('STATE')`,
// `classList.toggle('STATE')`, or `className = '... STATE'` in JS. A CSS rule
// for a state class that no JS ever applies is dead CSS - the most reliable
// signal that a feature was removed but the styles were left behind.
//
// The existing dead-css.test.js checks that CSS class selectors appear
// somewhere in HTML/JS/Rust source text, but a class name can appear in a
// comment, a labels map, or a string literal without ever being applied to a
// DOM element. This test goes further: for state classes (classes that are
// dynamically toggled, not present in the static HTML), it verifies an actual
// classList application site exists.

const CSS_PATHS = [
  '../styles/components.css', '../styles/modal.css', '../styles/log.css',
  '../styles/capture.css', '../styles/toast.css', '../styles/layout.css',
];

async function fetchText(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

// Extract compound state-class selectors from CSS: `.conn-badge.disconnected`
// → `disconnected`. We look for `.base.state` patterns (a class combined with
// another class via dot, no space) where the second class looks like a state.
function extractStateClasses(css) {
  const states = new Set();
  // Match `.someClass.stateClass {` or `.someClass.stateClass.otherClass {`
  // The state class is the one that looks like a state word.
  const stateWords = new Set([
    'connected', 'reconnecting', 'disconnected', 'active', 'open', 'closed',
    'loading', 'done', 'expanded', 'collapsed', 'drag', 'error', 'flash',
    'hidden', 'hover', 'focus', 'disabled', 'checked', 'selected',
  ]);
  const re = /\.([a-zA-Z_-]+)\.([a-zA-Z_-]+)\b/g;
  let m;
  while ((m = re.exec(css))) {
    const stateClass = m[2];
    if (stateWords.has(stateClass)) {
      states.add(stateClass);
    }
  }
  return states;
}

suite('dead state CSS', () => {
  test('every CSS state class has a JS application site', async () => {
    // Collect all state classes from CSS.
    const allCSS = (await Promise.all(CSS_PATHS.map(fetchText))).join('\n');
    const stateClasses = extractStateClasses(allCSS);

    // Collect all JS source to search for application sites.
    const jsSources = new Map();
    for (const path of MODULE_PATHS) {
      jsSources.set(path, await fetchText(path));
    }

    const failures = [];
    for (const state of stateClasses) {
      // Look for classList.add('state'), classList.toggle('state'),
      // classList.remove('state'), className = '...state...'
      // or class="...state..." in the template (for initial states).
      let found = false;
      for (const [path, src] of jsSources) {
        // Match classList.add/remove/toggle('state'), className = '...state...',
        // or the state passed as a string argument to a setter function like
        // setConnState('reconnecting') - the function assigns it to className.
        const re = new RegExp(
          `classList\\.(add|remove|toggle)\\(\\s*['"]${state}['"]|className\\s*=\\s*['"][^']*\\b${state}\\b|['"]${state}['"]\\s*\\)`,
          'g'
        );
        if (re.test(src)) { found = true; break; }
      }
      if (!found) {
        // Also check the served HTML for the class (initial state in template).
        const r = await fetch('/', { cache: 'no-store' });
        const html = await r.text();
        const re = new RegExp(`class="[^"]*\\b${state}\\b[^"]*"`, 'g');
        if (re.test(html)) found = true;
      }
      if (!found) {
        failures.push(`.${state} - CSS rule exists but no JS classList application site found`);
      }
    }
    assert(failures.length === 0,
      'CSS state classes with no JS application site (dead CSS):\n  ' +
      failures.join('\n  '));
  });
});
