// Button loading state helpers.

export function btnLoading(btn) {
  if (btn) { btn.disabled = true; btn.classList.add('loading'); }
}

export function btnReset(btn) {
  if (btn) { btn.disabled = false; btn.classList.remove('loading'); }
}
