import { test, suite, assert, assertEq } from './harness.js';

// tests/index.html ↔ tests/*.test.js sync. The test harness discovers tests
// via explicit `<script type="module">` tags in index.html - a new .test.js
// not listed there silently never runs. This suite pins the expected test-file
// list and asserts index.html lists exactly that set (no orphans, no missing).

// The canonical list of test files (relative to tests/). Add a test file →
// append it here AND to tests/index.html, or this suite fails.
const EXPECTED_TEST_FILES = [
  'format.test.js',
  'form.test.js',
  'client-schema.test.js',
  'capture-helpers.test.js',
  'capture-helpers-extra.test.js',
  'dom-helpers.test.js',
  'task-list-helpers.test.js',
  'paths.test.js',
  'lint.test.js',
  'exports.test.js',
  'unused.test.js',
  'dry.test.js',
  'api-paths.test.js',
  'assets.test.js',
  'modules-sync.test.js',
  'dead-exports.test.js',
  'dead-css.test.js',
  'inline-styles.test.js',
  'token-drift.test.js',
  'shared-logic.test.js',
  'dom-hooks.test.js',
  'labels-sync.test.js',
  'modal-cascade.test.js',
  'perf-transitions.test.js',
  'shared-helpers.test.js',
  'dead-state-css.test.js',
  'index-sync.test.js',
  'settings-dirty.test.js',
  'dom-id-drift.test.js',
];

suite('index sync', () => {
  test('tests/index.html lists every expected .test.js (no orphans, no missing)', async () => {
    const r = await fetch('/static/tests/index.html', { cache: 'no-store' });
    assert(r.ok, 'tests/index.html must be reachable');
    const html = await r.text();
    const listed = new Set();
    for (const m of html.matchAll(/<script[^>]*\bsrc="([^"]+\.test\.js)"/g)) {
      listed.add(m[1].replace(/^\.?\//, ''));
    }
    const missing = EXPECTED_TEST_FILES.filter(f => !listed.has(f) && !listed.has('./' + f));
    const orphan = [...listed].filter(f => !EXPECTED_TEST_FILES.includes(f));
    assert(missing.length === 0,
      'test files not listed in tests/index.html (would silently never run):\n  ' + missing.join('\n  '));
    assert(orphan.length === 0,
      'orphan test files in index.html not in EXPECTED_TEST_FILES:\n  ' + orphan.join('\n  '));
  });

  test('EXPECTED_TEST_FILES count is pinned (add a test → update both sides)', () => {
    assertEq(EXPECTED_TEST_FILES.length, 29,
      'EXPECTED_TEST_FILES changed - update the pinned count in index-sync.test.js');
  });
});
