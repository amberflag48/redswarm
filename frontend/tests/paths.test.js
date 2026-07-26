import { test, suite } from './harness.js';
import { MODULE_PATHS } from './modules.js';

// Path validation - dynamically import every library module and assert it
// resolves. Catches broken import paths and missing named exports (e.g.
// `import { x } from './wrong.js'`), which would otherwise throw at module
// evaluation and silently break the whole graph. Dynamic import + try/catch
// per module means one failure is reported precisely, not as a blank page.
suite('paths', () => {
  for (const path of MODULE_PATHS) {
    test(`import ${path.replace('../js/', '')} resolves`, async () => {
      try {
        await import(path);
      } catch (e) {
        throw new Error(`${path} failed to import: ${(e && e.message) || e}`);
      }
    });
  }
});
