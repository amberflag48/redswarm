// Network utilities - shared fetch wrappers with consistent error handling.
// `request` is the low-level primitive (no toast); `fetchJson`/`putJson`/
// `postJson`/`postAction`/`deleteAction` are the shared sugar every caller
// uses. No component should hand-roll `fetch()` + a manual error toast - go
// through these so error handling is identical across the app.

import { toast } from '../components/toast.js';

// Pending-fetch registry - one AbortController per logical operation id, so a
// modal close / task switch can cancel an in-flight fetch instead of letting
// it update stale DOM. `pendingSignal` is the convenience entry: it clears
// any existing controller for the id, installs a fresh one, and returns its
// signal to pass to a fetch wrapper.
const pending = new Map();

export function clearPending(id) {
  const controller = pending.get(id);
  if (controller) {
    controller.abort();
    pending.delete(id);
  }
}

export function clearAllPending() {
  for (const controller of pending.values()) controller.abort();
  pending.clear();
}

function withPending(id) {
  clearPending(id);
  const controller = new AbortController();
  pending.set(id, controller);
  return controller;
}

export function pendingSignal(id) {
  return withPending(id).signal;
}

// Low-level: returns { ok, status, text, data }. Never toasts - the caller
// decides. `text` is the response body on error (the backend's message) and
// '' on success. `data` is the parsed JSON on success, null otherwise.
// `opts.signal` (if present) is forwarded to `fetch` for cancellation.
export async function request(url, opts) {
  try {
    const r = await fetch(url, opts);
    if (!r.ok) {
      const text = await r.text();
      return { ok: false, status: r.status, text, data: null };
    }
    const data = await r.json();
    return { ok: true, status: r.status, text: '', data };
  } catch (e) {
    return { ok: false, status: 0, text: e.message || String(e), data: null };
  }
}

// The sanctioned escape hatch for non-JSON fetches (binary downloads, void
// fire-and-forget actions). Returns the raw Response - the caller checks
// `.ok`/`.headers`/`.blob()` itself. This is the ONLY `fetch` wrapper a
// component should call when it can't use `request`/`fetchJson`; the DRY tests
// forbid bare `fetch(` outside net.js.
export async function fetchRaw(url, opts) {
  return fetch(url, opts);
}

// Fetch JSON, toast on error, return parsed JSON or null.
export async function fetchJson(url, opts, errPrefix) {
  const r = await request(url, opts);
  if (!r.ok) toast(errPrefix + ': ' + (r.text || httpErrorMessage(r.status)), 'error');
  return r.ok ? r.data : null;
}

export async function putJson(url, data, errPrefix, signal) {
  return fetchJson(url, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
    signal,
  }, errPrefix);
}

export async function postJson(url, data, errPrefix, signal) {
  return fetchJson(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
    signal,
  }, errPrefix);
}

// POST/DELETE a void action (no response body), toast on error, return bool.
// Used by stop/start/delete/fire-and-forget actions where only success vs
// failure matters.
export async function postAction(url, errPrefix, signal) {
  const r = await request(url, { method: 'POST', signal });
  if (!r.ok) toast(errPrefix + ': ' + (r.text || httpErrorMessage(r.status)), 'error');
  return r.ok;
}

export async function deleteAction(url, errPrefix, signal) {
  const r = await request(url, { method: 'DELETE', signal });
  if (!r.ok) toast(errPrefix + ': ' + (r.text || httpErrorMessage(r.status)), 'error');
  return r.ok;
}

// Format the same "HTTP <status>" message every hand-rolled site used, so
// custom error text stays consistent with the shared helpers.
export function httpErrorMessage(status) {
  return 'HTTP ' + status;
}
