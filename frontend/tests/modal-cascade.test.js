import { test, suite, assert } from './harness.js';

// Modal cascade regression: the compositor-friendly modal show/hide depends on
// `.modal-overlay.modal-closed` keeping `display: flex` (set on `.modal-overlay`)
// and toggling only `visibility`/`opacity`/`pointer-events`. The global
// `.hidden { display: none !important }` in the tokens layer MUST NOT be used
// on modal overlays - `!important` in an earlier layer beats a normal
// `display: flex` in a later layer, silently reverting to the slow
// `display: none → display: flex` toggle (full layout of the subtree on every
// open, two vsyncs instead of one). This test injects the served page's CSS
// into the test document and checks the actual computed cascade.

async function fetchInlinedCSS() {
  const r = await fetch('/', { cache: 'no-store' });
  assert(r.ok, 'page must be reachable');
  const html = await r.text();
  const m = html.match(/<style>([\s\S]*?)<\/style>/);
  assert(m, 'page must have an inlined <style> block');
  return m[1];
}

function withAppCSS(fn) {
  return async () => {
    const css = await fetchInlinedCSS();
    const style = document.createElement('style');
    style.textContent = css;
    document.head.appendChild(style);
    try { return await fn(); } finally { style.remove(); }
  };
}

suite('modal cascade', () => {
  test('modal-overlay.modal-closed keeps display:flex (no display:none regression)', withAppCSS(async () => {
    const el = document.createElement('div');
    el.className = 'modal-overlay modal-closed';
    document.body.appendChild(el);
    try {
      const cs = getComputedStyle(el);
      assert(cs.display === 'flex',
        `.modal-overlay.modal-closed must keep display:flex (compositor-friendly), got "${cs.display}" - the .hidden{display:none!important} in the tokens layer must NOT be used on modals`);
      assert(cs.visibility === 'hidden',
        `.modal-overlay.modal-closed must have visibility:hidden, got "${cs.visibility}"`);
      assert(cs.opacity === '0',
        `.modal-overlay.modal-closed must have opacity:0, got "${cs.opacity}"`);
      assert(cs.pointerEvents === 'none',
        `.modal-overlay.modal-closed must have pointer-events:none, got "${cs.pointerEvents}"`);
    } finally { el.remove(); }
  }));

  test('modal-overlay without modal-closed is fully visible', withAppCSS(async () => {
    const el = document.createElement('div');
    el.className = 'modal-overlay';
    document.body.appendChild(el);
    try {
      const cs = getComputedStyle(el);
      assert(cs.display === 'flex', `modal-overlay should be display:flex, got "${cs.display}"`);
      assert(cs.visibility === 'visible', `modal-overlay should be visible, got "${cs.visibility}"`);
      assert(cs.opacity === '1', `modal-overlay should have opacity:1, got "${cs.opacity}"`);
    } finally { el.remove(); }
  }));

  test('modal-overlay has transition on opacity (fade animation)', withAppCSS(async () => {
    const el = document.createElement('div');
    el.className = 'modal-overlay';
    document.body.appendChild(el);
    try {
      const cs = getComputedStyle(el);
      // When prefers-reduced-motion: reduce is active, the tokens layer sets
      // `transition: none !important` on all elements (accessibility override).
      // In that case, verify the transition is in the CSS SOURCE rather than
      // the computed style (which is force-overridden to "none").
      if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
        const css = await fetchInlinedCSS();
        assert(/\.modal-overlay\s*\{[^}]*transition:\s*opacity/.test(css),
          '.modal-overlay CSS should declare transition on opacity (reduced-motion override hides it from computed style)');
      } else {
        assert(cs.transitionProperty.includes('opacity'),
          `.modal-overlay should transition opacity, got transition-property: "${cs.transitionProperty}"`);
      }
    } finally { el.remove(); }
  }));

  test('no modal-overlay uses the global .hidden class in the template', async () => {
    const r = await fetch('/', { cache: 'no-store' });
    const html = await r.text();
    const modalOverlays = html.match(/class="modal-overlay[^"]*"/g) || [];
    assert(modalOverlays.length >= 4, `expected at least 4 modal-overlay elements, found ${modalOverlays.length}`);
    for (const cls of modalOverlays) {
      assert(!/\bhidden\b/.test(cls),
        `modal-overlay should use .modal-closed, not .hidden: "${cls}"`);
    }
  });
});
