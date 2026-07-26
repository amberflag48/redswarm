import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// DRY / "shared logic used everywhere" scanner. For each library module,
// fetch its source and assert no forbidden inline reimplementation of a
// shared helper appears outside the module that owns the helper. This is the
// regression net for the project's "reuse before creating" rule: a new
// hand-rolled `Math.min/Math.max` clamp, a manual `&amp;`-replace escape, a
// bare `fetch()` in a component, or an inline `style="…"` in JS-built HTML
// fails loudly, naming the offending file.
//
// Each rule has an allowlist of module paths where the pattern is the
// definition itself (e.g. `clamp` lives in form.js, so `Math.min(Math.max(…))`
// is allowed there and only there).

const NAMED_IMPORT = /import\s*\{([^}]*)\}\s*from\s*['"][^'"]+['"]/;

async function fetchSource(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

// Strip the import lines so an `import { clamp } from …` line doesn't itself
// trigger the "Math.min/Math.max" rule via the word `clamp` etc.
function stripImports(src) {
  return src.split('\n').filter(l => !NAMED_IMPORT.test(l)).join('\n');
}

// Does `path` match any glob in `allow`? Supports a leading `!` to require the
// pattern is NOT present (not used here yet) and plain substring matching.
function allowed(path, allow) {
  return allow.some(a => path.includes(a));
}

const RULES = [
  {
    name: 'no bare fetch() outside net.js - use fetchJson/request/postAction/deleteAction/fetchRaw',
    re: /\bfetch\s*\(/,
    allow: ['utils/net.js'],
  },
  {
    name: 'no hand-rolled Math.min(Math.max(…)) clamp - use clamp() from form.js',
    re: /Math\.(?:min|max)\s*\(\s*Math\.(?:min|max)\s*\(/,
    allow: ['utils/form.js'],
  },
  {
    name: 'no manual &amp;/&quot;/&lt; .replace escaping - use escAttr/escHtml from dom-helpers.js',
    re: /\.replace\(\s*\/[&<>"]\s*\/g/,
    allow: ['utils/dom-helpers.js'],
  },
  {
    name: 'no === null/=== undefined value-presence checks - use hasValue() from form.js',
    re: /===\s*null\s*\|\|[^=]*===\s*undefined/,
    allow: ['utils/form.js'],
  },
  {
    name: "no literal 'set-client-' + string concat - use clientFieldId() from client-schema.js",
    re: /['"`]set-client-['"`]\s*\+/,
    allow: ['data/client-schema.js'],
  },
  {
    name: 'no inline fast-extension bit check - use fastExtensionBit() from capture-helpers.js',
    re: /parseInt\([^)]*\.slice\(\s*14\s*,\s*16\s*\)\s*,\s*16\s*\)\s*&\s*0x04/,
    allow: ['data/capture-helpers.js'],
  },
  {
    name: 'no inline style="…" in JS-built HTML - use a CSS class',
    re: /style\s*=\s*"/,
    allow: [],
  },
  {
    name: 'no forced-reflow flash restart (void el.offsetWidth) - use double-rAF in flashCell',
    re: /void\s+\w+\.offsetWidth/,
    allow: ['components/task-list.js'],
  },
];

suite('dry', () => {
  for (const rule of RULES) {
    test(rule.name, async () => {
      const failures = [];
      for (const path of MODULE_PATHS) {
        if (allowed(path, rule.allow)) continue;
        const src = stripImports(await fetchSource(path));
        if (rule.re.test(src)) failures.push(path);
      }
      assert(failures.length === 0,
        'forbidden pattern found in:\n  ' + failures.join('\n  '));
    });
  }
});
