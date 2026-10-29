// Small UI helpers shared by the views.
export const $ = id => document.getElementById(id);
export const esc = v => String(v ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
export const mb = bytes => bytes >= 1 << 30 ? `${(bytes / (1 << 30)).toFixed(1)} GB` : `${Math.round(bytes / (1 << 20))} MB`;
/** `~/…` for paths in a home folder (Linux `/home/x`, macOS `/Users/x`). */
export const home = path => path.replace(/^\/(home|Users)\/[^/]+/, '~');

export function toast(message, isError = false) {
  let el = document.querySelector('.ui-toast');
  if (!el) { el = document.createElement('div'); el.className = 'ui-toast'; el.setAttribute('role', 'status'); document.body.append(el); }
  el.textContent = message;
  el.classList.toggle('error', isError);
  el.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { el.hidden = true; }, 2800);
}

/** `~/…` path, shortened to its last folders when long (phones). */
export function shortPath(path, max = 36) {
  const p = home(path);
  if (p.length <= max) return p;
  const parts = p.split('/');
  let out = parts.pop();
  while (parts.length && parts.at(-1).length + out.length + 3 <= max) out = `${parts.pop()}/${out}`;
  return `…/${out}`;
}
