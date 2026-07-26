import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// DOM hook drift: every `data-*` attribute the backend emits must be consumed
// by the JS, and every `data-*` the JS reads should be emitted by the backend.
// A mismatch in either direction silently breaks the wire between Rust and
// JS - a renamed hook means the click handler or update never fires.
//
// We fetch the full rendered page (which includes the static template + the
// server-rendered task list, log panel, and settings panes) plus the HTML
// fragment endpoints, extract every `data-*` attribute, and cross-reference
// against the JS module sources. The first direction (emitted but not
// consumed) is a hard failure - a dead hook. The reverse (consumed but not
// emitted) is reported but not failed, because the DB may simply be empty
// (no task rows to carry data-id/data-action/etc.).

// data-* attributes consumed by CSS (attr()) or used as static template
// markers rather than read by JS. These are NOT dead hooks.
const ALLOWLIST = new Set([
  'data-label',       // consumed by CSS `content: attr(data-label)` (mobile table labels)
]);

async function fetchText(path) {
  const r = await fetch(path, { cache: 'no-store' });
  if (!r.ok) return '';
  return r.text();
}

// Extract all data-foo attribute names from HTML text.
function extractDataAttrs(html) {
  const attrs = new Set();
  for (const m of html.matchAll(/\bdata-([a-z][a-z0-9-]*)/gi)) attrs.add(m[0]);
  return attrs;
}

// Convert a data-attr name to its dataset property: data-show-downloaded → showDownloaded.
function toDatasetProp(attr) {
  // attr is like "data-show-downloaded" → "showDownloaded"
  return attr.replace(/^data-/, '').replace(/-(.)/g, (_, c) => c.toUpperCase());
}

suite('dom hooks', () => {
  test('every data-* emitted in rendered HTML is consumed by JS (no dead hooks)', async () => {
    // Gather all rendered HTML: the full page + HTML fragments.
    const fragments = [
      await fetchText('/'),
      await fetchText('/html/audits'),
      await fetchText('/html/audits/1/log'),
      await fetchText('/html/goals'),
    ].join('\n');
    assert(fragments.trim(), 'served page must be reachable');

    const emitted = extractDataAttrs(fragments);

    // Gather all JS source text.
    let js = '';
    for (const path of MODULE_PATHS) {
      js += '\n' + await fetchText(new URL(path, import.meta.url).href);
    }

    const dead = [];
    const jsConsumed = new Set();
    const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    for (const attr of emitted) {
      if (ALLOWLIST.has(attr)) continue;
      const prop = toDatasetProp(attr);
      const literal = new RegExp(esc(attr));
      const datasetRe = new RegExp('\\bdataset\\.' + esc(prop) + '\\b');
      // The dataset is commonly aliased to a short local (e.g.
      // `const ds = el.dataset; … ds.showDownloaded`), so `dataset.<prop>`
      // alone misses the read. `<prop>` is the camelCase dataset property
      // name, so `\.showDownloaded\b` matches the property access on any
      // holder (the alias, `el.dataset`, or anything else) without a
      // `data-show-downloaded` literal being present.
      const propRe = new RegExp('\\.' + esc(prop) + '\\b');
      if (literal.test(js) || datasetRe.test(js) || propRe.test(js)) {
        jsConsumed.add(attr);
      } else {
        dead.push(attr);
      }
    }

    assert(dead.length === 0,
      'data-* attributes emitted by Rust/template but not consumed by JS (dead hooks):\n  ' +
      dead.join('\n  '));
  });

  test('data-* consumed by JS but not emitted anywhere is a broken hook', async () => {
    // Reverse direction: a JS-referenced data-* that is NEVER emitted - not
    // by the rendered HTML, not by any JS-built HTML string - is a genuine
    // broken hook (the JS reads something nobody writes). We assert on these.
    //
    // data-row-only attributes (data-id, data-seq, data-col, data-stat,
    // data-show-*) are emitted by Rust only when the DB has rows; with an
    // empty DB they don't appear in rendered HTML. These are allowlisted
    // because they ARE emitted by Rust (render.rs) - just not visible when
    // the DB is empty.
    const fragments = [
      await fetchText('/'),
      await fetchText('/html/audits'),
      await fetchText('/html/audits/1/log'),
      await fetchText('/html/goals'),
    ].join('\n');
    const rendered = extractDataAttrs(fragments);

    let js = '';
    for (const path of MODULE_PATHS) {
      js += '\n' + await fetchText(new URL(path, import.meta.url).href);
    }

    // Collect every data-* the JS references (consumed). Filter out partial
    // matches ending with a hyphen (e.g. "data-show-" from a comment like
    // "data-show-* attrs" - the glob * stops the regex, leaving a truncated
    // name that is not a real attribute).
    const jsConsumed = new Set();
    for (const m of js.matchAll(/\bdata-([a-z][a-z0-9-]*)/gi)) {
      if (!m[0].endsWith('-')) jsConsumed.add(m[0]);
    }
    for (const m of js.matchAll(/\bdataset\.([a-zA-Z][a-zA-Z0-9]*)/g)) {
      const name = m[1].replace(/([A-Z])/g, '-$1').toLowerCase();
      jsConsumed.add('data-' + name);
    }

    // Collect every data-* the JS EMITS (in HTML-building strings -
    // data-foo="..." or data-foo='...' or `data-foo=${...}`).
    const jsEmitted = new Set();
    for (const m of js.matchAll(/\bdata-([a-z][a-z0-9-]*)\s*=/gi)) jsEmitted.add('data-' + m[1]);

    // data-row-only attributes: emitted by Rust (render.rs) but only visible
    // when the DB has rows. Not emitted by JS. Allowlisted because they ARE
    // emitted server-side - just absent with an empty DB.
    const ROW_ONLY = new Set([
      'data-id',       // <tr data-id="..."> in task rows
      'data-seq',      // <tr data-seq="..."> in log rows
      'data-col',      // <td data-col="..."> in task/log cells
      'data-stat',     // <div data-stat="..."> in log stats
      'data-show-downloaded', 'data-show-left', 'data-show-download-speed',
      // Goal config rides on each task row (render_task_row); only present
      // when the DB has rows. Read by goalFromRow → state.goals (topbar).
      'data-goal-enabled', 'data-goal-direction', 'data-goal-upload-target', 'data-goal-download-target', 'data-goal-secs',
      // Tab content panels (rendered with data-tab-content in index.html).
      'data-tab-content',
      // Goal tiles in the topbar (render_topbar_stats); only present when
      // goals exist. Read by renderGlobalGoalTiles/patchGoalTile.
      'data-goal-id',
      // Comment-only match ("data-driven" in a code comment, not a real attr).
      'data-driven',
    ]);

    const broken = [];
    for (const attr of jsConsumed) {
      if (ALLOWLIST.has(attr) || ROW_ONLY.has(attr)) continue;
      if (rendered.has(attr) || jsEmitted.has(attr)) continue;
      broken.push(attr);
    }
    assert(broken.length === 0,
      'data-* consumed by JS but never emitted by rendered HTML or JS-built HTML (broken hooks):\n  ' +
      broken.join('\n  '));
  });
});
