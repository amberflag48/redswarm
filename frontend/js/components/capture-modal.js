// Capture modal - live client fingerprinting via a server-driven capture session.
// startCapture() POSTs /api/capture/start, auto-downloads the .torrent, and
// listens for capture_progress SSE events. The captured fingerprint is
// rendered as an editable client card (showCaptureSnippet) and can be copied
// as TOML or added to config.toml (addCapturedClient) with conflict checks.

import { state } from '../state/store.js';
import { snapshotForm, isFormDirty, hasValue } from '../utils/form.js';
import { putJson, fetchRaw, httpErrorMessage, clearPending, pendingSignal } from '../utils/net.js';
import { escHtml } from '../utils/dom-helpers.js';
import { btnLoading, btnReset } from '../utils/buttons.js';
import { openModalEl, registerModal } from './modal.js';
import { showConfirm, CONFIRM_CANCEL } from './confirm.js';
import { clientCardHtml, expandClientCard } from './client-card.js';
import { collectOneClient, clientLabelVersion } from '../data/client-schema.js';
import { CAPTURE_STEPS, KEEPALIVE_DEFAULT, versionHint, detectKeyFormat, compareVersions, fingerprintToClient, clientToToml, fastExtensionBit } from '../data/capture-helpers.js';
import { toast } from './toast.js';

const NO_FINGERPRINT_MSG = 'No fingerprint captured yet';

function captureIsDirty() {
    const card = document.getElementById('capture-client-card');
    return isFormDirty(card, state.captureFormSnapshot);
}

// Delete the server-side capture session and cancel any in-flight start
// fetch. Fire-and-forget the DELETE: the session is being torn down
// regardless, and a race with a completed capture would otherwise toast a
// spurious error.
function abortCapture() {
    clearPending('capture');
    if (state.captureToken) {
        fetchRaw('/api/capture/' + state.captureToken, { method: 'DELETE' }).catch(() => {});
        state.captureToken = null;
    }
}

// Reset all capture UI elements to their initial state.
function resetCaptureUI() {
    state.capturedFingerprint = null;
    state.captureFormSnapshot = '';
    document.getElementById('capture-fields-raw').innerHTML = '';
    document.getElementById('capture-client-card').innerHTML = '';
    document.getElementById('capture-snippet-section').classList.add('hidden');
    document.getElementById('capture-progress-section').classList.add('hidden');
    document.getElementById('capture-download-section').classList.add('hidden');
    document.getElementById('capture-loading-section').classList.remove('hidden');
    CAPTURE_STEPS.forEach(function(name) {
        const el = document.getElementById('cap-step-' + name);
        if (el) el.classList.remove('active', 'done');
    });
}

// Tear down the capture session + reset UI. Shared close/open/new reset path.
function resetCaptureModal() {
    abortCapture();
    resetCaptureUI();
}

export const closeCaptureModal = registerModal({ isDirty: captureIsDirty, noun: 'edits', reset: resetCaptureModal });

export function openCaptureModal() {
    resetCaptureModal();
    openModalEl(document.getElementById('capture-modal'), closeCaptureModal);
    startCapture();
}

export function newCapture() {
    resetCaptureModal();
    startCapture();
}

async function startCapture() {
    abortCapture();
    const signal = pendingSignal('capture');
    try {
        const resp = await fetchRaw('/api/capture/start', { method: 'POST', signal });
        if (!resp.ok) throw new Error(httpErrorMessage(resp.status));
        state.captureToken = resp.headers.get('X-Capture-Token');
        // Extract filename from Content-Disposition header
        const cd = resp.headers.get('Content-Disposition') || '';
        const fnameMatch = cd.match(/filename="?([^"]+)"?/);
        const filename = fnameMatch ? fnameMatch[1] : 'capture.torrent';
        const blob = await resp.blob();
        // Auto-download: create a temporary <a> and click it
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = filename;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
        // Also set the "Download again" link
        const dl = document.getElementById('capture-download-link');
        dl.href = url;
        dl.download = filename;
        document.getElementById('capture-loading-section').classList.add('hidden');
        document.getElementById('capture-download-section').classList.remove('hidden');
        document.getElementById('capture-progress-section').classList.remove('hidden');
        // Progress updates arrive via the global SSE `capture_progress` event -
        // no polling. The handler is in connectGlobalSSE.
    } catch (e) {
        // An abort means the user closed/newed the modal - not a real error.
        if (signal.aborted) return;
        toast('Failed to start capture: ' + (e.message || e), 'error');
        closeCaptureModal();
    }
}

export function updateCaptureUI(data) {
    const fp = data.fingerprint || {};
    const s = data.status;
    const steps = CAPTURE_STEPS;
    const order = ['waiting_for_announce', 'announce_captured', 'handshake_captured', 'ext_handshake_captured'];
    const idx = order.indexOf(s);
    steps.forEach((name, i) => {
        const el = document.getElementById('cap-step-' + name);
        el.classList.remove('active', 'done');
        if (idx > i) el.classList.add('done');
        else if (idx === i) el.classList.add('active');
    });
    const reservedHex = fp.reserved_bytes || null;
    const fastExt = fastExtensionBit(reservedHex);
    // After ext handshake is captured, fields that are still null mean the
    // client omitted them - show "not sent" instead of "waiting...".
    // Exception: keepalive_secs needs two keepalives (~2min), so show "measuring...".
    const extDone = s === 'ext_handshake_captured';
    const fmt = (v, suffix) => {
        if (hasValue(v)) return suffix ? v + suffix : String(v);
        if (extDone) return 'not sent';
        return null; // still waiting
    };
    const fmtKeepalive = (v) => {
        if (!hasValue(v)) return extDone ? 'measuring...' : null;
        if (v === 0) return 'not measured (connection too short)';
        return v + 's (measured)';
    };
    const fmtCompleteAgo = (v) => {
        if (!hasValue(v)) return extDone ? 'not sent' : null;
        if (v < 0) return 'not completed';
        return v + 's';
    };
    const fields = [
        ['peer_id_prefix', fp.peer_id_prefix],
        ['user_agent', fp.user_agent],
        ['numwant', fp.numwant],
        ['reserved_bytes', reservedHex],
        ['fast_extension', fastExt !== null ? String(fastExt) : null],
        ['v_string', fp.v_string],
        ['m_dict', fp.m_dict ? Object.entries(fp.m_dict).map(([k,v]) => k + '=' + v).join(', ') : null],
        ['reqq', fp.reqq],
        ['encryption', fmt(fp.encryption_preferred)],
        ['upload_only', fmt(fp.upload_only)],
        ['complete_ago', fmtCompleteAgo(fp.complete_ago)],
        ['yourip', fmt(fp.yourip)],
        ['listen_port', fmt(fp.listen_port)],
        ['metadata_size', fmt(fp.metadata_size)],
        ['ipv4', fmt(fp.ipv4)],
        ['ipv6', fmt(fp.ipv6)],
        ['share_mode', fmt(fp.share_mode)],
        ['key_format', fp.raw_query ? (detectKeyFormat(fp.raw_query) || 'unknown (all-digit key)') : null],
        ['keepalive_secs', fmtKeepalive(fp.keepalive_secs)],
        ['message_order', fp.message_order ? fp.message_order.join(' → ') : null],
        ['query_params', fp.query_param_order ? fp.query_param_order.join(' → ') : null],
        ['http_headers', fp.http_headers ? fp.http_headers.join('\n') : null],
    ];
    // Full raw fields in collapsible details (collapsed by default)
    document.getElementById('capture-fields-raw').innerHTML = fields.map(([k, v]) =>
        '<div class="capture-field"><span class="key">' + escHtml(k) + '</span><span class="val ' + (!hasValue(v) ? 'pending' : (v === 'not sent' ? 'pending' : '')) + '">' + escHtml(v ?? 'waiting...') + '</span></div>'
    ).join('');
}

export function showCaptureSnippet(fp) {
    if (!fp) return;
    state.capturedFingerprint = fp;
    const client = fingerprintToClient(fp);
    if (!client) return;
    if (!hasValue(client.keepalive_secs)) client.keepalive_secs = KEEPALIVE_DEFAULT;
    state.captureFormSnapshot = '';
    const html = clientCardHtml('cap', client);
    const container = document.getElementById('capture-client-card');
    container.innerHTML = html;
    // Expand the card
    const card = container.querySelector('.client-card');
    if (card) expandClientCard(card);
    // Snapshot form state after render - for exact dirty detection
    state.captureFormSnapshot = snapshotForm(container);
    document.getElementById('capture-snippet-section').classList.remove('hidden');
}

export function copyCaptureSnippet() {
    const client = collectOneClient('cap');
    if (!client) { toast(NO_FINGERPRINT_MSG, 'error'); return; }
    const text = clientToToml(client);
    // Always use execCommand fallback - navigator.clipboard requires a secure
    // context (HTTPS or localhost), which fails when accessing via LAN IP.
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.top = '50%';
    ta.style.left = '50%';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    let ok = false;
    try { ok = document.execCommand('copy'); } catch (e) {}
    document.body.removeChild(ta);
    if (ok) toast('TOML copied to clipboard', 'success');
    else toast('Copy failed - select the text manually', 'error');
}

// Add the captured client to config.toml.
// Two sequential checks:
//   1. Same peer_id_prefix? → replace or abort only (same ID can't coexist)
//   2. Same label? → replace, add anyway, or abort (different IDs can coexist)
export async function addCapturedClient() {
    const client = collectOneClient('cap');
    if (!client) { toast(NO_FINGERPRINT_MSG, 'error'); return; }
    if (!hasValue(client.keepalive_secs)) client.keepalive_secs = KEEPALIVE_DEFAULT;
    if (!hasValue(client.key_format)) { delete client.key_format; }
    const btn = document.getElementById('capture-add-client-btn');
    if (btn) { btnLoading(btn); }

    try {
        if (!state.settingsConfig) { btnReset(btn); return; }
        // Use the cached config from the store (kept fresh by the
        // config_reloaded SSE handler) instead of a redundant round-trip.
        const cfg = JSON.parse(JSON.stringify(state.settingsConfig));

        // Check 1: same peer_id_prefix (the unique identity key)
        const prefixIdx = cfg.clients.findIndex(c => c.peer_id_prefix === client.peer_id_prefix);
        if (prefixIdx >= 0) {
            const existing = cfg.clients[prefixIdx];
            const sameVersion = existing.version === client.version;
            let confirmed;
            if (sameVersion) {
                confirmed = await showConfirm(
                    'Client already exists',
                    'Client <strong>' + escHtml(clientLabelVersion(existing)) + '</strong> (<code>' + escHtml(client.peer_id_prefix) + '</code>) already exists. Overwrite it?',
                    [CONFIRM_CANCEL, { label: 'Overwrite', class: 'btn-danger', value: 'ok' }]
                );
            } else {
                const cmp = compareVersions(client.version, existing.version);
                const hint = versionHint(cmp);
                confirmed = await showConfirm(
                    'Same client ID, different version',
                    'Existing: <strong>' + escHtml(clientLabelVersion(existing)) + '</strong><br>Captured: <strong>' + escHtml(clientLabelVersion(client)) + '</strong><br>Both use <code>' + escHtml(client.peer_id_prefix) + '</code>.<br>' + escHtml(hint) + ' Replace the existing version?',
                    [CONFIRM_CANCEL, { label: 'Replace', class: 'btn-danger', value: 'ok' }]
                );
            }
            if (confirmed !== 'ok') { return; }
            cfg.clients[prefixIdx] = client;
        } else {
            // Check 2: same label (different ID - can coexist)
            const labelIdx = cfg.clients.findIndex(c => c.label === client.label);
            if (labelIdx >= 0) {
                const existingLbl = cfg.clients[labelIdx];
                const cmp = compareVersions(client.version, existingLbl.version);
                const hint = versionHint(cmp);
                const replace = await showConfirm(
                    'Same client, different version',
                    'Existing: <strong>' + escHtml(clientLabelVersion(existingLbl)) + '</strong> (<code>' + escHtml(existingLbl.peer_id_prefix) + '</code>)<br>Captured: <strong>' + escHtml(clientLabelVersion(client)) + '</strong> (<code>' + escHtml(client.peer_id_prefix) + '</code>)<br>' + escHtml(hint),
                    [
                        CONFIRM_CANCEL,
                        { label: 'Add anyway', class: 'btn-warning', value: 'add' },
                        { label: 'Replace', class: 'btn-danger', value: 'ok' }
                    ]
                );
                if (replace === 'ok') {
                    cfg.clients[labelIdx] = client;
                } else if (replace === 'add') {
                    cfg.clients.unshift(client);
                } else {
                    return;
                }
            } else {
                cfg.clients.unshift(client);
            }
        }

        state.settingsSaveAt = Date.now();
        const putResult = await putJson('/api/settings', cfg, 'Failed to add client');
        if (!putResult) { state.settingsSaveAt = 0; btnReset(btn); return; }
        toast('Client added', 'success');
        state.captureFormSnapshot = '';
        closeCaptureModal();
    } catch (e) {
        toast('Failed to add client: ' + (e.message || e), 'error');
        state.settingsSaveAt = 0;
    } finally {
        if (btn) { btnReset(btn); }
    }
}
