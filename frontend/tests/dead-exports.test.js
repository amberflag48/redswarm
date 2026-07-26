import { test, suite, assert } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Dead-export detection: every named export should be imported by at least one
// OTHER module, test file, or the bootstrap entry (main.js). An export nothing
// imports is dead code - the most reliable signal the codebase has drifted (a
// refactor removed the last consumer but left the export behind). Self-imports
// don't count; re-exports count as usage; test-file imports count so test-only
// exports aren't flagged.

const NAMED_IMPORT = /import\s*\{([^}]*)\}\s*from\s*['"]([^'"]+)['"]/g;
const REEXPORT = /export\s*\{([^}]*)\}\s*from\s*['"]([^'"]+)['"]/g;
const EXPORT_DECL = /export\s+(?:async\s+)?(?:function|class)\s+(\w+)|export\s+(?:const|let)\s+(\w+)/g;
// `export { a, b as c }` without a `from` clause (plain re-export of locals).
const EXPORT_LIST = /export\s*\{([^}]*)\}(?!\s*from)/g;

async function fetchText(path) {
  const url = new URL(path, import.meta.url).href;
  const r = await fetch(url, { cache: 'no-store' });
  if (!r.ok) throw new Error(`${path} → HTTP ${r.status}`);
  return r.text();
}

// Extract exported names from a module source.
function parseExports(src) {
  const names = new Set();
  let m;
  EXPORT_DECL.lastIndex = 0;
  while ((m = EXPORT_DECL.exec(src))) names.add(m[1] || m[2]);
  EXPORT_LIST.lastIndex = 0;
  while ((m = EXPORT_LIST.exec(src))) {
    for (const binding of m[1].split(',').map(s => s.trim()).filter(Boolean)) {
      // `a as b` → exported name is `b`; plain `a` → `a`.
      names.add(binding.split(/\s+as\s+/).pop().trim());
    }
  }
  REEXPORT.lastIndex = 0;
  while ((m = REEXPORT.exec(src))) {
    for (const binding of m[1].split(',').map(s => s.trim()).filter(Boolean)) {
      names.add(binding.split(/\s+as\s+/).pop().trim());
    }
  }
  return names;
}

// Extract imported names (the exported name - the part before `as`).
function parseImports(src) {
  const names = new Set();
  let m;
  NAMED_IMPORT.lastIndex = 0;
  while ((m = NAMED_IMPORT.exec(src))) {
    for (const binding of m[1].split(',').map(s => s.trim()).filter(Boolean)) {
      names.add(binding.split(/\s+as\s+/)[0].trim());
    }
  }
  REEXPORT.lastIndex = 0;
  while ((m = REEXPORT.exec(src))) {
    for (const binding of m[1].split(',').map(s => s.trim()).filter(Boolean)) {
      names.add(binding.split(/\s+as\s+/)[0].trim());
    }
  }
  return names;
}

// Read tests/index.html and extract the .test.js script src paths.
async function testFilePaths() {
  const html = await fetchText('/static/tests/index.html');
  const paths = new Set();
  for (const m of html.matchAll(/<script[^>]*\bsrc="([^"]+\.test\.js)"/g)) {
    paths.add(m[1]);
  }
  return [...paths];
}

suite('dead exports', () => {
  test('every named export is imported by another module, test, or main.js', async () => {
    // exports: Map<name, Set<exporterPath>>
    // importers: Map<name, Set<importerPath>>
    const exports = new Map();
    const importers = new Map();
    const addExport = (n, p) => { if (!exports.has(n)) exports.set(n, new Set()); exports.get(n).add(p); };
    const addImport = (n, p) => { if (!importers.has(n)) importers.set(n, new Set()); importers.get(n).add(p); };

    // 1. Parse exports + imports from every library module.
    for (const path of MODULE_PATHS) {
      const src = await fetchText(path);
      for (const name of parseExports(src)) addExport(name, path);
      for (const name of parseImports(src)) addImport(name, path);
    }

    // 2. main.js is the bootstrap entry (excluded from MODULE_PATHS) but still
    //    imports `init` - include its imports so init isn't flagged as dead.
    try {
      const mainSrc = await fetchText('../js/main.js');
      for (const name of parseImports(mainSrc)) addImport(name, '../js/main.js');
    } catch { /* main.js may be absent in some builds - skip gracefully */ }

    // 3. Test files import module exports directly (settings-dirty.test.js
    //    imports state, snapshotForm, etc.) - include them so test-only
    //    exports aren't flagged as dead.
    for (const tpath of await testFilePaths()) {
      try {
        const src = await fetchText(tpath);
        for (const name of parseImports(src)) addImport(name, tpath);
      } catch { /* skip unreachable test files */ }
    }

    // 4. For each export, check if any importer is NOT also an exporter of it
    //    (self-imports don't count).
    const dead = [];
    for (const [name, exporters] of exports) {
      const users = importers.get(name);
      if (!users || users.size === 0) { dead.push(name); continue; }
      const external = [...users].some(u => !exporters.has(u));
      if (!external) dead.push(name);
    }

    assert(dead.length === 0,
      'exports never imported by another module or test (dead exports):\n  ' + dead.join('\n  '));
  });
});
