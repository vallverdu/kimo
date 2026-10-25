// Full-screen terminal: xterm.js over the session websocket, with a key bar and a compose box for phones.
const terminal = (() => {
  const el = id => document.getElementById(id);
  const css = getComputedStyle(document.documentElement);
  const color = name => css.getPropertyValue(name).trim();
  const KEYS = { esc: '\x1b', tab: '\t', ctrlc: '\x03', up: '\x1b[A', down: '\x1b[B', right: '\x1b[C', left: '\x1b[D', enter: '\r' };
  let term, fit, ws, current, retry;

  function ensureTerm() {
    if (term) return;
    term = new Terminal({
      fontFamily: '"Roboto Mono", ui-monospace, Menlo, monospace',
      fontSize: matchMedia('(max-width: 600px)').matches ? 11 : 13,
      fontWeight: 300,
      fontWeightBold: 500,
      scrollback: 5000,
      cursorBlink: true,
      theme: {
        background: color('--color-background'), foreground: color('--color-ink'),
        cursor: color('--color-ink'), cursorAccent: color('--color-background'),
        selectionBackground: color('--color-line'),
        // ANSI colours darkened so agent output stays readable on white.
        black: '#232323', red: color('--color-bad'), green: color('--color-good'), yellow: color('--color-warn'),
        blue: '#2b5fb4', magenta: '#8a3fa0', cyan: color('--color-teal'), white: '#898989',
        brightBlack: '#666666', brightRed: '#e0495a', brightGreen: '#3a9d5a', brightYellow: '#a8741c',
        brightBlue: '#3d6fc4', brightMagenta: '#9b52b1', brightCyan: '#1aa3aa', brightWhite: '#aaaaaa',
      },
    });
    fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(el('term-screen'));
    term.onData(d => send(new TextEncoder().encode(d)));
    new ResizeObserver(() => resize()).observe(el('term-screen'));
  }

  function send(bytes) { if (ws?.readyState === WebSocket.OPEN) ws.send(bytes); }

  function resize() {
    if (!current || !fit) return;
    try { fit.fit(); } catch { return; }
    if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
  }

  function connect() {
    clearTimeout(retry);
    term.reset();
    ws = new WebSocket(api.termUrl(current.id));
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => resize();
    ws.onmessage = e => term.write(typeof e.data === 'string' ? e.data : new Uint8Array(e.data));
    ws.onclose = () => { if (current) retry = setTimeout(connect, 1500); };
  }

  function open(session) {
    ensureTerm();
    current = session;
    el('term').hidden = false;
    document.body.style.overflow = 'hidden';
    update(session);
    connect();
    if (!matchMedia('(pointer: coarse)').matches) term.focus();
  }

  function close() {
    current = null;
    clearTimeout(retry);
    ws?.close();
    el('term').hidden = true;
    document.body.style.overflow = '';
  }

  function update(session) {
    if (!current || session.id !== current.id) return;
    current = session;
    el('term-name').textContent = session.name;
    const badge = el('term-status');
    badge.className = `ui-badge ${session.status}`;
    badge.textContent = session.status;
    const pause = el('term-pause');
    pause.hidden = !['running', 'paused'].includes(session.status);
    pause.textContent = session.status === 'paused' ? 'resume' : 'pause';
  }

  el('term-back').addEventListener('click', () => { close(); terminal.onClose?.(); });
  el('term-pause').addEventListener('click', () => terminal.onToggle?.(current));
  el('keys').addEventListener('click', e => {
    const key = e.target.closest('button')?.dataset.key;
    if (key) send(new TextEncoder().encode(KEYS[key]));
  });
  const compose = el('compose');
  compose.addEventListener('submit', e => {
    e.preventDefault();
    const box = compose.elements.text;
    // Text first, then Enter separately so TUIs treat it as typed input rather than a paste.
    send(new TextEncoder().encode(box.value));
    setTimeout(() => send(new TextEncoder().encode('\r')), 30);
    box.value = '';
  });
  compose.elements.text.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); compose.requestSubmit(); }
  });

  return { open, close, update, get current() { return current; }, onClose: null, onToggle: null };
})();
