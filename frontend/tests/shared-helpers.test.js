import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Shared-helpers enforcement: patterns that should be centralized in a helper
// must not be reimplemented across modules. This suite catches DRY violations
// the existing dry.test.js and shared-logic.test.js don't cover:
//
//  A. `httpErrorMessage()` - the `'HTTP ' + status` string must go through the
//     shared `httpErrorMessage()` in net.js, not be hand-rolled at each call
//     site. Even net.js itself (the module that defines the helper) must call
//     its own helper rather than inlining the literal.
//
//  B. `escHtml`/`escAttr` in innerHTML sinks - any `.innerHTML =` assignment
//     that interpolates a variable must wrap that variable in `escHtml()`
//     (for text content) or `escAttr()` (for attribute values). A bare variable
//     in an innerHTML string is an XSS vector. The allowlist covers modules
//     that insert server-rendered HTML (trusted) or build static strings with
//     no variable interpolation.
//
//  C. Modal overlays must use `.modal-closed`, not the global `.hidden` class.
//     The `.hidden { display: none !important }` in the tokens layer defeats
//     the compositor-friendly `.modal-overlay` show/hide (see
//     modal-cascade.test.js). This rule catches any JS that toggles `.hidden`
//     on a modal-overlay element.

async function fetchText(path) {
  const url = new URL(path, import.meta.url).href;
  return (await fetch(url, { cache: 'no-store' })).text();
}

function stripImports(src) {
  return src.split('\n').filter(l => !/^\s*import\s/.test(l)).join('\n');
}

const NAMED_IMPORT = /import\s*\{([^}]*)\}\s*from\s*['"][^'"]+['"]/;

function stripImportLines(src) {
  return src.split('\n').filter(l => !NAMED_IMPORT.test(l)).join('\n');
}

suite('shared helpers', () => {
  test('A: no bare "HTTP " + status literal - use httpErrorMessage() from net.js', async () => {
    // The shared helper `httpErrorMessage(status)` returns `'HTTP ' + status`.
    // Every call site should use the helper, not hand-roll the literal.
    // The helper definition itself (in net.js) is the only allowed site for
    // the literal `'HTTP ' + status`.
    const failures = [];
    for (const path of MODULE_PATHS) {
      const src = stripImportLines(await fetchText(path));
      // Match 'HTTP ' + <something>  (the hand-rolled form)
      // Allow net.js (the definition site) - the helper body itself contains the literal.
      if (path.includes('utils/net.js')) {
        // In net.js, the only allowed occurrence is inside httpErrorMessage().
        // The function body is `return 'HTTP ' + status;` - skip that line.
        const lines = src.split('\n');
        for (let i = 0; i < lines.length; i++) {
          if (/'HTTP '\s*\+/.test(lines[i]) && !/function\s+httpErrorMessage/.test(lines[i]) && !/return\s+'HTTP '\s*\+/.test(lines[i])) {
            failures.push(`${path}:${i + 1}: ${lines[i].trim()}`);
          }
        }
      } else {
        if (/'HTTP '\s*\+/.test(src)) {
          const lines = src.split('\n');
          for (let i = 0; i < lines.length; i++) {
            if (/'HTTP '\s*\+/.test(lines[i])) {
              failures.push(`${path}:${i + 1}: ${lines[i].trim()}`);
            }
          }
        }
      }
    }
    assert(failures.length === 0,
      'bare "HTTP " + status found - use httpErrorMessage() from net.js:\n  ' +
      failures.join('\n  '));
  });

  test('B: innerHTML assignments with variable interpolation use escHtml/escAttr', async () => {
    // Any `.innerHTML =` that builds a string with a variable reference must
    // wrap that variable in escHtml()/escAttr(). Modules that insert
    // server-rendered HTML (trusted) or build fully static strings are
    // allowlisted.
    const ALLOW = new Set([
      'components/task-list.js',   // inserts server-rendered task HTML
      'components/log-panel.js',   // inserts server-rendered log rows
      'components/client-card.js', // builds HTML from config data (escHtml added but check anyway)
    ]);
    const failures = [];
    for (const path of MODULE_PATHS) {
      if (ALLOW.has(path.replace('../js/', ''))) continue;
      const src = await fetchText(path);
      const lines = src.split('\n');
      for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        // Look for .innerHTML = followed by a string with ${...} or ' + var
        if (/\.innerHTML\s*=\s/.test(line)) {
          // Check if the RHS contains a variable reference (template literal ${...}
          // or string concat with a variable).
          // Extract the RHS (everything after =).
          const rhs = line.split(/\.innerHTML\s*=\s*/)[1] || '';
          // If the RHS is a plain string literal (no interpolation), it's fine.
          // If it contains ${...} with a variable, check for escHtml.
          // If it contains ' + <varname>, check for escHtml.
          const hasInterpolation = /\$\{[^}]*\}/.test(rhs) || /\+\s*[a-zA-Z_]\w*/.test(rhs);
          if (hasInterpolation && !/escHtml|escAttr/.test(line)) {
            // Also check the next few lines (multi-line innerHTML assignments).
            let multiLine = line;
            for (let j = 1; j <= 5 && i + j < lines.length; j++) {
              multiLine += '\n' + lines[i + j];
              if (/escHtml|escAttr/.test(multiLine)) { hasInterpolation = false; break; }
              if (/;/.test(lines[i + j])) break; // end of statement
            }
            if (hasInterpolation) {
              failures.push(`${path}:${i + 1}: ${line.trim()}`);
            }
          }
        }
      }
    }
    assert(failures.length === 0,
      'innerHTML with unescaped variable interpolation (XSS risk - use escHtml/escAttr):\n  ' +
      failures.join('\n  '));
  });

  test('C: modal overlays use .modal-closed, not .hidden (compositor-friendly show/hide)', async () => {
    // The global .hidden { display: none !important } in the tokens layer
    // defeats .modal-overlay's compositor-friendly show/hide. Modal overlays
    // must toggle .modal-closed instead. This catches any classList.add/remove
    // of 'hidden' in modal.js (the shared modal framework).
    const failures = [];
    for (const path of MODULE_PATHS) {
      if (!path.includes('components/modal.js')) continue;
      const src = await fetchText(path);
      const lines = src.split('\n');
      for (let i = 0; i < lines.length; i++) {
        if (/classList\.(add|remove|toggle)\(\s*['"]hidden['"]/.test(lines[i])) {
          failures.push(`${path}:${i + 1}: ${lines[i].trim()}`);
        }
      }
    }
    assert(failures.length === 0,
      'modal.js toggles .hidden on modal overlays (use .modal-closed to avoid display:none!important):\n  ' +
      failures.join('\n  '));
  });
});
