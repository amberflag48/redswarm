// Confirm dialog - single shared instance, customizable buttons.
// Returns the value string of the clicked button, or 'cancel' if dismissed.
//
// Built on the shared modal framework (openModalEl/closeModalEl) so a nested
// confirm (e.g. settings → "discard changes?") gets the same focus trap,
// scroll-lock depth counting, and Escape handling as every other modal.

import { openModalEl, closeModalEl } from './modal.js';
import { escHtml, escAttr } from '../utils/dom-helpers.js';

let confirmResolve = null;

export const CONFIRM_CANCEL = { label: 'Cancel', class: 'btn-secondary', value: 'cancel' };

export function showConfirm(title, message, buttons) {
  return new Promise(resolve => {
    confirmResolve = resolve;
    const modal = document.getElementById('confirm-modal');
    document.getElementById('confirm-title').textContent = title;
    document.getElementById('confirm-message').innerHTML = message;

    const btns = buttons || [CONFIRM_CANCEL, { label: 'OK', value: 'ok' }];
    const container = document.getElementById('confirm-buttons');
    container.innerHTML = btns.map(b => {
      const cls = b.class ? ' ' + b.class : '';
      return `<button class="btn${cls}" data-value="${escAttr(b.value)}">${escHtml(b.label)}</button>`;
    }).join('');
    container.addEventListener('click', e => {
      const btn = e.target.closest('button');
      if (btn) resolveConfirm(btn.dataset.value);
    });

    // Escape / backdrop-close routes through the shared closeFn → 'cancel'.
    openModalEl(modal, () => resolveConfirm('cancel'));
    const lastBtn = container.querySelector('button:last-child');
    if (lastBtn) lastBtn.focus();
  });
}

export function resolveConfirm(result) {
  closeModalEl();
  if (confirmResolve) { confirmResolve(result); confirmResolve = null; }
}

export async function confirmDiscardIfDirty(isDirtyFn, noun) {
  if (!isDirtyFn()) return true;
  const ok = await showConfirm(
    'Unsaved changes',
    `You have <strong>unsaved ${noun}</strong>. Discard them and close?`,
    [CONFIRM_CANCEL, { label: 'Discard & close', class: 'btn-danger', value: 'ok' }]
  );
  return ok === 'ok';
}
