import { test, suite, assert } from './harness.js';

// Performance: `transition: all` (and the bare `transition: <time>` shorthand,
// which defaults to `transition-property: all`) animates every animatable
// property, risking unexpected paint/layout work and INP regressions. Every
// transition should name the specific properties that change. This test scans
// every CSS source file and fails on any `transition: all` or bare-time
// shorthand.

const CSS_PATHS = [
  '../styles/tokens.css', '../styles/base.css', '../styles/layout.css',
  '../styles/components.css', '../styles/modal.css', '../styles/log.css',
  '../styles/capture.css', '../styles/toast.css', '../styles/animations.css',
];

async function fetchText(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

suite('perf transitions', () => {
  test('no transition: all in any CSS file', async () => {
    const failures = [];
    for (const path of CSS_PATHS) {
      const css = await fetchText(path);
      // Match "transition: all" or "transition: all 150ms" etc.
      if (/transition\s*:\s*all\b/.test(css)) failures.push(path);
    }
    assert(failures.length === 0,
      'transition: all found in CSS files (use specific properties to avoid animating layout/paint):\n  ' +
      failures.join('\n  '));
  });

  test('no bare transition: <time> shorthand (defaults to transition-property: all)', async () => {
    const failures = [];
    for (const path of CSS_PATHS) {
      const css = await fetchText(path);
      // Match "transition: 200ms" or "transition: 150ms" etc. - a bare time
      // value means transition-property defaults to `all`. The property list
      // must be explicit (e.g. "transition: opacity 120ms" or
      // "transition: color 150ms, background-color 150ms").
      if (/transition\s*:\s*\d+(?:ms|s)\b/.test(css)) failures.push(path);
    }
    assert(failures.length === 0,
      'bare transition: <time> found (defaults to transition-property: all - name the properties explicitly):\n  ' +
      failures.join('\n  '));
  });
});
