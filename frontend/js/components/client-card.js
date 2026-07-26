// Client card rendering + collapse/expand for the settings modal.
// Renders all 18 client fields across 4 sections: Identity, Tracker announce,
// Peer wire, BEP-10 ext handshake.

import { escHtml, escAttr } from '../utils/dom-helpers.js';
import { hasValue } from '../utils/form.js';
import { mDictToText, clientFieldId, clientDisplayName } from '../data/client-schema.js';

export function clientCardHtml(i, c) {
 const p = clientFieldId(i, '');
 const title = c.label ? clientDisplayName(c) : 'New client';
 const R = '<span class="req">*</span>';
 function field(label, id, input, hint) {
 return '<div class="field"><label for="' + id + '">' + label + '</label>' + input + '<div class="hint">' + hint + '</div></div>';
 }
 return '<div class="client-card collapsed" data-idx="' + i + '">'
 + '<div class="client-card-header" data-action="toggle-client-card"><div class="card-title"><span class="chevron">\u25BC</span>' + escHtml(title) + '</div>'
 + (i === 'cap' ? '' : '<button class="btn btn-danger" data-action="remove-client">Remove</button></div>')
 + (i === 'cap' ? '</div>' : '')
 + '<div class="client-card-body hidden">'
 // Identity
 + '<div class="client-section">'
 + '<div class="field-group-label">Identity</div>'
 + '<div class="grid2">'
 + field('Label' + R, p + 'label', '<input type="text" id="' + p + 'label" value="' + escAttr(c.label) + '">', 'Client name (e.g. qBittorrent, Transmission)')
 + field('Version' + R, p + 'version', '<input type="text" id="' + p + 'version" value="' + escAttr(c.version) + '">', 'Version string (e.g. 5.2.2)')
 + '</div>'
 + '<div class="grid2">'
 + field('Peer ID prefix' + R, p + 'peer_id_prefix', '<input type="text" id="' + p + 'peer_id_prefix" value="' + escAttr(c.peer_id_prefix) + '">', 'Azureus-style 8-char prefix (e.g. -qB5220-)')
 + field('User-Agent' + R, p + 'user_agent', '<input type="text" id="' + p + 'user_agent" value="' + escAttr(c.user_agent) + '">', 'HTTP User-Agent header')
 + '</div>'
 + field('v_string (BEP-10)' + R, p + 'v_string', '<input type="text" id="' + p + 'v_string" value="' + escAttr(c.v_string) + '">', 'Client name+version sent in LTEP extension handshake')
 + field('Aliases', p + 'aliases', '<textarea id="' + p + 'aliases" rows="2">' + escHtml((c.aliases || []).join('\n')) + '</textarea>', 'Alternative names for client matching (one per line)')
 + '</div>'
 // Tracker announce
 + '<div class="client-section">'
 + '<div class="field-group-label">Tracker announce</div>'
 + field('Query template' + R, p + 'query', '<textarea id="' + p + 'query" rows="3">' + escHtml(c.query) + '</textarea>', 'URL query params - must contain {info_hash} and {peer_id}')
 + '<div class="grid2">'
 + field('Numwant' + R, p + 'numwant', '<input type="number" id="' + p + 'numwant" min="1" step="1" value="' + escAttr(c.numwant) + '">', 'Peers requested per announce')
 + field('Key format' + R, p + 'key_format', '<select id="' + p + 'key_format"><option value="lower_hex"' + (c.key_format === 'lower_hex' ? ' selected' : '') + '>lower_hex</option><option value="upper_hex"' + (c.key_format === 'upper_hex' ? ' selected' : '') + '>upper_hex</option><option value="decimal"' + (c.key_format === 'decimal' ? ' selected' : '') + '>decimal</option></select>', 'Format of the key parameter')
 + '</div>'
 + '</div>'
 // Peer wire
 + '<div class="client-section">'
 + '<div class="field-group-label">Peer wire</div>'
 + '<div class="grid2">'
 + field('Reserved bytes (hex)' + R, p + 'reserved_bytes', '<input type="text" id="' + p + 'reserved_bytes" value="' + escAttr(c.reserved_bytes) + '">', '8 bytes as hex (16 chars) - sets capability bits')
 + field('Keepalive (s)' + R, p + 'keepalive_secs', '<input type="number" id="' + p + 'keepalive_secs" min="1" step="1" value="' + escAttr(c.keepalive_secs) + '">', 'Interval between keepalive messages')
 + '</div>'
 + '<div class="grid2">'
 + field('reqq', p + 'reqq', '<input type="number" id="' + p + 'reqq" min="1" step="1" value="' + escAttr(c.reqq) + '">', 'Max outstanding block requests (blank = omit)')
 + '<div class="field"><label class="switch-row" for="' + p + 'fast_extension"><span>Fast extension' + R + '</span><span class="switch"><input type="checkbox" id="' + p + 'fast_extension"' + (c.fast_extension ? ' checked' : '') + '><span class="track"><span class="thumb"></span></span></span></label><div class="hint">Must match reserved bytes bit 0x04</div></div>'
 + '</div>'
 + field('m_dict', p + 'm_dict', '<textarea id="' + p + 'm_dict" rows="3">' + escHtml(mDictToText(c.m_dict)) + '</textarea>', 'BEP-10 extension message IDs (key = value per line)')
 + '</div>'
 // BEP-10 ext handshake
 + '<div class="client-section">'
 + '<div class="field-group-label">BEP-10 ext handshake fields</div>'
 + '<div class="grid2">'
 + '<div class="field"><label class="switch-row" for="' + p + 'send_upload_only"><span>Send upload_only</span><span class="switch"><input type="checkbox" id="' + p + 'send_upload_only"' + (c.send_upload_only ? ' checked' : '') + '><span class="track"><span class="thumb"></span></span></span></label><div class="hint">BEP-21 flag for partial seeds</div></div>'
 + '<div class="field"><label class="switch-row" for="' + p + 'send_yourip"><span>Send yourip</span><span class="switch"><input type="checkbox" id="' + p + 'send_yourip"' + (c.send_yourip ? ' checked' : '') + '><span class="track"><span class="thumb"></span></span></span></label><div class="hint">Peer IP as seen by us</div></div>'
 + '</div>'
 + '<div class="grid2">'
 + field('Encryption (e)', p + 'encryption_preferred', '<select id="' + p + 'encryption_preferred"><option value=""' + (!hasValue(c.encryption_preferred) ? ' selected' : '') + '>not sent</option><option value="true"' + (c.encryption_preferred === true ? ' selected' : '') + '>true</option><option value="false"' + (c.encryption_preferred === false ? ' selected' : '') + '>false</option></select>', 'BEP-10 encryption preference')
 + field('complete_ago', p + 'send_complete_ago', '<input type="text" id="' + p + 'send_complete_ago" value="' + (hasValue(c.send_complete_ago) ? escAttr(c.send_complete_ago) : '') + '">', 'Seconds since completed (blank = omit, -1 = never)')
 + '</div>'
 + '</div>'
 + '</div>'
 + '</div>';
}

export function toggleClientCard(idx) {
 const card = document.querySelector('.client-card[data-idx="' + idx + '"]');
 if (!card) return;
 const body = card.querySelector('.client-card-body');
 card.classList.toggle('collapsed');
 card.classList.toggle('expanded');
 if (body) body.classList.toggle('hidden');
}

// Force a card into the expanded state (no-op if already expanded).
// The card starts collapsed with a hidden body; this is the inverse.
export function expandClientCard(card) {
 if (!card) return;
 card.classList.remove('collapsed');
 card.classList.add('expanded');
 const body = card.querySelector('.client-card-body');
 if (body) body.classList.remove('hidden');
}
