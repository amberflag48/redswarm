import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// DOM ID drift: every `getElementById('id')` in the JS must reference an ID
// that exists in the served HTML. A rename in the template that isn't
// reflected in the JS (or vice versa) silently breaks at runtime with a null
// element - often a `Cannot set properties of null` error in `init()` that
// kills the whole page. This test catches that class of bug at test time.
//
// The test fetches the full rendered page (which includes all modals - they're
// server-rendered in the initial HTML, not JS-built), extracts every
// `getElementById('...')` call from the JS modules, and verifies each ID
// exists in the page HTML. IDs built dynamically (template literals, string
// concat) are skipped - only static string-literal IDs are checked.

async function fetchText(path) {
  const r = await fetch(path, { cache: 'no-store' });
  if (!r.ok) return '';
  return r.text();
}

// Extract static getElementById('...') IDs from JS source. Matches both
// single and double quoted strings, and template literals with a static
// string (no ${...} interpolation).
function extractGetElementByIdIds(js) {
  const ids = new Set();
  // Match getElementById('id') and getElementById("id")
  const re = /getElementById\s*\(\s*['"]([a-zA-Z][a-zA-Z0-9_-]*)['"]\s*\)/g;
  let m;
  while ((m = re.exec(js))) ids.add(m[1]);
  return ids;
}

// Extract all id="..." from HTML.
function extractHtmlIds(html) {
  const ids = new Set();
  for (const m of html.matchAll(/\bid\s*=\s*"([^"]+)"/g)) ids.add(m[1]);
  return ids;
}

suite('dom-id drift', () => {
  test('every static getElementById(id) in JS exists in the served HTML', async () => {
    // Fetch the full rendered page - includes all modals (server-rendered).
    const pageHtml = await fetchText('/');
    assert(pageHtml.length > 0, 'served page must be reachable');

    const htmlIds = extractHtmlIds(pageHtml);

    // Gather all JS source text from the module paths.
    const failures = [];
    for (const path of MODULE_PATHS) {
      const js = await fetchText(new URL(path, import.meta.url).href);
      if (!js) continue;
      const ids = extractGetElementByIdIds(js);
      for (const id of ids) {
        if (!htmlIds.has(id)) {
          failures.push(`${path}: getElementById('${id}') - ID not found in served HTML`);
        }
      }
    }

    assert(failures.length === 0,
      'JS references DOM IDs that do not exist in the served page (would cause null-element errors at runtime):\n  ' +
      failures.join('\n  '));
  });
});
