// <kimo-terminal>: a live kimo terminal for any page.
//
//   <script type="module" src="https://kimo.your-tailnet.ts.net/sdk/kimo-terminal.js"></script>
//   <kimo-terminal url="https://kimo.your-tailnet.ts.net" token="kimo_…" session="api-refactor-1a2b3c"></kimo-terminal>
//
// Or set `element.kimo = new Kimo({...})` instead of url/token. Attributes: `session`,
// `readonly` (watch only), `font-size`. Methods: `send(text)` types into the terminal (false
// when disconnected: nothing is queued), `focus()`; `connected`, with `open`/`close` events.
// Colours follow the page's --color-background / --color-ink tokens when defined.

import { Kimo } from './kimo.js';

const BASE = new URL('..', import.meta.url).href;
let library;

function loadLibrary() {
  if (window.Terminal && window.FitAddon) return Promise.resolve();
  library ||= new Promise((resolve, reject) => {
    const css = document.createElement('link');
    css.rel = 'stylesheet';
    css.href = `${BASE}vendor/xterm/xterm.css`;
    document.head.append(css);
    const load = src => new Promise((ok, fail) => {
      const s = document.createElement('script');
      s.src = `${BASE}vendor/xterm/${src}`;
      s.onload = ok;
      s.onerror = () => fail(new Error(`cannot load ${src} from kimo`));
      document.head.append(s);
    });
    load('xterm.js').then(() => load('addon-fit.js')).then(resolve, reject);
  });
  return library;
}

const encoder = new TextEncoder();

/** xterm colours from the page's tokens; ANSI colours tuned for a light or dark background. */
function terminalTheme(el) {
  const css = getComputedStyle(el);
  const v = (name, fallback) => css.getPropertyValue(name).trim() || fallback;
  const dark = v('color-scheme', '').includes('dark');
  const ansi = dark
    ? { black: '#3a3a3a', blue: '#6c9cf0', magenta: '#c084d6', white: '#c8c8c8', brightBlack: '#6e6e6e', brightRed: '#ff7a87',
      brightGreen: '#72d48e', brightYellow: '#f0c068', brightBlue: '#8cb4ff', brightMagenta: '#d6a0ea', brightCyan: '#5fd4da', brightWhite: '#f0f0f0' }
    // Darkened so agent output stays readable on white.
    : { black: '#232323', blue: '#2b5fb4', magenta: '#8a3fa0', white: '#898989', brightBlack: '#666666', brightRed: '#e0495a',
      brightGreen: '#3a9d5a', brightYellow: '#a8741c', brightBlue: '#3d6fc4', brightMagenta: '#9b52b1', brightCyan: '#1aa3aa', brightWhite: '#aaaaaa' };
  return {
    background: v('--color-background', '#ffffff'), foreground: v('--color-ink', '#232323'),
    cursor: v('--color-ink', '#232323'), cursorAccent: v('--color-background', '#ffffff'),
    selectionBackground: v('--color-line', '#e5e5e5'),
    red: v('--color-bad', '#d63345'), green: v('--color-good', '#2f8f4e'), yellow: v('--color-warn', '#b7791f'), cyan: v('--color-teal', '#12939a'),
    ...ansi,
  };
}

class KimoTerminal extends HTMLElement {
  static observedAttributes = ['session', 'url', 'token'];

  set kimo(client) { this._kimo = client; this.reconnect(); }
  get kimo() {
    return this._kimo || new Kimo({ url: this.getAttribute('url') || undefined, token: this.getAttribute('token') || '' });
  }

  async connectedCallback() {
    if (!this.style.display) this.style.display = 'block';
    if (!this.style.height && !this.clientHeight) this.style.height = '24rem';
    await loadLibrary();
    if (!this.isConnected || this.term) return;
    const css = getComputedStyle(this);
    const v = (name, fallback) => css.getPropertyValue(name).trim() || fallback;
    this.term = new window.Terminal({
      fontFamily: v('--kimo-terminal-font', '"Roboto Mono", ui-monospace, Menlo, monospace'),
      fontSize: Number(this.getAttribute('font-size')) || (matchMedia('(max-width: 600px)').matches ? 11 : 13),
      fontWeight: 300,
      fontWeightBold: 500,
      scrollback: 5000,
      cursorBlink: !this.hasAttribute('readonly'),
      disableStdin: this.hasAttribute('readonly'),
      theme: terminalTheme(this),
    });
    // Follow the page's light/dark scheme (data-theme on <html>, or the system's).
    this.themeWatch = new MutationObserver(() => this.retheme());
    this.themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    this.schemeWatch = matchMedia('(prefers-color-scheme: dark)');
    this.schemeWatch.addEventListener?.('change', this.retheme);
    this.fit = new window.FitAddon.FitAddon();
    this.term.loadAddon(this.fit);
    this.term.open(this);
    this.term.onData(d => { if (!this.hasAttribute('readonly')) this.sendBytes(encoder.encode(d)); });
    this.resizer = new ResizeObserver(() => this.resize());
    this.resizer.observe(this);
    this.reconnect();
  }

  retheme = () => { if (this.term) this.term.options.theme = terminalTheme(this); };

  disconnectedCallback() {
    this.themeWatch?.disconnect();
    this.schemeWatch?.removeEventListener?.('change', this.retheme);
    this.closeSocket();
    this.resizer?.disconnect();
    this.term?.dispose();
    this.term = null;
  }

  attributeChangedCallback() { this.reconnect(); }

  reconnect() {
    if (!this.term) return;
    this.closeSocket();
    const id = this.getAttribute('session');
    if (!id) return;
    this.term.reset();
    const ws = new WebSocket(this.kimo.terminalUrl(id));
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => { this.resize(); this.dispatchEvent(new Event('open')); };
    ws.onmessage = e => this.term?.write(typeof e.data === 'string' ? e.data : new Uint8Array(e.data));
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.dispatchEvent(new Event('close'));
      this.retry = setTimeout(() => this.reconnect(), 1500);
    };
    this.ws = ws;
  }

  closeSocket() {
    clearTimeout(this.retry);
    const ws = this.ws;
    this.ws = null;
    ws?.close();
  }

  resize() {
    if (!this.term || !this.fit) return;
    try { this.fit.fit(); } catch { return; }
    if (this.ws?.readyState === WebSocket.OPEN && !this.hasAttribute('readonly')) {
      this.ws.send(JSON.stringify({ type: 'resize', cols: this.term.cols, rows: this.term.rows }));
    }
  }

  /** Is the terminal connected (typing reaches it)? `open` and `close` events say when this changes. */
  get connected() { return this.ws?.readyState === WebSocket.OPEN; }

  /** Send bytes; `false` when not connected (nothing was sent: keep what was to be sent). */
  sendBytes(bytes) {
    if (!this.connected) return false;
    this.ws.send(bytes);
    return true;
  }

  /** Type text into the terminal (add '\r' for Enter); `false` when not connected. */
  send(text) { return this.sendBytes(encoder.encode(text)); }

  focus() { this.term?.focus(); }
}

if (!customElements.get('kimo-terminal')) customElements.define('kimo-terminal', KimoTerminal);
export { KimoTerminal };
