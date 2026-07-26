import { test, suite, assert } from './harness.js';

// Inline-style detection: the served page must contain ZERO inline `style="…"`
// attributes on elements in <body>. Inline styles are a DRY violation - every
// visual property belongs in a CSS class so it can be reused and changed in
// one place. The inlined <style> block (CSS property names like `font-style`)
// and the <script> bootstrap JSON (may contain the word "style") are excluded.
//
// The template ships the <body> after </style>, so we slice from there to
// </body> and strip any <script> blocks before searching for `style="…"`.

async function fetchText(path) {
  const r = await fetch(path, { cache: 'no-store' });
  if (!r.ok) throw new Error(`${path} → HTTP ${r.status}`);
  return r.text();
}

suite('inline styles', () => {
  test('the served page has zero inline style="…" attributes in <body>', async () => {
    const html = await fetchText('/');
    // Slice from the end of the inlined <style> block to </body> - this is the
    // element markup, free of CSS declarations. Then strip <script> blocks so
    // the bootstrap JSON (which may contain the token "style") can't match.
    const styleEnd = html.indexOf('</style>');
    const bodyClose = html.indexOf('</body>');
    assert(styleEnd !== -1, 'page must contain an inlined <style> block');
    assert(bodyClose !== -1, 'page must contain a </body> close tag');
    const body = html.slice(styleEnd, bodyClose).replace(/<script[\s\S]*?<\/script>/gi, '');

    // Match `style="…"` or `style='…'` as an HTML attribute (the `=` with no
    // spaces is the attribute syntax; CSS declarations inside <style> use
    // `style:` with a colon, which this doesn't match).
    const matches = [];
    for (const m of body.matchAll(/\sstyle\s*=\s*["'][^"']*["']/g)) {
      matches.push(m[0].trim());
    }

    assert(matches.length === 0,
      'inline style="…" attributes found in <body> - extract each to a CSS class:\n  ' +
      matches.join('\n  '));
  });
});
