// Cached DOM references - populated by cacheDom() on init.
export const dom = {};

export function cacheDom() {
  const ids = [
    'modal', 'modal-title', 'meta-preview', 'config-section', 'start-audit-btn',
    'file-input', 'magnet-input', 'audit-list', 'goal-list', 'log-panel', 'topbar-stats',
    'dropzone', 'tab-file', 'tab-magnet', 'input-file', 'input-magnet',
    'torrent-input-section', 'dynamic-fields',
    'upload-speed-field', 'download-speed-field', 'start-pct-field',
  ];
  ids.forEach(id => {
    const key = id.replace(/-/g, '_');
    dom[key] = document.getElementById(id);
  });
}

// Shared DOM lookup - used by task-list.js, log-panel.js, and others.
// Lives here to avoid circular imports between component modules.
export function taskRow(id) {
  return dom.audit_list.querySelector('tr[data-id="' + id + '"]');
}
