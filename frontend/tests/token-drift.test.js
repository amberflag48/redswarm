import { test, suite, assert } from './harness.js';

// Token-drift detection: CSS values that appear 3+ times outside `:root` are
// hardcoded duplicates that should be a token. The `:root` block defines the
// tokens - values there are correct. A literal repeated in 3+ rule
// declarations outside `:root` means someone bypassed the token system and
// the value will drift when the token changes. Each category is checked
// independently so the failure report names the exact value and a suggested
// token name.

async function fetchText(path) {
  const r = await fetch(path, { cache: 'no-store' });
  if (!r.ok) throw new Error(`${path} → HTTP ${r.status}`);
  return r.text();
}

// Extract the inlined <style> content from the served page.
function extractStyle(html) {
  const m = html.match(/<style>([\s\S]*?)<\/style>/);
  return m ? m[1] : '';
}

// Remove the first `:root { … }` block (the token definitions) via brace
// matching. Values inside :root are correct by definition - only values
// outside :root are checked for drift.
function removeRootBlock(css) {
  const idx = css.indexOf(':root');
  if (idx === -1) return css;
  const open = css.indexOf('{', idx);
  if (open === -1) return css;
  let depth = 1, i = open + 1;
  while (i < css.length && depth > 0) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}') depth--;
    i++;
  }
  return css.slice(0, idx) + css.slice(i);
}

// Extract property values: `prop: value` up to `;` or `}`.
function extractValues(css, prop) {
  const out = [];
  const re = new RegExp(prop + '\\s*:\\s*([^;}]*)', 'g');
  let m;
  while ((m = re.exec(css))) out.push(m[1].trim());
  return out;
}

// Count occurrences and return entries appearing `threshold` or more times.
// Values already using var() are skipped - they reference a token.
function repeatedValues(values, threshold) {
  const counts = new Map();
  for (const v of values) {
    if (/\bvar\s*\(/.test(v)) continue;      // already a token reference
    const norm = v.replace(/\s+/g, ' ').trim().toLowerCase();
    if (!norm) continue;
    counts.set(norm, (counts.get(norm) || 0) + 1);
  }
  return [...counts.entries()].filter(([, n]) => n >= threshold).sort((a, b) => b[1] - a[1]);
}

// Split a spacing declaration's value into individual length components,
// skipping values already tokenized (var(...)) and non-lengths (0, auto).
// This catches component-level reuse that repeatedValues() misses:
// `padding: 0.75rem 1rem` and `padding: 0.75rem 1.25rem` both contribute `0.75rem`.
function extractLengthComponents(values) {
  const out = [];
  for (const v of values) {
    if (/\bvar\s*\(/.test(v)) continue;          // already a --space-* token
    for (const tok of v.split(/\s+/)) {
      if (tok === '0' || tok === 'auto') continue;  // conventionally literal
      if (/^[0-9.]+(rem|em|px|vh|vw|%)?$/.test(tok)) out.push(tok);
    }
  }
  return out;
}

const THRESHOLD = 3;

suite('token drift', () => {
  let css;
  test('served page inlines a <style> block', async () => {
    const html = await fetchText('/');
    css = extractStyle(html);
    assert(css, 'served page must inline a <style> block');
  });

  test('no hex color literal repeated 3+ times outside :root', () => {
    const body = removeRootBlock(css);
    const colors = [];
    for (const m of body.matchAll(/#[0-9a-f]{3,8}\b/gi)) colors.push(m[0].toLowerCase());
    const counts = new Map();
    for (const c of colors) counts.set(c, (counts.get(c) || 0) + 1);
    const repeated = [...counts.entries()].filter(([, n]) => n >= THRESHOLD);
    assert(repeated.length === 0,
      'hex colors repeated 3+ times outside :root - define a --token:\n  ' +
      repeated.map(([v, n]) => `${v} ×${n}`).join('\n  '));
  });

  test('no rgba() color repeated 3+ times outside :root', () => {
    const body = removeRootBlock(css);
    const colors = [];
    for (const m of body.matchAll(/rgba?\([^)]*\)/g)) colors.push(m[0].replace(/\s+/g, '').toLowerCase());
    const counts = new Map();
    for (const c of colors) counts.set(c, (counts.get(c) || 0) + 1);
    const repeated = [...counts.entries()].filter(([, n]) => n >= THRESHOLD);
    assert(repeated.length === 0,
      'rgba() colors repeated 3+ times outside :root - define a --token:\n  ' +
      repeated.map(([v, n]) => `${v} ×${n}`).join('\n  '));
  });

  test('no font-size literal repeated 3+ times outside :root', () => {
    const body = removeRootBlock(css);
    const repeated = repeatedValues(extractValues(body, 'font-size'), THRESHOLD);
    assert(repeated.length === 0,
      'font-size values repeated 3+ times outside :root - define a --fs-* token:\n  ' +
      repeated.map(([v, n]) => `${v} ×${n}`).join('\n  '));
  });

  test('no z-index literal repeated 3+ times outside :root', () => {
    const body = removeRootBlock(css);
    const repeated = repeatedValues(extractValues(body, 'z-index'), THRESHOLD);
    assert(repeated.length === 0,
      'z-index values repeated 3+ times outside :root - define a --z-* token:\n  ' +
      repeated.map(([v, n]) => `${v} ×${n}`).join('\n  '));
  });

  test('no font-family stack outside :root that isn\'t var(--font-*)', () => {
    const body = removeRootBlock(css);
    const stacks = extractValues(body, 'font-family').filter(v => !/\bvar\(--font/.test(v));
    const repeated = repeatedValues(stacks, THRESHOLD);
    assert(repeated.length === 0,
      'font-family stacks repeated 3+ times outside :root - define a --font-* token:\n  ' +
      repeated.map(([v, n]) => `${v} ×${n}`).join('\n  '));
  });

  test('no border-radius literal repeated 3+ times outside :root', () => {
    const body = removeRootBlock(css);
    const repeated = repeatedValues(extractValues(body, 'border-radius'), THRESHOLD);
    assert(repeated.length === 0,
      'border-radius values repeated 3+ times outside :root - use the existing --radius-* tokens:\n  ' +
      repeated.map(([v, n]) => `${v} ×${n}`).join('\n  '));
  });

  test('no spacing literal repeated 3+ times outside :root', () => {
    const body = removeRootBlock(css);
    const SPACING_PROPS = [
      'padding', 'padding-top', 'padding-bottom', 'padding-left', 'padding-right',
      'margin',  'margin-top',  'margin-bottom',  'margin-left',  'margin-right',
      'gap', 'column-gap', 'row-gap',
    ];
    const vals = SPACING_PROPS.flatMap(p => extractValues(body, p));
    const repeated = repeatedValues(extractLengthComponents(vals), THRESHOLD);
    assert(repeated.length === 0,
      'spacing values repeated 3+ times outside :root - define a --space-* token:\n  ' +
      repeated.map(([v, n]) => `${v} ×${n}  →  --space-${v.replace(/rem$|em$|px$/, '').replace(/\./g, '-')}`)
              .join('\n  '));
  });
});
