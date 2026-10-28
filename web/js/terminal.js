// Full-screen terminal: a <kimo-terminal> (the SDK element) plus a key bar and a compose box for phones.
import '../sdk/kimo-terminal.js';
import { kimo } from './api.js';
import { toast } from './ui.js';

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
    // Agents kimo stopped to free memory continue as soon as they are opened.
    if (session.freed && session.status === 'stopped') kimo.act(session.id, 'start').catch(() => {});
    current = session;
    el('term').hidden = false;
    document.body.style.overflow = 'hidden';
    screen.kimo = kimo;
    screen.setAttribute('session', session.id);
    terminal.update(session);
    if (!matchMedia('(pointer: coarse)').matches) setTimeout(() => screen.focus(), 300);
  },

  close() {
    clearAttachments();
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
  const ready = attachments.filter(a => a.path);
  if (attachments.some(a => !a.path && !a.error)) return toast('Still sending the image…');
  if (!box.value.trim() && !ready.length) return;
  // Image paths first (the agent attaches them), then the words.
  const text = [...ready.map(a => quote(a.path)), box.value].filter(Boolean).join(' ');
  // Not connected: nothing is sent and nothing is lost (the draft and images stay).
  if (!screen.send(text)) return toast('Not connected: your message is kept. Send it again once the terminal is back.', true);
  box.value = '';
  clearAttachments();
  // Text first, then Enter separately so TUIs treat it as typed input rather than a paste.
  // Enter is never sent again by itself: the text may have arrived.
  setTimeout(() => {
    if (!screen.send('\r')) toast('The message was typed but Enter did not arrive: press ⏎ once the terminal is back.', true);
  }, 30);
});

// While disconnected, sending waits (the draft is kept) and the box says so.
const sendButton = compose.querySelector('button:not([type=button])');
function showConnection(connected) {
  sendButton.disabled = !connected;
  compose.elements.text.placeholder = connected ? 'Message the agent…' : 'Reconnecting… your message is kept';
}
showConnection(false);
screen.addEventListener('open', () => showConnection(true));
screen.addEventListener('close', () => showConnection(false));

// ----- photos and images for the agent -----

let attachments = [];
const quote = p => /^[\w./~@%+=:,-]+$/.test(p) ? p : `'${p.replace(/'/g, `'\\''`)}'`;

/** Photos are large: send at most 2000 px, as JPEG, unless the file is already small. */
async function shrink(file) {
  if (file.size < 1.5e6 || !/^image\/(jpeg|png|webp|heic|heif)$/.test(file.type)) return file;
  try {
    const bitmap = await createImageBitmap(file);
    const scale = Math.min(1, 2000 / Math.max(bitmap.width, bitmap.height));
    const canvas = document.createElement('canvas');
    canvas.width = Math.round(bitmap.width * scale);
    canvas.height = Math.round(bitmap.height * scale);
    canvas.getContext('2d').drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    const blob = await new Promise(ok => canvas.toBlob(ok, 'image/jpeg', 0.85));
    return blob ? new File([blob], file.name.replace(/\.\w+$/, '') + '.jpg', { type: 'image/jpeg' }) : file;
  } catch { return file; }
}

function renderAttachments() {
  const list = el('attachments');
  list.hidden = !attachments.length;
  list.replaceChildren(...attachments.map((a, i) => {
    const li = document.createElement('li');
    li.className = a.error ? 'error' : a.path ? '' : 'busy';
    li.title = a.error || a.path || 'sending…';
    const img = document.createElement('img');
    img.src = a.url;
    img.alt = '';
    const remove = document.createElement('button');
    remove.type = 'button';
    remove.textContent = '×';
    remove.setAttribute('aria-label', 'remove');
    remove.addEventListener('click', () => { URL.revokeObjectURL(a.url); attachments.splice(i, 1); renderAttachments(); });
    li.append(img, remove);
    return li;
  }));
}

function clearAttachments() {
  attachments.forEach(a => URL.revokeObjectURL(a.url));
  attachments = [];
  renderAttachments();
}

async function attach(files) {
  if (!current) return;
  // The kimo and session chosen now: shrinking takes a while, and the user may switch.
  const id = current.id;
  const to = kimo.forHost(kimo.host);
  for (const original of files) {
    const a = { url: URL.createObjectURL(original), path: null, error: null };
    attachments.push(a);
    renderAttachments();
    try {
      const file = await shrink(original);
      if (!attachments.includes(a)) return; // closed or removed meanwhile
      a.path = (await to.sendFile(id, file, file.name || 'photo.jpg')).path;
    } catch (err) {
      a.error = err.message;
      if (attachments.includes(a)) toast(err.message, true);
    }
    if (!attachments.includes(a)) return;
    renderAttachments();
  }
}

compose.querySelectorAll('[data-attach]').forEach(input => input.addEventListener('change', () => {
  attach([...input.files]);
  input.value = '';
}));
compose.elements.text.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); compose.requestSubmit(); }
});
