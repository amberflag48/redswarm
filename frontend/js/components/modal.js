// Modal framework - shared open/close with focus trapping and scroll lock.
// modalStack tracks nested modals (e.g. settings → confirm).

import { confirmDiscardIfDirty } from './confirm.js';

let scrollLockDepth = 0;
const modalStack = [];

function lockScroll() {
  if (scrollLockDepth++ === 0) {
    const sb = window.innerWidth - document.documentElement.clientWidth;
    document.body.style.overflow = 'hidden';
    if (sb > 0) document.body.style.paddingRight = sb + 'px';
  }
}

function unlockScroll() {
  if (--scrollLockDepth <= 0) {
    scrollLockDepth = 0;
    document.body.style.overflow = '';
    document.body.style.paddingRight = '';
  }
}

export function openModalEl(el, closeFn) {
  const trigger = document.activeElement;
  const handler = e => trapFocus(el, e, closeFn);
  el.addEventListener('keydown', handler);
  el.classList.remove('modal-closed');
  lockScroll();
  modalStack.push({ el, handler, closeFn, trigger });
}

export function closeModalEl() {
  const entry = modalStack.pop();
  if (!entry) return;
  entry.el.classList.add('modal-closed');
  entry.el.removeEventListener('keydown', entry.handler);
  unlockScroll();
  if (entry.trigger) entry.trigger.focus();
}

function trapFocus(overlay, e, closeFn) {
  if (e.key === 'Escape') {
    e.preventDefault();
    e.stopPropagation();
    if (closeFn) closeFn();
    return;
  }
  if (e.key !== 'Tab') return;
  const focusables = overlay.querySelectorAll(
    'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'
  );
  if (focusables.length === 0) return;
  const first = focusables[0], last = focusables[focusables.length - 1];
  if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
}

// Shared modal-close factory: confirm-discard-if-dirty → closeModalEl → reset.
// Every content modal (task/settings/capture) had the same bespoke shape; this
// collapses them into one. Returns `true` if closed, `false` if the user kept
// the modal open (cancelled the discard prompt).
export function registerModal({ isDirty, noun, reset }) {
  return async function close() {
    if (isDirty && !(await confirmDiscardIfDirty(isDirty, noun))) return false;
    closeModalEl();
    if (reset) reset();
    return true;
  };
}

// Shared modal-close wiring: overlay backdrop click, .modal-close button, and
// .btn-row .btn-secondary (Cancel) all call `closeFn`. Every modal in init.js
// had the same 3-line boilerplate; this collapses them into one call.
export function wireModalClose(modalEl, closeFn) {
  modalEl.addEventListener('click', e => { if (e.target === e.currentTarget) closeFn(); });
  modalEl.querySelector('.modal-close').addEventListener('click', closeFn);
  modalEl.querySelector('.btn-row .btn-secondary').addEventListener('click', closeFn);
}
