import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Broken-path detection: every `/api/` or `/html/` string literal in the
// frontend must be a known backend route (a static prefix of one of api.rs's
// routes). Catches typos like `/api/setting` and rogue endpoints. Also asserts
// the frontend still references the core endpoints it depends on, so a silent
// removal fails.

async function fetchSource(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

// Static prefixes of every route in api::router (the segments before any
// `{id}`/`{token}` interpolation). A literal like `/api/audits/` (followed by
// `+ id + '/log'`) trims to `/api/audits`, which is a valid prefix.
const STATIC_API_PREFIXES = new Set([
  '/api/events',
  '/api/audits',
  '/api/settings',
  '/api/clients',
  '/api/bootstrap',
  '/api/parse-torrent',
  '/api/parse-magnet',
  '/api/goals',
  '/api/capture/start',
  '/api/capture',
  '/html/audits',
  '/html/goals',
]);

// Endpoints the frontend must still reference (guards against a silent drop).
const MUST_REFERENCE = [
  '/api/events',
  '/api/audits',
  '/api/settings',
  '/api/clients',
  '/api/bootstrap',
  '/api/capture/start',
  '/api/parse-torrent',
  '/api/parse-magnet',
  '/html/audits',
  '/html/goals',
];

const ROUTE_LITERAL = /['"`](\/(?:api|html)\/[a-z0-9/_-]*)/g;

async function allRouteLiterals() {
  const found = new Set();
  for (const path of MODULE_PATHS) {
    const src = await fetchSource(path);
    let m;
    ROUTE_LITERAL.lastIndex = 0;
    while ((m = ROUTE_LITERAL.exec(src))) {
      let p = m[1];
      if (p.endsWith('/')) p = p.slice(0, -1);
      found.add(p);
    }
  }
  return found;
}

suite('api paths', () => {
  test('every /api/ or /html/ literal is a known route prefix', async () => {
    const found = await allRouteLiterals();
    const unknown = [...found].filter(p => !STATIC_API_PREFIXES.has(p));
    assert(unknown.length === 0,
      'unknown route literals (frontend↔backend route drift):\n  ' + unknown.join('\n  '));
  });

  test('frontend still references every core endpoint', async () => {
    const found = await allRouteLiterals();
    const missing = MUST_REFERENCE.filter(p => !found.has(p));
    assert(missing.length === 0,
      'core endpoints no longer referenced by the frontend:\n  ' + missing.join('\n  '));
  });
});
