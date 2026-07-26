import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Named-export integrity: every named binding imported by a module must
// actually exist in the target module's namespace. Dynamic `import()` alone
// (paths.test.js) does NOT catch a renamed/deleted export - `import { x } from
// './y.js'` resolves even if y.js no longer exports `x` (it only throws on a
// missing default or a bad path). This suite parses each module's `import {…}`
// lines, resolves the target, and asserts each imported name is defined. This
// is the exact check that would have caught the historical bug where init.js
// imported `taskRow` from task-list.js (which never re-exported it - it only
// worked in the bundle via hoisting, and would have broken native ESM).

const NAMED_IMPORT = /import\s*\{([^}]*)\}\s*from\s*['"]([^'"]+)['"]/g;

function resolve(target, fromUrl) {
  return new URL(target, fromUrl).href;
}

async function moduleImports(path) {
  const url = new URL(path, import.meta.url).href;
  const src = await (await fetch(url, { cache: 'no-store' })).text();
  const out = [];
  let m;
  NAMED_IMPORT.lastIndex = 0;
  while ((m = NAMED_IMPORT.exec(src))) {
    const names = m[1].split(',').map(s => s.trim()).filter(Boolean);
    for (const binding of names) {
      // `a as b` → the exported name is `a` (what the target must export).
      const exported = binding.split(/\s+as\s+/)[0].trim();
      if (exported) out.push({ exported, from: m[2], url });
    }
  }
  return out;
}

suite('exports', () => {
  // Build the full import graph across every module, then verify each
  // imported name resolves in its target. One test per offending module-path
  // keeps the failure report precise.
  test('every named import resolves to an actual export', async () => {
    const allImports = [];
    for (const path of MODULE_PATHS) {
      for (const imp of await moduleImports(path)) {
        if (!imp.from.startsWith('.')) continue; // bare/absolute imports - none here
        allImports.push(imp);
      }
    }
    // Cache target namespaces so we don't re-import the same module many times.
    const nsCache = new Map();
    async function nsFor(url) {
      if (!nsCache.has(url)) nsCache.set(url, import(url).then(m => Object.keys(m)));
      return nsCache.get(url);
    }
    const failures = [];
    for (const imp of allImports) {
      const targetUrl = resolve(imp.from, imp.url);
      let names;
      try {
        names = await nsFor(targetUrl);
      } catch (e) {
        failures.push(`${imp.exported} from ${imp.from} (in ${imp.url}) → import failed: ${e.message}`);
        continue;
      }
      if (!names.includes(imp.exported)) {
        failures.push(`${imp.exported} not exported by ${imp.from} (imported in ${imp.url})`);
      }
    }
    assert(failures.length === 0,
      'missing/renamed exports:\n  ' + failures.join('\n  '));
  });
});
