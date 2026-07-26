// DOM helper utilities - escaping, focus management, segmented controls.

// Segmented-control aria-labels - the single source for these strings. The
// template ships matching aria-label attributes on the static markup; the JS
// references them only through these consts so a rename on one side is
// obvious. (The HTML can't import JS, so the template mirrors the literal.)
export const SEG_TRANSFER_MODE = 'Transfer mode';
export const SEG_SPEED_STRATEGY = 'Speed strategy';

export function escHtml(s) {
  const d = document.createElement('div');
  d.textContent = String(s == null ? '' : s);
  return d.innerHTML;
}

export function escAttr(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

export function focusFirst(root) {
  const f = root.querySelector('button:not(.modal-close), input, select, textarea');
  if (f) f.focus();
}

export function setSegmented(ariaLabel, value) {
  document.querySelectorAll(`.segmented[aria-label="${ariaLabel}"] button`).forEach(b => {
    const selected = b.dataset.value === value;
    b.classList.toggle('active', selected);
    b.setAttribute('aria-checked', selected);
    b.tabIndex = selected ? 0 : -1;
  });
}

// Wire a segmented control's buttons to an onChange handler by aria-label.
// Replaces the hand-written `.segmented[aria-label="…"] button` selector +
// addEventListener loop duplicated in init.js.
export function wireSegmented(ariaLabel, onChange) {
  document.querySelectorAll(`.segmented[aria-label="${ariaLabel}"] button`).forEach(btn => {
    btn.addEventListener('click', () => onChange(btn.dataset.value));
  });
}
