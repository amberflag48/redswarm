import { test, suite, assert } from './harness.js';

// Asset integrity: the served page must wire the deferred bundle, the bundle
// must be reachable and non-empty, and the CSS must be inlined (no
// render-blocking stylesheet request). Catches a broken build.sh run that
// drops the <script> tag, empties the bundle, or leaves a <link rel=stylesheet>
// after a re-run.

async function fetchText(path) {
  const r = await fetch(path, { cache: 'no-store' });
  if (!r.ok) throw new Error(`${path} → HTTP ${r.status}`);
  return { text: await r.text(), status: r.status };
}

suite('assets', () => {
  test('index page embeds bootstrap + deferred bundle (no render-blocking CSS link)', async () => {
    const { text } = await fetchText('/');
    assert(text.includes('window.__BOOTSTRAP__'),
      'index.html must inline window.__BOOTSTRAP__ (zero API calls for first paint)');
    // The bundle URL is content-hash fingerprinted (e.g. /static/bundle.a1b2c3d4e5f6.js)
    // so the exact hash varies per build. Extract the actual URL and verify it.
    const bundleMatch = text.match(/<script defer src="(\/static\/bundle\.[a-f0-9]+\.js)"><\/script>/);
    assert(bundleMatch,
      'index.html must reference a fingerprinted deferred bundle (/static/bundle.<hash>.js)');
    assert(!text.includes('<link rel="stylesheet" href="/static/bundle.css">'),
      'CSS must be inlined - no render-blocking <link rel=stylesheet>');
    assert(text.includes('<style>'),
      'CSS must be inlined as a <style> block');
  });

  test('bundle.js is reachable, non-empty, and starts with the generated header', async () => {
    // Fetch the index page, extract the fingerprinted bundle URL, fetch that.
    const { text } = await fetchText('/');
    const bundleMatch = text.match(/<script defer src="(\/static\/bundle\.[a-f0-9]+\.js)"><\/script>/);
    assert(bundleMatch, 'index.html must contain a fingerprinted bundle URL');
    const bundleUrl = bundleMatch[1];
    const r = await fetch(bundleUrl);
    assert(r.ok, `${bundleUrl} must be reachable`);
    const bundleText = await r.text();
    assert(bundleText.length > 1000, 'bundle must be non-trivial');
    assert(bundleText.startsWith('// Auto-generated bundle'),
      'bundle must start with the build.sh header comment');
  });
});
