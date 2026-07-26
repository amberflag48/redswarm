// Client dropdown service - populates #cfg-client from /api/clients.

import { state } from '../state/store.js';
import { fetchJson } from '../utils/net.js';

// The single "auto" option label. The template ships a matching static
// <option>; refreshClientDropdown re-creates it on every refresh.
const AUTO_CLIENT_LABEL = 'Auto (probe all)';

// Rebuild the #cfg-client dropdown + state.clientMap.
//
// Pass `pairs` to populate from already-fetched data (the page bootstrap) and
// skip the /api/clients round-trip. Omit it to fetch /api/clients live (used by
// the config_reloaded / clients-changed paths). In both cases the previously
// selected value is preserved when it still exists.
export function refreshClientDropdown(pairs) {
  const sel = document.getElementById('cfg-client');
  if (!sel) return Promise.resolve();
  const cur = sel.value;
  const populate = list => {
    if (!Array.isArray(list)) return;
    state.clientMap = {};
    while (sel.firstChild) sel.removeChild(sel.firstChild);
    const auto = document.createElement('option');
    auto.value = '';
    auto.textContent = AUTO_CLIENT_LABEL;
    sel.appendChild(auto);
    for (const [value, display] of list) {
      state.clientMap[value] = display;
      const o = document.createElement('option');
      o.value = value;
      o.textContent = display;
      sel.appendChild(o);
    }
    if (list.some(p => p[0] === cur)) sel.value = cur;
  };
  if (pairs) { populate(pairs); return Promise.resolve(); }
  return fetchJson('/api/clients', null, 'Client list refresh failed').then(populate);
}
