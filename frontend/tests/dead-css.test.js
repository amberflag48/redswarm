import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Dead-CSS detection: every class selector in the inlined stylesheet must
// appear somewhere in the rendered HTML, the JS-built DOM, or the server-
// rendered fragments. A class in CSS that nothing references is dead -
// usually a refactor removed the last consumer but left the rule behind.
//
// We fetch the served page (which inlines the CSS and the server-rendered
// task list / log panel / settings panes), extract class selectors from the
// <style> block, then search for each class name (word-boundary) across the
// combined haystack of page HTML + all JS module sources + HTML fragments.
// This is deliberately broad: a class referenced only via dynamic string
// concatenation (`'toast ' + type`) still matches because the literal token
// appears in the JS source text.

// Classes emitted only by Rust (render.rs) in data rows / on-demand panels
// that may not be present when the DB is empty (no tasks, no selected log).
// These are NOT dead CSS - they just can't be observed via fetch('/') with an
// empty database. Verified against src/render.rs.
const RUST_RENDERED_DYNAMIC = new Set([
  'mono',                   // <td class="mono"> task rows (render.rs render_task_row)
  'name-cell',              // <td class="name-cell"> task rows (render.rs render_task_row)
  'fail',                   // <span class="badge fail"> log rows (render.rs render_log_row)
  'phase-attack',           // <td class="phase-attack"> log rows (render.rs render_log_row)
  'phase-probe',            // <td class="phase-probe"> log rows (render.rs render_log_row)
  'audit-info-head',        // audit-info panel header (render.rs)
  'audit-info-name',        // audit-info panel name (render.rs)
  'audit-info-client-row',  // audit-info panel client row (render.rs)
  'config-rows',            // config rows in audit-info (render.rs)
  'config-rows-header',     // config rows header (render.rs)
  'info-grid',              // event-log audit-info grid (render.rs)
]);

async function fetchText(path) {
  const r = await fetch(path, { cache: 'no-store' });
  if (!r.ok) return '';
  return r.text();
}

// Extract the inlined <style> content from the served page.
function extractStyle(html) {
  const m = html.match(/<style>([\s\S]*?)<\/style>/);
  return m ? m[1] : '';
}

// Extract class selectors from CSS text. Walks the CSS character-by-character,
// collecting the text before each `{` that isn't an @-rule header (so @media
// and @layer inner selectors are included, @keyframes keyframe selectors are
// harmless - they contain no `.class`). Pseudo-parts are stripped so
// `.badge.running:hover` yields `badge` and `running`.
function extractClassNames(css) {
  // Remove comments first so `/* .foo */` doesn't produce false classes.
  css = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const selectors = [];
  let depth = 0, buf = '';
  for (let i = 0; i < css.length; i++) {
    const ch = css[i];
    if (ch === '{') {
      const sel = buf.trim();
      if (sel && !sel.startsWith('@')) selectors.push(sel);
      depth++;
      buf = '';
    } else if (ch === '}') {
      depth--;
      buf = '';
    } else {
      buf += ch;
    }
  }
  const classes = new Set();
  for (const sel of selectors) {
    for (const part of sel.split(',')) {
      // Strip pseudo-classes/elements including functional ones: :hover,
      // ::before, :not(.x), :nth-child(2n+1).
      const stripped = part.replace(/::?[a-zA-Z-]+(\([^)]*\))?/g, '');
      for (const m of stripped.matchAll(/\.([a-zA-Z][\w-]*)/g)) {
        classes.add(m[1]);
      }
    }
  }
  return classes;
}

// Strip <style> and <script> blocks from HTML so their content doesn't
// pollute the class haystack (CSS selectors aren't "used" classes).
function stripBlocks(html) {
  return html
    .replace(/<style[\s\S]*?<\/style>/gi, '')
    .replace(/<script[\s\S]*?<\/script>/gi, '');
}

suite('dead css', () => {
  test('every CSS class selector is referenced in HTML, JS, or rendered fragments', async () => {
    const pageHtml = await fetchText('/');
    assert(pageHtml, 'served page must be reachable');

    const css = extractStyle(pageHtml);
    assert(css, 'served page must inline a <style> block');
    const cssClasses = extractClassNames(css);

    // Build the haystack: page body (no style/script) + all JS sources +
    // server-rendered HTML fragments (may be empty with an empty DB).
    let haystack = stripBlocks(pageHtml);
    for (const path of MODULE_PATHS) {
      haystack += '\n' + await fetchText(new URL(path, import.meta.url).href);
    }
    // Also fetch the HTML fragment endpoints for extra server-rendered markup.
    haystack += '\n' + await fetchText('/html/audits');
    haystack += '\n' + await fetchText('/html/audits/1/log');

    const dead = [];
    for (const cls of cssClasses) {
      if (RUST_RENDERED_DYNAMIC.has(cls)) continue;
      const re = new RegExp('\\b' + cls.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\b');
      if (!re.test(haystack)) dead.push(cls);
    }

    assert(dead.length === 0,
      'CSS class selectors never referenced in HTML, JS, or rendered fragments (dead CSS):\n  ' +
      dead.join('\n  '));
  });
});
