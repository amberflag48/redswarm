// Toast notifications - shared across the entire app.

const TOAST_CONTAINER_ID = 'toast-container';

export function toast(msg, type) {
  const el = document.createElement('div');
  el.className = 'toast' + (type ? ' ' + type : '');
  if (type === 'error') el.setAttribute('role', 'alert');
  el.textContent = msg;
  const container = document.getElementById(TOAST_CONTAINER_ID);
  if (!container) return;
  container.appendChild(el);
  const duration = type === 'error' ? 6000 : 4000;
  let timer = setTimeout(() => removeToast(el), duration);
  el.addEventListener('mouseenter', () => clearTimeout(timer));
  el.addEventListener('mouseleave', () => { timer = setTimeout(() => removeToast(el), duration); });
}

function removeToast(el) {
  el.style.opacity = '0';
  setTimeout(() => el.remove(), 200);
}
