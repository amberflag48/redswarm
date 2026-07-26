# Frontend performance & architecture guide

> How RedSwarm achieves 100/100 Lighthouse, sub-60ms LCP, instant interactions,
> and zero-duplicate maintainable code. Every rule below is enforced by an
> automated test (Rust or browser harness).

---

## Table of contents

1. [Page load: the complete pipeline](#1-page-load-the-complete-pipeline)
2. [Caching: fingerprinting + Cache-Control](#2-caching-fingerprinting--cache-control)
3. [CSS architecture: tokens, layers, containment](#3-css-architecture-tokens-layers-containment)
4. [JS architecture: shared modules, no duplication](#4-js-architecture-shared-modules-no-duplication)
5. [SSE: targeted DOM updates, no polling](#5-sse-targeted-dom-updates-no-polling)
6. [Interaction latency: INP and the vsync floor](#6-interaction-latency-inp-and-the-vsync-floor)
7. [Testing: what and how](#7-testing-what-and-how)
8. [Things to avoid](#8-things-to-avoid)
9. [Build pipeline](#9-build-pipeline)
10. [Quick reference](#10-quick-reference)

---

## 1. Page load: the complete pipeline

### The goal

| Metric | Target | How we hit it |
|---|---|---|
| TTFB | < 5ms | Server responds in 0.1ms; IPv6 dual-stack bind eliminates `localhost` fallback delay |
| LCP | < 60ms | Server-rendered HTML (no client-side hydration), inlined CSS, no render-blocking requests |
| CLS | 0.00 | Server-rendered DOM with fixed dimensions; no async layout-shifting inserts |
| INP | < 35ms | 0.6-2ms JS handlers; compositor-friendly modal show/hide; `contain` isolates layout |

### What makes this possible

**Server-side rendering (Askama).** The entire first-paint DOM is in the initial
HTML response. No JS hydration, no client-side rendering, no loading spinners.
The browser receives final HTML and paints immediately.

**Inlined CSS.** All CSS (~24KB raw, ~7KB gzipped) is inlined in a single
`<style>` block in `<head>`. No render-blocking stylesheet request. For a
single-page app with one page, inlining beats external CSS - there are no
subsequent page loads to benefit from caching, and 24KB is well under the 14KB
first-RTT budget.

**Deferred JS.** The JS bundle is `<script defer src="/static/bundle.<hash>.js">`
at the end of `<body>`. It doesn't block first paint. The `defer` attribute
means it executes after HTML parsing, in order - no race conditions.

**No external requests on first paint.** Zero web fonts, zero CDN images,
zero third-party scripts. The only network requests are: the HTML document, the
JS bundle (immutable-cached), the favicon (SVG, revalidated), and the SSE stream.
System fonts (`system-ui`) eliminate font download + CLS entirely.

### What NOT to do

- **Do not add a web font.** `system-ui` is instant, zero bytes, zero CLS. A
  self-hosted variable WOFF2 adds 30KB+ and a layout shift on swap.
- **Do not add a render-blocking `<link rel="stylesheet">`.** The
  `assets.test.js` test asserts no `<link rel="stylesheet">` exists.
- **Do not lazy-load the LCP element.** The LCP is text in the server-rendered
  HTML - it's already there. Lazy-loading adds 500ms+.
- **Do not add `<link rel="preload">` for the JS bundle.** `defer` already
  schedules it; preload would compete with the HTML parse.
- **Do not use `display: none` → `display: flex` for modal show/hide.** This
  forces a full layout of the subtree (see §6).

### Verifying page load

```bash
# Lighthouse (must be 100/100/100/100)
# Run via Chrome DevTools MCP: lighthouse_audit (desktop, navigation mode)

# Performance trace (TTFB + LCP breakdown)
# Run via Chrome DevTools MCP: performance_start_trace (reload=true, autoStop=true)

# Manual curl check (TTFB should be < 2ms)
curl -s -o /dev/null -w "ttfb=%{time_starttransfer}s total=%{time_total}s\n" http://127.0.0.1:3000/
```

---

## 2. Caching: fingerprinting + Cache-Control

### The strategy

| Asset | Cache-Control | Why |
|---|---|---|
| `/` (HTML document) | `no-cache` | Always revalidate - the HTML carries the current bundle fingerprint. Must be fresh so the browser picks up new JS. |
| `/static/bundle.<hash>.js` | `public, max-age=31536000, immutable` | URL changes iff content changes (SHA-256 hash). Safe to cache forever. |
| `/static/favicon.svg`, `logo.png`, `og.png` | `no-cache` | Revalidate via `Last-Modified` (ServeDir sets it). Free 304 when unchanged, fresh copy when changed. |
| `/api/events` (SSE stream) | *(not set)* | Long-lived stream - caching is nonsensical. The middleware returns `None` so no `Cache-Control` header is sent. |
| Other API responses (`/api/*`) | `no-cache` | Dynamic data, never cache. |

### How cache busting works

`build.sh` fingerprints the JS bundle with a SHA-256 hash of its content:

```bash
BUNDLE_HASH=$(sha256sum frontend/.bundle.tmp.js | cut -c1-12)
BUNDLE_NAME="bundle.${BUNDLE_HASH}.js"
```

When JS content changes → new hash → new URL → browser fetches fresh JS
automatically. When JS content is unchanged → same hash → same URL → browser
uses disk cache (zero bandwidth, zero latency).

The HTML document (`no-cache`) always carries the current fingerprint in the
`<script defer src="...">` tag, so the browser discovers the new URL on its
very next visit. There is no stale-JS window.

### The Cache-Control middleware

The caching policy is enforced by a single middleware in `src/api.rs`. The
per-route policy lives in a pure, unit-tested function `cache_control_for_path`
that returns `Option<&'static str>` - `None` means "send no header" (used for
the SSE stream):

```rust
// Pure routing function - extracted so the per-route policy is unit-testable.
fn cache_control_for_path(path: &str) -> Option<&'static str> {
    // The global SSE stream is a long-lived connection; Cache-Control is
    // nonsensical and forbidden (see "What NOT to do" below).
    if path == crate::data::sse::EVENTS_ROUTE {
        return None;
    }
    if path == "/" {
        Some(crate::data::protocol::CACHE_NO_CACHE)        // HTML: always revalidate
    } else if path.starts_with("/static/bundle.") && path.ends_with(".js") {
        Some(crate::data::protocol::CACHE_IMMUTABLE)       // fingerprinted → cache forever
    } else if path.starts_with("/static/") {
        Some(crate::data::protocol::CACHE_NO_CACHE)       // favicon/images: revalidate
    } else {
        Some(crate::data::protocol::CACHE_NO_CACHE)       // /api/*: dynamic
    }
}

async fn cache_control_layer(OriginalUri(uri): OriginalUri, request: Request, next: Next) -> Response {
    let path = uri.path();
    let mut response = next.run(request).await;
    if let Some(cache_control) = cache_control_for_path(path) {
        response.headers_mut().insert(header::CACHE_CONTROL, cache_control.parse().unwrap());
    }
    response
}
```

Wired into the router: `.layer(middleware::from_fn(cache_control_layer))`. The
directive strings (`CACHE_NO_CACHE`, `CACHE_IMMUTABLE`) are constants in
`src/data/protocol.rs` - the middleware never hand-types a header value.

### Verifying cache busting

```bash
# 1. Note the current hash
ls frontend/bundle.*.js

# 2. Modify any JS source file
echo '// test' >> frontend/js/utils/format.js

# 3. Rebuild
./build.sh
# → New hash, old bundle deleted, HTML updated

# 4. Verify
grep -oP '/static/bundle\.[a-f0-9]+\.js' templates/index.html
# → Should show the new hash

# 5. Revert
sed -i '/^\/\/ test$/d' frontend/js/utils/format.js
./build.sh
# → Hash returns to original (deterministic)
```

### What NOT to do

- **Do not add `Cache-Control: max-age` to the HTML document.** If the browser
  caches the HTML, it won't discover the new bundle URL after a rebuild.
- **Do not use query-string cache busting** (`bundle.js?v=123`). Proxies may
  not cache query-string URLs. Content-hash in the filename is the robust
  standard.
- **Do not set `Cache-Control` on the SSE stream** (`/api/events`). It's a
  long-lived stream; caching makes no sense.

---

## 3. CSS architecture: tokens, layers, containment

### Token system (single source of truth)

All colors, spacing, radii, fonts, z-index, font-sizes, and modal widths live
in `frontend/styles/tokens.css` as CSS custom properties:

```css
@layer tokens {
:root {
    --bg: #0f1117; --surface: #1a1d27; ...
    --accent: #a8525a; --on-accent: #ffffff;
    --font-mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    --z-modal: 50; --z-modal-capture: 60; --z-modal-confirm: 70; --z-toast: 100;
    --fs-label: 0.68rem; --fs-meta: 0.78rem; ...
    --modal-width: 640px; --modal-width-settings: 860px; ...
}
}
```

No value is ever hardcoded as a literal in another CSS file. The
`token-drift.test.js` test asserts no hex color, font-size, z-index,
border-radius, or font-family literal appears 3+ times outside `:root`.

### Cascade layers

Every CSS file is wrapped in a named `@layer`, declared in order at the top of
`tokens.css`:

```css
@layer tokens, base, layout, components, modal, log, capture, toast, animations;
```

This fixes precedence: a later layer (e.g. `modal`) overrides an earlier layer
(e.g. `components`) at equal specificity, eliminating `!important` wars.

**Important caveat with `!important` and `@layer`:** the cascade layer order
is **inverted** for `!important` declarations - an `!important` in an earlier
layer wins over an `!important` in a later layer. This is why modals use the
dedicated `.modal-overlay.modal-closed` class (in the later `modal` layer)
rather than the global `.hidden` utility: `.hidden { display: none !important }`
lives in the earlier `tokens` layer, so applying it to an overlay would
silently win and revert to the slow `display: none → display: flex` toggle.
Using a separate `.modal-closed` class sidesteps the inversion entirely - the
`@layer` provides the specificity win without `!important`. The
`modal-cascade.test.js` test pins this.

### Containment (`contain` and `content-visibility`)

- **`.body`** and **`.modal-overlay`**: `contain: layout style paint` - tells
  the browser these subtrees are independent, so opening a modal doesn't
  re-layout the body behind it.
- **`#log-panel`**: `contain: layout style paint` - isolates the log panel's
  layout from the rest of the page.
- **`.task-table tbody tr`**: `content-visibility: auto; contain-intrinsic-size:
  auto 33px` - skips rendering of offscreen task rows.

Do NOT use `content-visibility: auto` on elements that are always visible (like
`#log-panel` was originally) - it adds intersection-observer overhead with no
benefit. Use `contain` instead for always-visible elements.

### Compositor-friendly modal show/hide

Modals use `visibility: hidden; opacity: 0; pointer-events: none` instead of
`display: none` when hidden:

```css
.modal-overlay.modal-closed { display: flex; visibility: hidden; opacity: 0; pointer-events: none; }
```

This keeps the overlay laid out (one-time cost at first paint), so toggling
visibility is a compositor-only opacity change. The `will-change: opacity`
pre-promotes the layer. See §6 for why `display: none → flex` is slower.

### What NOT to do

- **No `backdrop-filter: blur()`.** Gaussian blur of the full viewport behind
  the modal is the single most expensive paint operation. Use a solid
  `rgba(0,0,0,.6)` overlay instead.
- **No inline `style="..."` in the template.** The `inline-styles.test.js`
  test asserts zero inline styles in `<body>`. Every visual property belongs in
  a CSS class.
- **No `!important` in component/modal layers.** Use `@layer` ordering instead.
  The only `!important` declarations live in the `tokens` layer: `.hidden { display: none !important }`
  (the global hide utility) and the `prefers-reduced-motion` override
  (`transition: none !important; animation: none !important`), which must
  override all animation regardless of layer.

---

## 4. JS architecture: shared modules, no duplication

### Module structure

```
frontend/js/
├── state/          # shared mutable state
│   ├── store.js    # state object + shouldSuppressToast()
│   └── dom.js      # cached DOM refs (cacheDom), taskRow(id)
├── utils/          # pure helpers (no side effects)
│   ├── format.js   # byte/speed/duration formatters
│   ├── form.js     # clamp, hasValue, snapshotForm, isFormDirty, field errors
│   ├── dom-helpers.js  # escHtml, escAttr, focusFirst, setSegmented, wireSegmented
│   ├── buttons.js  # btnLoading, btnReset
│   └── net.js      # fetch wrappers + AbortController pending system
├── components/     # UI components (modal, toast, task-list, etc.)
├── data/           # data constants (labels, client-schema, capture-helpers)
├── services/       # SSE, client refresh, runtime settings
└── app/            # init() - wires everything together
```

### The single fetch wrapper

`utils/net.js` is the ONLY file that calls `fetch()`. The `dry.test.js` test
enforces this. All other modules use `fetchJson`, `postJson`, `postAction`,
`putJson`, `deleteAction`, or `fetchRaw` - all of which go through `request()`.

### AbortController + pending state

Every async action uses the shared pending system in `utils/net.js`:

```js
export function withPending(id) {
    clearPending(id);
    const controller = new AbortController();
    pending.set(id, controller);
    return controller;
}

export function clearPending(id) { /* abort + delete */ }
export function clearAllPending() { /* abort all - called on pagehide */ }
export function pendingSignal(id) { return withPending(id).signal; }
```

The `shared-logic.test.js` test asserts every `new AbortController` is paired
with a `withPending`/`pendingSignal`/`clearPending` call. No bare controllers.

**Critical: chain the promise.** When starting a task, the start POST must be
returned in the promise chain so `closeModal` waits for it:

```js
// CORRECT - start is returned, chain waits
return postJson('/api/audits', body, err, signal)
    .then(data => {
        if (data && data.id) {
            return postAction('/api/audits/' + data.id + '/start', err, signal)
                .then(() => data);  // ← wait for start before resolving
        }
        return null;
    });

// WRONG - start is fire-and-forget, closeModal aborts it
return postJson('/api/audits', body, err, signal)
    .then(data => {
        if (data && data.id) {
            postAction('/api/audits/' + data.id + '/start', err, signal); // not returned!
            return data;
        }
    });
```

### Toast suppression

A single `shouldSuppressToast()` in `state/store.js` checks if a self-action
was recent (within 2 seconds). The `shared-logic.test.js` test asserts
`Date.now() - state.*` appears at most once across all modules - all checks
go through the shared helper.

### Modal framework

`components/modal.js` provides:
- `openModalEl(el, closeFn)` - opens a modal (focus trap, scroll lock, Escape)
- `closeModalEl()` - closes the top modal
- `registerModal({ isDirty, noun, reset })` - factory for content modals that
  confirms-discard-if-dirty, closes, and resets in one shared function

All 3 content modals (New task, Settings, Capture) use `registerModal()`:

```js
export const closeModal = registerModal({ isDirty: editIsDirty, noun: 'changes', reset: resetTaskModal });
```

### Event delegation

The task table uses a single delegated click listener on `#audit-list`:

```js
dom.audit_list.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action]');
    if (!btn) return;
    const row = btn.closest('[data-id]');
    if (!row) return;
    switch (btn.dataset.action) {
        case 'stop': stopAudit(row.dataset.id); break;
        case 'start': restartAudit(row.dataset.id); break;
        // ...
    }
});
```

No per-row listeners. SSE-injected rows get free listeners automatically.

### Optimistic UI

Action handlers update the DOM immediately, then send the request. On
failure, they revert:

```js
export async function stopAudit(id) {
    const row = taskRow(id);
    const prevStatus = row ? getBadgeStatus(row) : null;
    if (row) setTaskStatus(id, 'stopped');  // optimistic
    try {
        await postAction(`/api/audits/${id}/stop`, 'stop audit');
    } catch (e) {
        if (row && prevStatus) setTaskStatus(id, prevStatus);  // rollback
        toast('Failed to stop task', 'error');
    }
}
```

### What NOT to do

- **No bare `fetch()` outside `net.js`.** Enforced by `dry.test.js`.
- **No manual `setTimeout(() => focusFirst(root), 0)`.** Use synchronous
  `focusFirst(root)` - the setTimeout pattern adds an extra style-recalc +
  layout round (~15ms per modal open).
- **No `Date.now() - state.*` checks outside `store.js`.** Use
  `shouldSuppressToast()`. Enforced by `shared-logic.test.js`.
- **No manual HTML escaping.** Use `escHtml`/`escAttr` from `dom-helpers.js`.
  Enforced by `dry.test.js`.
- **No `style="..."` in JS-built HTML strings.** Use CSS classes. Enforced by
  `dry.test.js`.
- **No over-exported functions.** If a function is only used inside its own
  module, don't `export` it. Enforced by `dead-exports.test.js`.

---

## 5. SSE: targeted DOM updates, no polling

### The architecture

A single global SSE connection (`GET /api/events`) drives all live updates.
No polling, no WebSocket, no multiple connections. The browser opens one
`EventSource` on page load; the server pushes events as they happen.

### Event types

The SSE stream carries 13 event types (defined in `src/data/sse.rs`):

| Event | When | JS handler |
|---|---|---|
| `audit` | A log event for a task (announce result, speed, etc.) | `appendLogRow`, `updateLogStats` |
| `task_created` | A new task was created | `addTaskRow` |
| `task_deleted` | A task was deleted | `removeTaskRow` |
| `task_status` | A task's status changed (running/stopped) | `setTaskStatus` |
| `task_client` | The working client was identified during probing (or set directly when a forced client skips probing) | `setTaskClient` |
| `task_progress` | Uploaded/downloaded counters updated | `setTaskProgress`, `flashCell` |
| `task_updated` | A task's mode/strategy was edited via settings | `setTaskUpdated` |
| `config_reloaded` | `config.toml` was hot-reloaded at runtime | `applyRuntimeSettings`, `refreshClientDropdown`, `populateSettingsFields` |
| `capture_progress` | A fingerprint-capture session advanced (state-machine transition) | `updateCaptureUI`, `showCaptureSnippet` |
| `goal_progress` | A bound task's counters moved a goal's progress/ETA | `patchGoalTile` |
| `goal_created` | A new goal was created | `addGoalTile` |
| `goal_deleted` | A goal was deleted | `removeGoalTile` |
| `goal_updated` | A goal's config or bound task set changed | `patchGoalTile` |

### Targeted DOM updates (diff-based)

Each SSE event handler patches only the specific DOM cells that changed - no
`innerHTML` replacement, no full-table re-render. For example,
`setTaskProgress(id, uploaded, downloaded)` finds the row's uploaded/downloaded
`<td>` cells by `data-col` attribute and updates only their `textContent`:

```js
export function setTaskProgress(id, uploaded, downloaded) {
    const row = taskRow(id);
    if (!row) return;
    const upCell = row.querySelector('[data-col="uploaded"]');
    if (upCell && upCell.textContent !== formattedUp) {
        upCell.textContent = formattedUp;
        flashCell(upCell);
    }
    // ... same for downloaded
}
```

The `data-show-*` attributes on `.log-stats` control which stat tiles are
visible based on the task's mode (Upload Only hides download/left/speed).

### Connection state

The connection badge (`Live` / `Reconnecting` / `Disconnected`) is driven by
the `EventSource`'s `onopen`/`onerror` events. The SSE keepalive interval
(configurable: `server.sse_keepalive_secs`, default 15s) prevents idle
connection drops.

### What NOT to do

- **Do not poll.** No `setInterval(() => fetch(...), 5000)`. The SSE stream
  pushes updates as they happen - polling adds latency and server load.
- **Do not create multiple EventSource connections.** One global connection
  for all events. The server multiplexes all task updates onto one stream.
- **Do not re-render the full task table on each event.** Patch only the
  changed cells. `innerHTML` replacement destroys focus, scroll position, and
  event listeners.
- **Do not add new SSE event types without updating `sse.rs` AND the JS
  dispatcher.** The `js_sse_event_names_match_consts` Rust test checks that
  JS event listener names match `sse::EV_*` constants.

---

## 6. Interaction latency: INP and the vsync floor

### The physical floor

At 60Hz, the minimum INP for any main-thread interaction is ~30ms - two vsync
cycles (~16.7ms each). This is Chrome's rendering pipeline: main-thread
commit → compositor activates on next vsync → presents on the following vsync.

Even a 0.6ms JS handler (a single class toggle) produces 30ms INP on a 60Hz
display. This is **not our code** - it's the browser's frame schedule. A
120Hz display would halve this to ~16ms.

### What we CAN optimize

| Technique | Impact | Status |
|---|---|---|
| Remove `backdrop-filter: blur(6px)` | Saves ~8ms paint per frame | Done |
| Synchronous `focusFirst` (no setTimeout) | Saves ~15ms (extra style recalc round) | Done |
| `contain: layout style paint` on body/overlay | Isolates layout scope | Done |
| `content-visibility: auto` on offscreen rows | Skips offscreen rendering | Done |
| Compositor-friendly modal (`opacity` not `display:none`) | One vsync instead of two | Done |
| `will-change: opacity` on overlay | Pre-promotes layer | Done |

### What we CANNOT optimize

- **The vsync wait.** The browser presents on its frame schedule, not ours.
  Our JS finishes in 2ms but the browser waits until the next vsync (~11ms
  later) to begin the frame, then presents on the following vsync (~16ms
  after that).
- **The INP metric's measurement.** Chrome's INP measures to the frame where
  the compositor **flips** the change to the screen. At 60Hz, that's always
  the next-next vsync for any main-thread-triggered change.

### Verifying INP

```bash
# Via Chrome DevTools MCP:
# 1. performance_start_trace (autoStop=false, reload=false)
# 2. Click the target element
# 3. performance_stop_trace
# 4. performance_analyze_insight (insightName="INPBreakdown")
# → Input delay + Processing duration + Presentation delay = total INP
```

---

## 7. Testing: what and how

### Two test suites

| Suite | Runner | Count | Where |
|---|---|---|---|
| Rust | `cargo test` | 625 | `src/**/*.rs` (inline `#[cfg(test)]` modules) |
| Frontend | Browser (zero-dep ES module harness) | 218 | `frontend/tests/*.test.js` (29 files) |

### Frontend test harness

The harness (`frontend/tests/harness.js`) is zero-dependency - no npm, no
Node, no Jest. Tests run in the browser at `/static/tests/index.html`. The
harness provides:

- `suite(name, fn)` / `test(name, fn)` - registration
- `assert(cond, msg?)` / `assertEq(actual, expected, msg?)` - assertions
- `assertThrows(fn, msg?)` - error-path testing
- `withFixture(html, fn)` - mount HTML, run, cleanup

### What to test (and what each test catches)

| Test file | What it enforces |
|---|---|
| `format.test.js` | Byte/speed/duration formatters match Rust `fmt_bytes` etc. |
| `form.test.js` | `clamp`, `hasValue`, `snapshotForm`, `isFormDirty`, field errors |
| `client-schema.test.js` | Client field definitions, `collectOneClient`, `parseMDict` |
| `capture-helpers.test.js` | Version comparison, key format detection, fingerprint-to-client |
| `capture-helpers-extra.test.js` | `fingerprintToClient`, `clientToToml` (exact TOML output), `reconstructCaptureQuery` event templating |
| `dom-helpers.test.js` | HTML escaping (XSS vectors), focus management, segmented controls |
| `task-list-helpers.test.js` | `taskActionsHtml` (exact HTML), `resolveClientName` |
| `paths.test.js` | Every module in `MODULE_PATHS` resolves via dynamic import |
| `lint.test.js` | No TODO/FIXME/debugger/console.log in any module |
| `exports.test.js` | Every named `import { x }` resolves to an actual export |
| `unused.test.js` | Every imported binding is referenced in its module body |
| `dry.test.js` | 8 DRY rules: no bare fetch, no manual clamp, no manual escape, no `=== null`, no inline style, etc. |
| `api-paths.test.js` | Every `/api/` literal is a known route; frontend references all core endpoints |
| `assets.test.js` | HTML embeds bootstrap + fingerprinted bundle; no render-blocking CSS; bundle is reachable + non-empty |
| `modules-sync.test.js` | `MODULE_PATHS` has no duplicates; count is pinned; every import target is registered |
| `index-sync.test.js` | `tests/index.html` lists every expected `.test.js` (no orphans, no missing) |
| `settings-dirty.test.js` | Regression: remove/add settings client leaves form dirty |
| `shared-helpers.test.js` | DRY: `httpErrorMessage()` and other shared helpers must not be reimplemented across modules |
| `modal-cascade.test.js` | Regression: `.modal-overlay.modal-closed` keeps `display: flex`; global `.hidden` must not be used on modal overlays (layer-inversion) |
| `perf-transitions.test.js` | No `transition: all` / bare-time shorthand - every transition must name specific properties |
| `dead-state-css.test.js` | CSS state classes toggled by JS (`.connected`, `.reconnecting`, …) must have an actual JS application site |
| **`dead-exports.test.js`** | Every named export is imported by another module or test (no dead code) |
| **`dead-css.test.js`** | Every CSS class selector appears in HTML, JS, or server-rendered fragments |
| **`inline-styles.test.js`** | Zero `style="..."` attributes in `<body>` |
| **`token-drift.test.js`** | No hex color, font-size, z-index, border-radius, or font-family literal repeated 3+× outside `:root` |
| **`shared-logic.test.js`** | Every `AbortController` paired with pending helper; `Date.now() - state.*` ≤ 1 occurrence; modal close functions use `confirmDiscardIfDirty` or `registerModal` |
| **`dom-hooks.test.js`** | Every `data-*` emitted in rendered HTML is consumed by JS (no dead hooks) |
| **`dom-id-drift.test.js`** | Every static `getElementById(id)` in JS exists in the served HTML (no drift after template edits) |
| **`labels-sync.test.js`** | JS `labels.js` values match Rust `labels.rs` (exact casing); values appear in rendered page |

### How to add a new test

1. Create `frontend/tests/NAME.test.js`:
   ```js
   import { test, suite, assert, assertEq } from './harness.js';
   suite('name', () => {
       test('case', async () => {
           assert(cond, 'message');
       });
   });
   ```
2. Add `<script type="module" src="NAME.test.js"></script>` to
   `frontend/tests/index.html` (before `index-sync.test.js`).
3. Add the filename to `EXPECTED_TEST_FILES` in `index-sync.test.js` and
   increment the pinned count.

### How to test in the browser

```bash
# 1. Start the server: cargo run --release
# 2. Open http://localhost:3000/static/tests/index.html
# 3. The page title shows "N passed" or "(N) failing"
# 4. Or via Chrome DevTools MCP:
#    navigate_page → http://localhost:3000/static/tests/index.html
#    wait_for → ["passed", "failing"]
#    take_snapshot → read the result
```

---

## 8. Things to avoid

### CSS

- No `backdrop-filter: blur()` - too expensive for paint
- No `!important` in component/modal layers (use `@layer` ordering instead)
- No inline `style="..."` in HTML (use classes)
- No hardcoded values outside `:root` (use tokens)
- No `content-visibility: auto` on always-visible elements (use `contain`)

### JS

- No bare `fetch()` outside `net.js`
- No `setTimeout(() => focusFirst(...), 0)` (synchronous is faster)
- No `Date.now() - state.*` outside `store.js` (use `shouldSuppressToast`)
- No manual HTML escaping (use `escHtml`/`escAttr`)
- No `style="..."` in JS-built HTML strings (use CSS classes)
- No over-exported functions (only export what other modules import)
- No fire-and-forget `postAction` in a promise chain (return it so the
  chain waits - otherwise `clearPending` aborts it)

### Architecture

- No polling (use SSE)
- No multiple EventSource connections (one global stream)
- No full-table re-render on SSE events (patch only changed cells)
- No web fonts (system-ui is instant, zero CLS)
- No render-blocking `<link rel="stylesheet">` (inline CSS)
- No `display: none → flex` for modal show/hide (use opacity/visibility)
- No new SSE event types without updating both `sse.rs` and JS

---

## 9. Build pipeline

### `build.sh` (run after any CSS or JS change)

```
build.sh
  ├─ Concatenate frontend/styles/*.css → frontend/bundle.css
  ├─ Concatenate the MODULES array (25 ordered entries, stripping import/export) → .bundle.tmp.js
  ├─ SHA-256 hash → bundle.<hash>.js (delete old, rename)
  ├─ Inline bundle.css into templates/index.html <style> block
  ├─ Wire <script defer src="/static/bundle.<hash>.js"> into index.html
  └─ Print final sizes
```

After `build.sh`, run `cargo build --release` (the template is compiled into
the binary at build time - Askama templates are not read at runtime).

### Module list

The JS module list lives in TWO places:
- `build.sh` `MODULES` array (25 entries) - the concatenation order
- `frontend/tests/modules.js` `MODULE_PATHS` array (25 entries) - the test paths

`modules-sync.test.js` checks `MODULE_PATHS` in isolation (no duplicates, count
pinned, every import target registered). Because `build.sh` is not reachable
from the browser, the cross-check that the two lists match is enforced by the
`build_sh_modules_match_modules_js` Rust test in `src/data/mod.rs`. Adding a
new module requires updating both lists - drift is caught by that Rust test.

---

## 10. Quick reference

### Performance targets

```
TTFB:  < 5ms    (server responds in 0.1ms; IPv6 dual-stack)
LCP:   < 60ms   (server-rendered, inlined CSS, no render-blocking)
CLS:   0.00     (fixed dimensions, no async inserts)
INP:   < 35ms   (2ms JS + vsync floor)
Lighthouse: 100/100/100/100 (accessibility/best-practices/SEO/agentic)
```

### Commands

```bash
# Build (CSS + JS bundle + inline)
./build.sh

# Compile (template baked into binary)
cargo build --release

# Run tests
cargo test                          # 625 Rust tests
# Open /static/tests/index.html     # 218 frontend tests

# Lint
cargo clippy -- -W warnings

# Run the app
cargo run --release
```

### Key files

```
frontend/styles/tokens.css     # all CSS tokens (single source of truth)
frontend/js/utils/net.js        # fetch wrappers + AbortController pending
frontend/js/state/store.js      # shared state + shouldSuppressToast
frontend/js/components/modal.js # modal framework + registerModal
frontend/tests/harness.js       # zero-dep test harness
frontend/tests/dry.test.js       # DRY rules (no bare fetch, no manual escape, etc.)
build.sh                       # CSS/JS bundling + fingerprinting + inlining
src/data/sse.rs                # SSE event name constants
src/data/labels.rs             # UI label constants (single source of truth)
src/api.rs cache_control_layer # per-route Cache-Control middleware
src/main.rs bind_dual_stack    # IPv4 + IPv6 dual-stack listener
```
