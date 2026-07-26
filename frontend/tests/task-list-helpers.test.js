import { test, suite, assert, assertEq } from './harness.js';
import { taskActionsHtml, resolveClientName } from '../js/components/task-list.js';
import { state } from '../js/state/store.js';

// Pure helpers that build task-list HTML. taskActionsHtml renders the per-row
// action buttons (XSS-relevant: the id is interpolated into attributes), and
// resolveClientName maps a working_client key to a display name via the shared
// state.clientMap. Exact-string assertions pin the markup so attribute-order
// or action-set regressions fail loudly.

suite('taskActionsHtml', () => {
  const RUNNING = '<button class="act-edit" data-action="edit" data-id="1" aria-label="Edit task 1">Edit</button><button class="act-stop" data-action="stop" data-id="1" aria-label="Stop task 1">Stop</button><button class="act-del" data-action="delete" data-id="1" aria-label="Delete task 1">Delete</button>';
  const STOPPED = '<button class="act-edit" data-action="edit" data-id="2" aria-label="Edit task 2">Edit</button><button class="act-start" data-action="start" data-id="2" aria-label="Start task 2">Start</button><button class="act-del" data-action="delete" data-id="2" aria-label="Delete task 2">Delete</button>';

  test('taskActionsHtml(1, "running") renders edit/stop/delete (exact)', () =>
    assertEq(taskActionsHtml(1, 'running'), RUNNING));

  test('running row contains stop, edit, delete and data-id="1"', () => {
    const out = taskActionsHtml(1, 'running');
    assert(out.includes('data-action="stop"'), 'has stop');
    assert(out.includes('data-action="edit"'), 'has edit');
    assert(out.includes('data-action="delete"'), 'has delete');
    assert(out.includes('data-id="1"'), 'carries data-id="1"');
  });

  test('taskActionsHtml(2, "stopped") renders edit/start/delete (exact)', () =>
    assertEq(taskActionsHtml(2, 'stopped'), STOPPED));

  test('stopped row contains start and NOT stop', () => {
    const out = taskActionsHtml(2, 'stopped');
    assert(out.includes('data-action="start"'), 'has start');
    assert(!out.includes('data-action="stop"'), 'no stop on a stopped task');
  });

  test('the id is interpolated into every action button', () => {
    const out = taskActionsHtml(42, 'running');
    assertEq(out.match(/data-id="42"/g).length, 3, 'all three buttons carry data-id="42"');
  });
});

suite('resolveClientName', () => {
  // resolveClientName reads the shared `state.clientMap`. Save it, mutate for
  // the test, and restore in finally so this suite never leaks into others
  // (the store is a singleton shared across the whole test page).
  const withMap = (map, fn) => {
    const saved = state.clientMap;
    state.clientMap = map;
    try { fn(); } finally { state.clientMap = saved; }
  };

  test('resolves a known key to its mapped display name', () =>
    withMap({ '-qB5220-': 'qBittorrent 5.2.2' }, () =>
      assertEq(resolveClientName('-qB5220-'), 'qBittorrent 5.2.2')));

  test('falls back to the raw key when it is not in the map', () =>
    withMap({ '-qB5220-': 'qBittorrent 5.2.2' }, () =>
      assertEq(resolveClientName('-unknown-'), '-unknown-')));

  test('null → hyphen', () =>
    withMap({}, () => assertEq(resolveClientName(null), '-')));

  test('undefined → hyphen', () =>
    withMap({}, () => assertEq(resolveClientName(undefined), '-')));

  test('empty-string key is falsy → hyphen (not a lookup miss)', () =>
    withMap({ '': 'ShouldNotWin' }, () => assertEq(resolveClientName(''), '-')));
});
