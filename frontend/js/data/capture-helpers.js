// Capture helpers - constants, version comparison, fingerprint mapping, TOML serialization.

export const CAPTURE_STEPS = ['announce', 'handshake', 'ext'];
export const KEEPALIVE_DEFAULT = 90;

import { hasValue } from '../utils/form.js';

export function versionHint(cmp) {
  if (cmp > 0) return 'The captured version is <strong>newer</strong>.';
  if (cmp < 0) return 'The captured version is <strong>older</strong>.';
  return '';
}

// Read the fast-extension bit (reserved byte 7, bit 0x04) from a 16-char hex
// string. Returns null when `reservedHex` is empty (no handshake yet), true
// when the bit is set, false otherwise. Single source for the bit-twiddle that
// both fingerprintToClient and the capture-modal UI need.
export function fastExtensionBit(reservedHex) {
  if (!reservedHex) return null;
  return (parseInt(reservedHex.slice(14, 16), 16) & 0x04) !== 0;
}

export function compareVersions(a, b) {
  const pa = String(a).split('.').map(Number);
  const pb = String(b).split('.').map(Number);
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i++) {
    const va = pa[i] || 0, vb = pb[i] || 0;
    if (va > vb) return 1;
    if (va < vb) return -1;
  }
  return 0;
}

export function detectKeyFormat(rawQuery) {
  if (!rawQuery) return null;
  // `(?:^|[?&])` so `key=` matches whether it is first or after another param.
  const m = rawQuery.match(/(?:^|[?&])key=([A-Za-z0-9]+)/);
  if (!m) return null;
  const key = m[1];
  if (/[g-zG-Z]/.test(key)) return 'base62';
  // Uppercase A-F takes precedence: a lower_hex key never contains uppercase,
  // so any A-F letter proves upper_hex (a mixed-case key like "Ab1234" is
  // upper_hex, not lower_hex).
  if (/[A-F]/.test(key)) return 'upper_hex';
  if (/[a-f]/.test(key)) return 'lower_hex';
  return null;
}

export function reconstructCaptureQuery(rawQuery) {
  if (!rawQuery) return '';
  const dynamic = new Set(['info_hash', 'peer_id', 'port', 'uploaded', 'downloaded', 'left', 'key', 'numwant']);
  const strip = new Set(['ipv4', 'ipv6']);
  const parts = rawQuery.split('&').filter(pair => {
    const eq = pair.indexOf('=');
    const key = eq >= 0 ? pair.slice(0, eq) : pair;
    return !strip.has(key);
  }).map(pair => {
    const eq = pair.indexOf('=');
    const key = eq >= 0 ? pair.slice(0, eq) : pair;
    if (key === 'event') return '{event}';
    if (dynamic.has(key)) return key + '={' + key + '}';
    return pair;
  });
  let result = '';
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    const isEvent = part === '{event}';
    const prevWasEvent = i > 0 && parts[i - 1] === '{event}';
    if (isEvent) result += '{event}';
    else if (prevWasEvent) result += '&' + part;
    else if (result) result += '&' + part;
    else result = part;
  }
  return result;
}

export function fingerprintToClient(fp) {
  if (!fp) return null;
  const reserved = fp.reserved_bytes || '0000000000100005';
  const label = fp.label || 'Captured Client';
  const version = fp.version || 'unknown';
  const mDict = fp.m_dict || {};
  return {
    label,
    version,
    peer_id_prefix: fp.peer_id_prefix || '',
    user_agent: fp.user_agent || '',
    query: reconstructCaptureQuery(fp.raw_query),
    numwant: fp.numwant || 50,
    reserved_bytes: reserved,
    fast_extension: fastExtensionBit(reserved) ?? false,
    keepalive_secs: hasValue(fp.keepalive_secs) && fp.keepalive_secs > 0 ? fp.keepalive_secs : null,
    v_string: fp.v_string || fp.user_agent || '',
    m_dict: mDict,
    aliases: [],
    reqq: hasValue(fp.reqq) ? fp.reqq : null,
    encryption_preferred: hasValue(fp.encryption_preferred) ? fp.encryption_preferred : null,
    send_upload_only: hasValue(fp.upload_only) || hasValue(mDict.upload_only) ? true : false,
    send_complete_ago: hasValue(fp.complete_ago) ? fp.complete_ago : null,
    send_yourip: hasValue(fp.yourip) ? true : false,
    key_format: detectKeyFormat(fp.raw_query),
  };
}

export function clientToToml(c) {
  const lines = ['[[clients]]'];
  lines.push('label = "' + c.label + '"');
  lines.push('version = "' + c.version + '"');
  lines.push('peer_id_prefix = "' + c.peer_id_prefix + '"');
  lines.push('user_agent = "' + c.user_agent + '"');
  lines.push('query = "' + c.query + '"');
  lines.push('numwant = ' + c.numwant);
  lines.push('aliases = ' + (c.aliases.length === 0 ? '[]'
    : '[' + c.aliases.map(a => '"' + a + '"').join(', ') + ']'));
  lines.push('reserved_bytes = "' + c.reserved_bytes + '"');
  lines.push('fast_extension = ' + c.fast_extension);
  if (hasValue(c.keepalive_secs)) lines.push('keepalive_secs = ' + c.keepalive_secs);
  else lines.push('# keepalive_secs not measured');
  lines.push('v_string = "' + c.v_string + '"');
  if (hasValue(c.reqq)) lines.push('reqq = ' + c.reqq);
  if (hasValue(c.encryption_preferred)) lines.push('encryption_preferred = ' + c.encryption_preferred);
  lines.push('send_upload_only = ' + c.send_upload_only);
  if (hasValue(c.send_complete_ago)) lines.push('send_complete_ago = ' + c.send_complete_ago);
  lines.push('send_yourip = ' + c.send_yourip);
  if (hasValue(c.key_format)) lines.push('key_format = "' + c.key_format + '"');
  else lines.push('# key_format unknown (all-digit key - pick one: decimal, lower_hex, upper_hex)');
  const entries = Object.entries(c.m_dict);
  if (entries.length > 0) {
    lines.push('');
    lines.push('[clients.m_dict]');
    entries.forEach(([k, v]) => lines.push(k + ' = ' + v));
  }
  return lines.join('\n');
}
