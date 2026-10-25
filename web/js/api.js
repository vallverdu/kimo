// kimo's own page uses the same SDK as any other app. The owner token comes from the link
// kimo prints (#token=…) and is kept in localStorage.
import { Kimo } from '../sdk/kimo.js';

const KEY = 'kimo-token';
const stored = () => { try { return localStorage.getItem(KEY) || ''; } catch { return ''; } };
const fromLink = location.hash.match(/token=([\w-]+)/)?.[1];
if (fromLink) {
  try { localStorage.setItem(KEY, fromLink); } catch { /* private mode: token lives for this page only */ }
  history.replaceState(null, '', location.pathname);
}

/** Pairing code from a scanned QR (`#pair=…`), if this page was opened from one. */
export const pairCode = location.hash.match(/pair=([\w-]+)/)?.[1] || null;
if (pairCode) history.replaceState(null, '', location.pathname);

export const kimo = new Kimo({ url: location.origin, token: fromLink || stored() });

export function setToken(token) {
  kimo.token = token;
  try { localStorage.setItem(KEY, token); } catch { /* ignore */ }
}
