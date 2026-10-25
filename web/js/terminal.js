// Full-screen terminal: a <kimo-terminal> (the SDK element) plus a key bar and a compose box for phones.
import '../sdk/kimo-terminal.js';
import { kimo } from './api.js';

const el = id => document.getElementById(id);
const KEYS = { esc: '\x1b', tab: '\t', ctrlc: '\x03', up: '\x1b[A', down: '\x1b[B', right: '\x1b[C', left: '\x1b[D', enter: '\r' };
const screen = document.createElement('kimo-terminal');
screen.style.height = '100%';
el('term-screen').append(screen);
let current = null;

export const terminal = {
  onClose: null,
  onToggle: null,
  get current() { return current; },

  open(session) {
    current = session;
    el('term').hidden = false;
    document.body.style.overflow = 'hidden';
    screen.kimo = kimo;
    screen.setAttribute('session', session.id);
    terminal.update(session);
    if (!matchMedia('(pointer: coarse)').matches) setTimeout(() => screen.focus(), 300);
  },

  close() {
    current = null;
    screen.removeAttribute('session');
    el('term').hidden = true;
    document.body.style.overflow = '';
  },

  update(session) {
    if (!current || session.id !== current.id) return;
    current = session;
    el('term-name').textContent = session.name;
    const badge = el('term-status');
    badge.className = `ui-badge ${session.status}`;
    badge.textContent = session.status;
    const pause = el('term-pause');
    pause.hidden = !['running', 'paused'].includes(session.status);
    pause.textContent = session.status === 'paused' ? 'resume' : 'pause';
  },
};

el('term-back').addEventListener('click', () => { terminal.close(); terminal.onClose?.(); });
el('term-pause').addEventListener('click', () => terminal.onToggle?.(current));
el('keys').addEventListener('click', e => {
  const key = e.target.closest('button')?.dataset.key;
  if (key) screen.send(KEYS[key]);
});
const compose = el('compose');
compose.addEventListener('submit', e => {
  e.preventDefault();
  const box = compose.elements.text;
  // Text first, then Enter separately so TUIs treat it as typed input rather than a paste.
  screen.send(box.value);
  setTimeout(() => screen.send('\r'), 30);
  box.value = '';
});
compose.elements.text.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); compose.requestSubmit(); }
});
