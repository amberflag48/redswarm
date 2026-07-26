// Client field schema - single source of truth for all 18 client fields.
// Drives defaultClient, collectOneClient, clientCardHtml, and fingerprintToClient.

export const CLIENT_FIELDS = [
 // Identity
 { key: 'label', section: 'identity', type: 'text', req: true, default: '', fallback: '' },
 { key: 'version', section: 'identity', type: 'text', req: true, default: '', fallback: '' },
 { key: 'peer_id_prefix', section: 'identity', type: 'text', req: true, default: '', fallback: '' },
 { key: 'user_agent', section: 'identity', type: 'text', req: true, default: '', fallback: '' },
 { key: 'v_string', section: 'identity', type: 'text', req: true, default: '', fallback: '' },
 { key: 'aliases', section: 'identity', type: 'aliases', req: false, default: [], fallback: null },
 // Tracker announce
 { key: 'query', section: 'tracker', type: 'textarea', req: true, default: '', fallback: '' },
 { key: 'numwant', section: 'tracker', type: 'number', req: true, default: '', fallback: 200 },
 { key: 'key_format', section: 'tracker', type: 'select', req: true, default: 'upper_hex', fallback: 'upper_hex', options: ['lower_hex', 'upper_hex', 'decimal'] },
 // Peer wire
 { key: 'reserved_bytes', section: 'peer_wire', type: 'text', req: true, default: '', fallback: '' },
 { key: 'keepalive_secs', section: 'peer_wire', type: 'number', req: true, default: '', fallback: 90 },
 { key: 'reqq', section: 'peer_wire', type: 'number', req: false, default: '', fallback: null },
 { key: 'fast_extension', section: 'peer_wire', type: 'bool', req: true, default: false, fallback: false },
 { key: 'm_dict', section: 'peer_wire', type: 'm_dict', req: false, default: {}, fallback: null },
 // BEP-10 ext handshake
 { key: 'send_upload_only', section: 'bep10', type: 'bool', req: false, default: false, fallback: false },
 { key: 'send_yourip', section: 'bep10', type: 'bool', req: false, default: false, fallback: false },
 { key: 'encryption_preferred', section: 'bep10', type: 'bool_or_null', req: false, default: null, fallback: null },
 { key: 'send_complete_ago', section: 'bep10', type: 'int_or_null', req: false, default: null, fallback: null },
];

export function clientFieldId(idx, key) {
 return 'set-client-' + idx + '-' + key;
}

// Build the display name for a client config object. Mirrors Rust
// `ClientSpecConfig::display_name()` - `"{label} - {version} ({peer_id_prefix})"`
// with raw values (no fallbacks) so it matches the server's format exactly.
// Used by the SSE config_reloaded handler to build dropdown pairs, the
// settings error matcher, and client-card titles.
export function clientDisplayName(c) {
 return c.label + ' - ' + c.version + ' (' + c.peer_id_prefix + ')';
}

// Build the short "label version" form (space separator, no prefix). Used
// by capture-modal confirm dialogs where the peer_id_prefix is shown
// separately in a <code> tag.
export function clientLabelVersion(c) {
 return c.label + ' ' + c.version;
}

export function defaultClient() {
 const c = {};
 CLIENT_FIELDS.forEach(f => { c[f.key] = f.default; });
 return c;
}

export function collectOneClient(idx) {
 const card = document.querySelector(`.client-card[data-idx="${idx}"]`);
 if (!card) return null;
 const c = {};
 CLIENT_FIELDS.forEach(f => {
 const id = clientFieldId(idx, f.key);
 if (f.type === 'bool') {
 const el = document.getElementById(id);
 c[f.key] = el ? el.checked : false;
 } else if (f.type === 'number') {
 const el = document.getElementById(id);
 if (!el || el.value === '') c[f.key] = f.fallback;
 else { const n = Number(el.value); c[f.key] = isNaN(n) ? f.fallback : n; }
 } else if (f.type === 'select') {
 const el = document.getElementById(id);
 c[f.key] = el ? el.value : f.fallback;
 } else if (f.type === 'bool_or_null') {
 const el = document.getElementById(id);
 if (!el || el.value === '') c[f.key] = null;
 else c[f.key] = el.value === 'true';
 } else if (f.type === 'int_or_null') {
 const el = document.getElementById(id);
 if (!el || el.value === '') c[f.key] = null;
 else { const n = parseInt(el.value, 10); c[f.key] = isNaN(n) ? null : n; }
 } else if (f.type === 'aliases') {
 const el = document.getElementById(id);
 c[f.key] = (el ? el.value : '').split('\n').map(s => s.trim()).filter(s => s);
 } else if (f.type === 'm_dict') {
 const el = document.getElementById(id);
 c[f.key] = parseMDict(el ? el.value : '');
 } else {
 const el = document.getElementById(id);
 c[f.key] = el ? el.value : (f.fallback || '');
 }
 });
 return c;
}

export function parseMDict(text) {
 const m = {};
 text.split('\n').forEach(line => {
 line = line.trim();
 if (!line) return;
 const eq = line.indexOf('=');
 if (eq === -1) return;
 const k = line.slice(0, eq).trim();
 const v = parseInt(line.slice(eq + 1).trim(), 10);
 if (k && !isNaN(v)) m[k] = v;
 });
 return m;
}

export function mDictToText(m) {
 return Object.entries(m || {}).map(e => e[0] + ' = ' + e[1]).join('\n');
}
