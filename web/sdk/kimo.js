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
  /** @param {{url?: string, token?: string}} options url defaults to the kimo serving this file. */
  constructor({ url, token } = {}) {
    this.url = (url || new URL('..', import.meta.url).href).replace(/\/+$/, '');
    this.token = token || '';
  }

  async request(method, path, body) {
    let res;
    try {
      res = await fetch(`${this.url}/api${path}`, {
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

  // Sessions
  sessions() { return this.request('GET', '/sessions'); }
  session(id) { return this.request('GET', `/sessions/${enc(id)}`); }
  /** @param {{name: string, kind: 'claude'|'codex'|'shell', cwd?: string, account?: string|null, args?: string[], command?: string[], start?: boolean}} spec */
  createSession(spec) { return this.request('POST', '/sessions', spec); }
  deleteSession(id) { return this.request('DELETE', `/sessions/${enc(id)}`); }
  act(id, action) { return this.request('POST', `/sessions/${enc(id)}/action`, { action }); }
  start(id) { return this.act(id, 'start'); }
  pause(id) { return this.act(id, 'pause'); }
  resume(id) { return this.act(id, 'resume'); }
  stop(id) { return this.act(id, 'stop'); }
  restart(id) { return this.act(id, 'restart'); }
  /** Move a session to another account (null = default login); it continues its conversation. */
  setAccount(id, account) { return this.request('POST', `/sessions/${enc(id)}/account`, { account }); }
  /** Type raw text into the terminal. */
  type(id, text) { return this.request('POST', `/sessions/${enc(id)}/input`, { text }); }
  /** Send a message to an agent: the text, then Enter (separately, so TUIs see typing, not a paste). */
  async send(id, text) {
    await this.type(id, text);
    await new Promise(r => setTimeout(r, 40));
    return this.type(id, '\r');
  }

  // Accounts
  accounts() { return this.request('GET', '/accounts'); }
  /** Move every session of an account (or 'default') to another one (null = default login). */
  moveSessions(from, to) { return this.request('POST', `/accounts/${enc(from)}/move`, { to }); }

  // Owner only (kimo's own token): logins and app keys
  createAccount(name, provider) { return this.request('POST', '/accounts', { name, provider }); }
  deleteAccount(id) { return this.request('DELETE', `/accounts/${enc(id)}`); }
  /** Start a sign-in terminal for an account; returns the session. */
  login(id) { return this.request('POST', `/accounts/${enc(id)}/login`); }
  clients() { return this.request('GET', '/clients'); }
  /** @param {{name: string, origins: string[], scope?: 'read'|'control'}} app returns the app with its token (shown once). */
  createClient(app) { return this.request('POST', '/clients', app); }
  deleteClient(id) { return this.request('DELETE', `/clients/${enc(id)}`); }

  // Live updates
  /**
   * Call `onUpdate({sessions, accounts})` now and on every change (status, account, usage).
   * Reconnects by itself. Returns a function that stops the subscription.
   */
  subscribe(onUpdate, onError) {
    const source = new EventSource(this.urlWithToken('/api/events'));
    source.addEventListener('update', e => onUpdate(JSON.parse(e.data)));
    if (onError) source.onerror = onError;
    return () => source.close();
  }

  /** WebSocket URL of a session's terminal (binary frames both ways; text `{"type":"resize",cols,rows}`). */
  terminalUrl(id) {
    return this.urlWithToken(`/api/sessions/${enc(id)}/term`).replace(/^http/, 'ws');
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
