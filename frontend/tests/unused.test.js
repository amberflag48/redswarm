import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Unused-import detection (the pragmatic, zero-AST subset of "unused
// variables"). For each module, parse its `import { a, b as c } from '…'`
// lines and assert every imported local name (after `as`) appears at least
// once more in the file body - i.e. it is actually used. A dead import is the
// most reliable signal the codebase has drifted (e.g. a refactor left an
// import behind). This would have caught the dead `hasValue` in task-list.js,
// the dead `clamp` in dom-helpers.js, and the dead `taskRow`/`afterListUpdate`/
// `loadRuntimeSettings` in init.js - all since removed.

const NAMED_IMPORT = /^\s*import\s*\{([^}]*)\}\s*from\s*['"][^'"]+['"]/;

async function fetchSource(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

suite('unused imports', () => {
  test('every imported binding is referenced in its module body', async () => {
    const failures = [];
    for (const path of MODULE_PATHS) {
      const src = await fetchSource(path);
      const lines = src.split('\n');
      for (const line of lines) {
        const m = line.match(NAMED_IMPORT);
        if (!m) continue;
        const bindings = m[1].split(',').map(s => s.trim()).filter(Boolean);
        for (const binding of bindings) {
          // Local name is the alias if present (`a as b` → `b`).
          const local = binding.split(/\s+as\s+/).pop().trim();
          // Whole-word count across the whole file, excluding the import line.
          const re = new RegExp('\\b' + local.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\b', 'g');
          const body = src.replace(line, '');
          const count = (body.match(re) || []).length;
          if (count === 0) {
            failures.push(`${path} imports "${local}" but never uses it`);
          }
        }
      }
    }
    assert(failures.length === 0,
      'unused imports:\n  ' + failures.join('\n  '));
  });
});
