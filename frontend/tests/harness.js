// harness.js - zero-dependency ES module test harness.
//
// Register tests with `test(name, fn)`; group them with `suite(name, fn)`.
// Assert with `assert` / `assertEq` / `assertThrows`. `withFixture` mounts DOM
// for tests that need it and tears it down in `finally` so a failing assert
// can never leak elements into the next test.
//
// Tests run automatically on `window load` and write a pass/fail report to
// `<pre id="results">`, with a one-line summary in `<div id="status">`. Import
// or fetch errors that fire before run() are surfaced as synthetic failures
// so a broken module path fails loudly instead of silently registering zero
// tests.

const tests = [];
const loadErrors = [];
let currentSuite = '';
let ran = false;

export function suite(name, fn) {
  const prev = currentSuite;
  currentSuite = name;
  try { fn(); } finally { currentSuite = prev; }
}

export function test(name, fn) {
  tests.push({ name: currentSuite ? `${currentSuite} · ${name}` : name, fn });
}

export function assert(cond, msg) {
  if (!cond) throw new Error(msg || 'assertion failed');
}

export function assertEq(actual, expected, msg) {
  if (!deepEqual(actual, expected)) {
    throw new Error(
      `${msg || 'not equal'}\n      expected: ${fmtVal(expected)}\n      actual:   ${fmtVal(actual)}`
    );
  }
}

export function assertThrows(fn, msg) {
  let threw = false;
  try { fn(); } catch { threw = true; }
  if (!threw) throw new Error(msg || 'expected function to throw');
}

// Mount `html` inside a detached <div> appended to <body>, run `fn(root)`,
// and remove the root in `finally` - even when `fn` throws. Lets DOM tests
// share a single, leak-proof fixture helper instead of each hand-rolling
// setup/teardown.
export async function withFixture(html, fn) {
  const root = document.createElement('div');
  root.innerHTML = html;
  document.body.appendChild(root);
  try { return await fn(root); } finally { root.remove(); }
}

function deepEqual(a, b) {
  if (typeof a === 'number' && typeof b === 'number') return Object.is(a, b);
  if (a === b) return true;
  if (a == null || b == null) return false;
  if (typeof a !== typeof b) return false;
  const aArr = Array.isArray(a), bArr = Array.isArray(b);
  if (aArr || bArr) {
    if (!aArr || !bArr || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!deepEqual(a[i], b[i])) return false;
    return true;
  }
  if (typeof a === 'object') {
    const ak = Object.keys(a), bk = Object.keys(b);
    if (ak.length !== bk.length) return false;
    for (const k of ak) {
      if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
      if (!deepEqual(a[k], b[k])) return false;
    }
    return true;
  }
  return false;
}

function fmtVal(v) {
  if (typeof v === 'string') return JSON.stringify(v);
  if (v === undefined) return 'undefined';
  if (v === null) return 'null';
  try { return JSON.stringify(v); } catch { return String(v); }
}

function resultsEl() {
  return document.getElementById('results') || document.body;
}

export async function run() {
  if (ran) return { skipped: true };
  ran = true;
  const out = resultsEl();
  let passed = 0, failed = 0;
  const lines = [];

  for (const e of loadErrors) {
    failed++;
    lines.push(`  ✗ [load] ${e}`);
  }

  for (const t of tests) {
    try {
      await t.fn();
      passed++;
      lines.push(`  ✓ ${t.name}`);
    } catch (e) {
      failed++;
      const msg = e && e.message ? e.message : String(e);
      lines.push(`  ✗ ${t.name}\n      ${msg.replace(/\n/g, '\n      ')}`);
    }
  }

  const total = passed + failed;
  const summary = `${passed} passed, ${failed} failed, ${total} total`;
  lines.push('', summary);
  out.textContent = lines.join('\n');

  document.title = failed ? `(${failed}) failing` : `${total} passed`;
  const status = document.getElementById('status');
  if (status) {
    status.textContent = summary;
    status.className = failed ? 'fail' : 'pass';
  }
  return { passed, failed, total };
}

if (typeof window !== 'undefined') {
  // Registered first (harness.js is the first module script in index.html) so
  // import/fetch errors from later modules are captured before run().
  window.addEventListener('error', e => {
    const where = e.filename ? ` (${(e.filename.split('/').pop() || '').replace(/^\/static\//, '')}:${e.lineno})` : '';
    loadErrors.push((e.message || 'error') + where);
  });
  window.addEventListener('unhandledrejection', e => {
    const r = e.reason;
    loadErrors.push('unhandledrejection: ' + (r && r.message ? r.message : String(r)));
  });
  window.addEventListener('load', run);
}
