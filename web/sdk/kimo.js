// kimo SDK: connect any PWA to a kimo instance.
//
//   import { Kimo } from 'https://kimo.your-tailnet.ts.net/sdk/kimo.js';
//   const kimo = new Kimo({ url: 'https://kimo.your-tailnet.ts.net', token: 'kimo_…' });
//   const sessions = await kimo.sessions();
//   const stop = kimo.subscribe(({ sessions, accounts }) => render(sessions));
//
// The token is an app key from kimo's Apps tab (or POST /api/clients), limited to your
// page's origin and to a scope (read or control).

export class KimoError extends Error {
  constructor(message, status) {
    super(message);
    this.name = 'KimoError';
    this.status = status;
  }
}

export class Kimo {
  /**
   * @param {{url?: string, token?: string, host?: string}} options url defaults to the kimo
   * serving this file; `host` reaches a kimo paired with it (`hosts()`) through it.
   */
  constructor({ url, token, host } = {}) {
    this.url = (url || new URL('..', import.meta.url).href).replace(/\/+$/, '');
    this.token = token || '';
    this.host = host || null;
  }

  /** API path prefix: this kimo, or a paired one through it. */
  get base() { return this.host ? `/api/hosts/${enc(this.host)}` : '/api'; }

  /** Talk to a paired kimo (an id or name from `hosts()`), or `null` for this one. */
  useHost(host) { this.host = host || null; }

  /** A client for a paired kimo, sharing this one's address and key. */
  forHost(host) { return new Kimo({ url: this.url, token: this.token, host }); }

  async request(method, path, body, { local = false } = {}) {
    let res;
    try {
      res = await fetch(`${this.url}${local ? '/api' : this.base}${path}`, {
        method,
        headers: {
          ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
          ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (e) {
      throw new KimoError(`cannot reach kimo at ${this.url}`, 0);
    }
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new KimoError(data.error || `HTTP ${res.status}`, res.status);
    return data;
  }

  // Machine
  status() { return this.request('GET', '/status'); }
  /**
   * The kimos paired with this one (`[{id, name, url, online, sessions}]`): pass an id to
   * `useHost()` or `forHost()` to use one of them through this kimo. Owner only.
   */
  hosts() { return this.request('GET', '/peers', undefined, { local: true }); }

  // Projects
  projects() { return this.request('GET', '/projects'); }
  /**
   * @param {{mode: 'new'|'existing'|'clone', name?: string, path?: string, git?: boolean, url?: string}} project
   * new: folder `name` inside `path` (default: the projects folder); existing: the folder at `path`;
   * clone: `git clone url` into a new folder; returns {project, session} (the clone terminal).
   */
  createProject(project) { return this.request('POST', '/projects', project); }
  /** Forget a project (its files stay). */
  deleteProject(id) { return this.request('DELETE', `/projects/${enc(id)}`); }
  /** Rename a project or change its colour: `{name?, color?: '#rrggbb' | ''}`. */
  updateProject(id, changes) { return this.request('PATCH', `/projects/${enc(id)}`, changes); }
  /**
   * A git worktree of the project (a branch in its own folder) with an optional agent in it:
   * `{name, agent?: 'claude'|'codex'|'shell'}`; returns {worktree, session}. Projects list
   * their worktrees in `worktrees` (the project's folder first, `primary: true`).
   */
  addWorktree(id, worktree) { return this.request('POST', `/projects/${enc(id)}/worktrees`, worktree); }
  /** Remove a worktree (refused with uncommitted changes or running agents; the branch stays). */
  removeWorktree(id, path) { return this.request('DELETE', `/projects/${enc(id)}/worktrees?path=${enc(path)}`); }
  /** Earlier Claude and Codex conversations in a folder, newest first (`session` set when attached). */
  conversations(path) { return this.request('GET', `/conversations?path=${enc(path)}`); }
  /** Continue a conversation as a session: `{agent, cwd, id, name?, start? = true}`. */
  continueConversation(c) { return this.request('POST', '/conversations/continue', c); }
  /** Subfolders of `path` (default: the projects folder). */
  /** Create folder `name` inside `parent` (start a project from nothing); returns its listing. */
  makeFolder(parent, name) { return this.request('POST', '/fs', { parent, name }); }
  browse(path) { return this.request('GET', `/fs${path ? `?path=${enc(path)}` : ''}`); }

  // Sessions
  sessions() { return this.request('GET', '/sessions'); }
  session(id) { return this.request('GET', `/sessions/${enc(id)}`); }
  /**
   * @param {{name: string, kind: 'claude'|'codex'|'shell', cwd?: string, account?: string|null, model?: string, effort?: string, args?: string[], command?: string[], start?: boolean}} spec
   * `model` and `effort`: this agent's own (see `agents()` for the levels and model names);
   * a folder outside every project becomes a project.
   */
  createSession(spec) { return this.request('POST', '/sessions', spec); }
  /** The agents' CLIs: [{kind, installed, version, efforts, models}]. */
  agents() { return this.request('GET', '/agents'); }
  deleteSession(id) { return this.request('DELETE', `/sessions/${enc(id)}`); }
  act(id, action) { return this.request('POST', `/sessions/${enc(id)}/action`, { action }); }
  start(id) { return this.act(id, 'start'); }
  pause(id) { return this.act(id, 'pause'); }
  resume(id) { return this.act(id, 'resume'); }
  stop(id) { return this.act(id, 'stop'); }
  restart(id) { return this.act(id, 'restart'); }
  /** Move a session to another account (null = default login); it continues its conversation. */
  setAccount(id, account) { return this.request('POST', `/sessions/${enc(id)}/account`, { account }); }
  /** The agent's last answer (from its own transcript), or its recent screen for shells. */
  reply(id) { return this.request('GET', `/sessions/${enc(id)}/reply`); }
  /** Recent terminal text of a session (escape codes removed). */
  screen(id, lines = 40) { return this.request('GET', `/sessions/${enc(id)}/screen?lines=${lines}`); }
  /** What a session's memory is made of: [{pid, label, bytes, processes}], the agent first, then each MCP server or tool it started. */
  sessionMemory(id) { return this.request('GET', `/sessions/${enc(id)}/memory`); }
  /** Memory report: {total_bytes, available_bytes, agents_bytes, pressure: ok|low|critical, heavy, idle, freed}. */
  memory() { return this.request('GET', '/memory'); }
  /** Stop every idle agent now (they continue their conversation later); returns {stopped}. */
  freeMemory() { return this.request('POST', '/memory/free'); }
  /** Message an agent as another agent (`from`: the sender's session id), like `kimo send`. */
  message(id, text, from) { return this.request('POST', `/sessions/${enc(id)}/message`, { text, from }); }
  /** Type raw text into the terminal. */
  type(id, text) { return this.request('POST', `/sessions/${enc(id)}/input`, { text }); }
  /**
   * Send a file (a photo, a screenshot: a File or Blob, up to 20 MB) to the session's machine;
   * returns {path}. Put the path in a message: Claude Code and Codex read images from it.
   */
  async sendFile(id, file, name = file.name || 'file') {
    // The kimo chosen now, even if useHost() changes while the file is read.
    const to = this.forHost(this.host);
    const bytes = new Uint8Array(await file.arrayBuffer());
    let binary = '';
    for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return to.request('POST', `/sessions/${enc(id)}/files`, { name, data: btoa(binary) });
  }
  /** Send a message to an agent: the text, then Enter (separately, so TUIs see typing, not a paste). */
  async send(id, text) {
    // Both steps go to the kimo chosen now, even if useHost() changes in between.
    const to = this.forHost(this.host);
    await to.type(id, text);
    await new Promise(r => setTimeout(r, 40));
    return to.type(id, '\r');
  }

  // Accounts
  accounts() { return this.request('GET', '/accounts'); }
  /** Usage per account: Claude tokens (5 h / 7 days) and Codex plan limits, for agents run through kimo. */
  usage() { return this.request('GET', '/usage'); }
  /** Usage of every Claude and Codex conversation on the machine: per day, project and account. */
  dailyUsage(days = 30) { return this.request('GET', `/usage/daily?days=${days}`); }
  /** Move every session of an account (or 'default') to another one (null = default login). */
  moveSessions(from, to) { return this.request('POST', `/accounts/${enc(from)}/move`, { to }); }

  // Owner only (kimo's own token): logins and app keys
  /**
   * Make an account the one new agents of `provider` start on (`null` or 'system': this
   * computer's own login); `move: true` also moves that provider's agents to it now.
   */
  setActiveAccount(provider, account, { move = false } = {}) {
    return this.request('POST', '/accounts/active', { provider, account, move });
  }
  createAccount(name, provider) { return this.request('POST', '/accounts', { name, provider }); }
  deleteAccount(id) { return this.request('DELETE', `/accounts/${enc(id)}`); }
  /** Start a sign-in terminal for an account; returns the session. */
  login(id) { return this.request('POST', `/accounts/${enc(id)}/login`); }
  clients() { return this.request('GET', '/clients'); }
  /** @param {{name: string, origins: string[], scope?: 'read'|'control'}} app returns the app with its token (shown once). */
  createClient(app) { return this.request('POST', '/clients', app); }
  deleteClient(id) { return this.request('DELETE', `/clients/${enc(id)}`); }

  // Owner only: settings and updates
  settings() { return this.request('GET', '/settings'); }
  saveSettings(settings) { return this.request('PUT', '/settings', settings); }
  /** The latest kimo release: {current, latest, available, url, notes, manual, error}. */
  release(force = false) { return this.request('GET', `/update${force ? '?force=true' : ''}`); }
  /** Install the latest release over this kimo's binary; returns {installed}. */
  installUpdate() { return this.request('POST', '/update'); }
  /** Restart kimo on the installed version; running sessions continue once it is back. */
  restartKimo() { return this.request('POST', '/update/restart'); }

  // Owner only: the machine
  /** Listening TCP ports: [{port, addresses, pid, process, session, session_name, kimo}]. */
  ports() { return this.request('GET', '/ports'); }
  /** Keeping the computer awake: {mode: 'off'|'agents'|'on', active, supported}. */
  awake() { return this.request('GET', '/awake'); }
  /** @param {'off'|'agents'|'on'} mode `agents`: while an agent is working. */
  setAwake(mode) { return this.request('POST', '/awake', { mode }); }

  // Owner only: devices and network
  network() { return this.request('GET', '/network'); }
  /** Serve kimo over HTTPS on the tailnet (tailscale serve); returns {url}. */
  enableHttps() { return this.request('POST', '/network/https'); }
  /** One-time code (10 minutes) for a pairing link `<address>/#pair=<code>`. */
  startPairing() { return this.request('POST', '/pairing'); }
  /** Trade a pairing code for this device's key (no key needed); returns the device with its token. */
  pair(code, name) { return this.request('POST', '/pair', { code, name }, { local: true }); }
  devices() { return this.request('GET', '/devices'); }
  deleteDevice(id) { return this.request('DELETE', `/devices/${enc(id)}`); }
  /** QR code of `text` as SVG markup. */
  async qr(text) {
    const res = await fetch(this.urlWithToken(`${this.base}/qr`) + `${this.token ? '&' : '?'}text=${enc(text)}`);
    if (!res.ok) throw new KimoError(`QR failed: HTTP ${res.status}`, res.status);
    return res.text();
  }

  // Live updates
  /**
   * Call `onUpdate({sessions, accounts})` now and on every change (status, account, usage).
   * Reconnects by itself. Returns a function that stops the subscription.
   */
  subscribe(onUpdate, onError) {
    const source = new EventSource(this.urlWithToken(`${this.base}/events`));
    source.addEventListener('update', e => onUpdate(JSON.parse(e.data)));
    if (onError) source.onerror = onError;
    return () => source.close();
  }

  /**
   * Resolve with the session once its activity is one of `activities` (default: not working any
   * more), e.g. `await kimo.waitFor(id)` after `kimo.send(id, 'run the tests')`.
   */
  waitFor(id, activities = ['idle', 'waiting'], { timeout = 0 } = {}) {
    return new Promise((resolve, reject) => {
      let timer = null;
      let stop = () => {};
      const settle = (fn, value) => { clearTimeout(timer); stop(); fn(value); };
      // Ignore the first second: an agent that was just prompted may not have started yet.
      const after = Date.now() + 1000;
      stop = this.subscribe(({ sessions }) => {
        const s = sessions.find(x => x.id === id);
        if (!s) return settle(reject, new KimoError(`session ${id} is gone`, 404));
        if (s.status !== 'running') return settle(resolve, s);
        if (Date.now() >= after && activities.includes(s.activity)) settle(resolve, s);
      });
      if (timeout) timer = setTimeout(() => settle(reject, new KimoError('timed out waiting for the agent', 408)), timeout);
    });
  }

  /** WebSocket URL of a session's terminal (binary frames both ways; text `{"type":"resize",cols,rows}`). */
  terminalUrl(id) {
    return this.urlWithToken(`${this.base}/sessions/${enc(id)}/term`).replace(/^http/, 'ws');
  }

  urlWithToken(path) {
    return `${this.url}${path}${this.token ? `?token=${encodeURIComponent(this.token)}` : ''}`;
  }
}

/** Seconds since a session last printed something (null when it never did). */
export function quietFor(session, now = Date.now()) {
  return session.last_output_at ? Math.max(0, Math.round((now - session.last_output_at) / 1000)) : null;
}

const enc = encodeURIComponent;
