import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Static lint checks over each module's source (fetched as text). These catch
// regressions the project rules forbid: TODO/FIXME markers left in production
// code, forgotten `debugger` statements, and ungated `console.log`/`console.debug`
// left from debugging. `console.log` on a line containing `DEBUG` is allowed
// (intentional dev-only logging gated by a query-param/localStorage flag).
async function fetchSource(path) {
  const res = await fetch(path, { cache: 'no-store' });
  if (!res.ok) throw new Error(`fetch ${path} → HTTP ${res.status}`);
  return res.text();
}

suite('lint', () => {
  test('every module file is reachable via fetch', async () => {
    for (const path of MODULE_PATHS) {
      const res = await fetch(path);
      assert(res.ok, `${path} unreachable (HTTP ${res.status})`);
    }
  });

  test('no TODO/FIXME/XXX/HACK markers in any module', async () => {
    for (const path of MODULE_PATHS) {
      const src = await fetchSource(path);
      assert(!/\b(TODO|FIXME|XXX|HACK)\b/.test(src), `${path} contains a TODO/FIXME/XXX/HACK marker`);
    }
  });

  test('no debugger statements in any module', async () => {
    for (const path of MODULE_PATHS) {
      const src = await fetchSource(path);
      assert(!/\bdebugger\b/.test(src), `${path} contains a debugger statement`);
    }
  });

  test('no ungated console.log/console.debug in any module', async () => {
    for (const path of MODULE_PATHS) {
      const src = await fetchSource(path);
      for (const line of src.split('\n')) {
        if (/\bconsole\.(log|debug)\s*\(/.test(line) && !/\bDEBUG\b/.test(line)) {
          assert(false, `${path} has ungated console.log/debug: ${line.trim()}`);
        }
      }
    }
  });
});
