import { test, suite, assert, assertEq } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// modules.js ↔ frontend/js/ sync. MODULE_PATHS is the shared list backing
// paths.test.js, lint.test.js, exports.test.js, dry.test.js, and api-paths.test.js.
// Drift here means a new module is silently untested or a removed module leaves
// a stale entry. This suite pins the count, forbids duplicates, and asserts
// every relative import target of every module is itself registered (or is
// main.js, the bootstrap entry excluded by design).

const NAMED_IMPORT = /import\s*\{[^}]*\}\s*from\s*['"]([^'"]+)['"]/g;
const DEFAULT_IMPORT = /import\s+(\w+)\s+from\s*['"]([^'"]+)['"]/g;

async function fetchSource(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

suite('modules sync', () => {
  test('MODULE_PATHS has no duplicates', () => {
    const seen = new Set();
    for (const p of MODULE_PATHS) {
      assert(!seen.has(p), `duplicate MODULE_PATHS entry: ${p}`);
      seen.add(p);
    }
  });

  test('MODULE_PATHS count is pinned (add a module → update this)', () => {
    assertEq(MODULE_PATHS.length, 25,
      'MODULE_PATHS changed - update the pinned count in modules-sync.test.js');
  });

  test('every relative import target is registered in MODULE_PATHS (or is main.js)', async () => {
    const registered = new Set(MODULE_PATHS.map(p => new URL(p, import.meta.url).href));
    const failures = [];
    for (const path of MODULE_PATHS) {
      const src = await fetchSource(path);
      for (const re of [NAMED_IMPORT, DEFAULT_IMPORT]) {
        re.lastIndex = 0;
        let m;
        while ((m = re.exec(src))) {
          const target = m[1];
          if (!target.startsWith('.')) continue; // bare imports - none here
          const targetUrl = new URL(target, new URL(path, import.meta.url)).href;
          // main.js is the dev entry, intentionally excluded from MODULE_PATHS.
          if (/\/js\/main\.js$/.test(targetUrl)) continue;
          if (!registered.has(targetUrl)) {
            failures.push(`${path} imports ${target} which is not in MODULE_PATHS`);
          }
        }
      }
    }
    assert(failures.length === 0,
      'unregistered import targets:\n  ' + failures.join('\n  '));
  });
});
