// Form utilities - dirty detection, clamping, validation.

export function clamp(v, lo, hi) { return Math.min(Math.max(v, lo), hi); }

export function hasValue(v) { return v !== null && v !== undefined; }

export function snapshotForm(container) {
  const parts = [];
  container.querySelectorAll('input, select, textarea').forEach(el => {
    if (!el.id) return;
    if (el.type === 'checkbox') {
      parts.push(el.id + '=' + (el.checked ? '1' : '0'));
    } else {
      parts.push(el.id + '=' + el.value);
    }
  });
  parts.sort();
  return parts.join('|');
}

export function isFormDirty(container, snapshot) {
  if (!snapshot) return false;
  return snapshotForm(container) !== snapshot;
}

export function addFieldError(fieldEl, message) {
  fieldEl.classList.add('error');
  const errDiv = document.createElement('div');
  errDiv.className = 'field-error';
  errDiv.textContent = message;
  const hint = fieldEl.querySelector('.hint');
  if (hint) hint.parentNode.insertBefore(errDiv, hint);
  else fieldEl.appendChild(errDiv);
}

// Clear every `.field.error` and `.field-error` in `scope` (defaults to the
// whole document). The natural pair of `addFieldError` - kept here so both
// halves of the error-lifecycle live in one module.
export function clearFieldErrors(scope) {
  const root = scope || document;
  root.querySelectorAll('.field.error').forEach(f => f.classList.remove('error'));
  root.querySelectorAll('.field-error').forEach(e => e.remove());
}

export function clampNumberOnBlur(e) {
  const el = e.target;
  if (el.type !== 'number' || el.min === '' || el.max === '') return;
  const v = parseFloat(el.value);
  if (isNaN(v)) return;
  el.value = clamp(v, parseFloat(el.min), parseFloat(el.max));
}
