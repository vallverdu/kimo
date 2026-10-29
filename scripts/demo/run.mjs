#!/usr/bin/env node
// A demo kimo with invented data, for screenshots and trying the app without real accounts.
//
//   node scripts/demo/run.mjs                 # run it: open the printed address
//   node scripts/demo/run.mjs --capture       # phone and window screenshots → docs/screenshots
//   node scripts/demo/run.mjs --capture phone # only the phone ones
//   node scripts/demo/run.mjs --capture --only phone-map,app-files
//
// A real kimo (target/debug/kimo, built with the window) runs on an isolated home folder seeded
// with demo projects, terminals, accounts and usage logs. A small proxy in front of it answers
// what cannot be real on this machine from scripts/demo/world.mjs: the tailnet, the paired
// kimos, the fleet, hardware facts, plan limits, addresses. Nothing real is read or shown: the
// fake `tailscale` on its PATH stands in for the real one, accounts hold no credentials and
// shutdown is `true`.

import { spawn, execFileSync } from 'node:child_process';
import { createServer, request as httpRequest } from 'node:http';
import { connect } from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import * as W from './world.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i < 0 ? fallback : args[i + 1] && !args[i + 1].startsWith('--') ? args[i + 1] : true;
};
const PORT = Number(flag('port', 7790));
const KIMO_PORT = PORT + 1;
const capture = flag('capture', false);
const only = flag('only', null); // a comma-separated list of shot names
const wanted = shot => !only || only.split(',').includes(shot.name);
const DIR = flag('dir', fs.mkdtempSync(path.join(os.tmpdir(), 'kimo-demo-')));
const HOME = path.join(DIR, 'home');
const DATA = path.join(DIR, 'data');
const BIN = path.join(DIR, 'bin');
const KIMO = path.join(ROOT, 'target/debug/kimo');
const OUT = path.join(ROOT, 'docs/screenshots');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const day = 86_400_000;

// ----- the home folder: projects, logins, usage logs -----

function write(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}
const projectPath = id => path.join(HOME, 'dev', id);
const claudeDir = cwd => cwd.replace(/[^A-Za-z0-9]/g, '-');

function seedHome() {
  write(path.join(BIN, 'tailscale'), `#!/bin/sh\n[ "$1" = version ] && { echo 1.90.0; exit 0; }\n[ "$1" = status ] && { cat '${path.join(DIR, 'tailscale.json')}'; exit 0; }\nexit 1\n`);
  fs.chmodSync(path.join(BIN, 'tailscale'), 0o755);
  write(path.join(DIR, 'tailscale.json'), JSON.stringify(W.tailscaleStatus()));
  // Stand-in agents: each shows its project's screen (and keeps working, or waits).
  const screens = path.join(DIR, 'screens');
  for (const s of W.SESSIONS) {
    const key = `${s.kind}-${s.project}`;
    write(path.join(screens, `${key}.ansi`), s.screen);
    if (/esc to interrupt|npm run dev/.test(s.screen)) write(path.join(screens, `${key}.working`), '');
  }
  for (const kind of ['claude', 'codex', 'shell']) {
    write(path.join(BIN, kind === 'shell' ? 'demo-shell' : kind), `#!/bin/sh
k='${screens}/${kind}-'"$(basename "$PWD")"
cat "$k.ansi"
if [ -f "$k.working" ]; then
  # Output that changes nothing on screen: the agent counts as working.
  while :; do printf '\\033[0m'; sleep 1; done
fi
exec sleep 1000000
`);
    fs.chmodSync(path.join(BIN, kind === 'shell' ? 'demo-shell' : kind), 0o755);
  }
  for (const p of W.PROJECTS) {
    const dir = projectPath(p.id);
    write(path.join(dir, 'README.md'), `# ${p.name}\n\nA demonstration project.\n`);
    write(path.join(dir, 'src', 'main.rs'), 'fn main() {\n    println!("hello");\n}\n');
    execFileSync('git', ['init', '-q', '-b', 'main'], { cwd: dir });
    execFileSync('git', ['-c', 'user.name=Demo', '-c', 'user.email=demo@example.com', '-c', 'commit.gpgsign=false', 'add', '.'], { cwd: dir });
    execFileSync('git', ['-c', 'user.name=Demo', '-c', 'user.email=demo@example.com', '-c', 'commit.gpgsign=false', 'commit', '-q', '-m', 'Start'], { cwd: dir });
    // Something changed, for the Changes panel.
    write(path.join(dir, 'src', 'main.rs'), 'fn main() {\n    println!("hello, invoices");\n    totals::print();\n}\n');
    write(path.join(dir, 'src', 'totals.rs'), 'pub fn print() {}\n');
  }
  // This computer's own logins (no credentials: an email in the config is all kimo reads).
  write(path.join(HOME, '.claude.json'), JSON.stringify({ oauthAccount: { emailAddress: 'studio@example.com' } }));
  write(path.join(HOME, '.codex', 'auth.json'), '{}');
  // Usage logs of earlier conversations, over two weeks.
  const now = Date.now();
  let n = 0;
  for (const c of W.CONVERSATIONS) {
    // As kimo sees the folder (macOS: /var is /private/var).
    const cwd = fs.realpathSync(projectPath(c.project));
    const id = `00000000-0000-4000-8000-${String(++n).padStart(12, '0')}`;
    const lines = [];
    const at = t => new Date(t).toISOString();
    const start = now - c.days * day - 2 * 3600_000;
    if (c.agent === 'claude') {
      lines.push({ type: 'user', timestamp: at(start), cwd, sessionId: id, uuid: `${id}-u`, message: { role: 'user', content: c.prompt } });
      for (let i = 0; i < 18; i++) {
        lines.push({ type: 'assistant', timestamp: at(start + i * 240_000), cwd, sessionId: id,
          message: { id: `${id}-m${i}`, model: i % 3 ? 'claude-sonnet-5-5' : 'claude-opus-5-5',
            usage: { input_tokens: 2200 + i * 40, output_tokens: 900 + i * 25, cache_creation_input_tokens: 4000, cache_read_input_tokens: 60_000 + i * 3000 } } });
      }
      // The Work account's conversations live in its own config folder.
      const base = n % 2 ? path.join(DIR, 'work-account') : path.join(HOME, '.claude');
      const file = path.join(base, 'projects', claudeDir(cwd), `${id}.jsonl`);
      write(file, lines.map(l => JSON.stringify(l)).join('\n') + '\n');
      fs.utimesSync(file, new Date(start + 3600_000), new Date(start + 3600_000));
    } else {
      lines.push({ timestamp: at(start), type: 'session_meta', payload: { id, cwd } });
      lines.push({ timestamp: at(start), type: 'turn_context', payload: { model: 'gpt-5.3-codex', cwd } });
      lines.push({ timestamp: at(start), type: 'event_msg', payload: { type: 'item_completed', item: { type: 'UserMessage', content: [{ type: 'text', text: c.prompt }] } } });
      for (let i = 0; i < 14; i++) {
        lines.push({ timestamp: at(start + i * 300_000), type: 'event_msg', payload: { type: 'token_count', info: { last_token_usage: { input_tokens: 9000 + i * 300, cached_input_tokens: 7000, output_tokens: 1200 } } } });
      }
      const d = new Date(start);
      const folder = path.join(HOME, '.codex', 'sessions', String(d.getFullYear()), String(d.getMonth() + 1).padStart(2, '0'), String(d.getDate()).padStart(2, '0'));
      const file = path.join(folder, `rollout-${id}.jsonl`);
      write(file, lines.map(l => JSON.stringify(l)).join('\n') + '\n');
      fs.utimesSync(file, new Date(start + 3600_000), new Date(start + 3600_000));
    }
  }
}

// Account ids kimo gave the demo accounts (plan limits are keyed by them).
const accountIds = {};
const sessionIds = {};

// ----- kimo itself -----

async function api(method, p, body) {
  const res = await fetch(`http://127.0.0.1:${KIMO_PORT}/api${p}`, {
    method,
    headers: { 'Content-Type': 'application/json', Host: `localhost:${KIMO_PORT}` },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(`${method} ${p}: ${data.error || res.status}`);
  return data;
}

async function startKimo() {
  const env = {
    ...process.env, HOME, KIMO_DATA_DIR: DATA, KIMO_NAME: 'studio', KIMO_PROJECTS_DIR: path.join(HOME, 'dev'),
    KIMO_SHUTDOWN_COMMAND: 'true', PATH: `${BIN}:${process.env.PATH}`, RUST_LOG: 'kimo=warn',
    SHELL: path.join(BIN, 'demo-shell'),
  };
  const child = spawn(KIMO, ['--bind', 'localhost', '--port', String(KIMO_PORT), 'serve'], { env, stdio: ['ignore', 'ignore', 'inherit'] });
  for (let i = 0; i < 100; i++) {
    if (await api('GET', '/status').then(() => true, () => false)) return child;
    await sleep(100);
  }
  throw new Error('kimo did not start');
}

async function seedKimo() {
  await api('PUT', '/settings', { ...(await api('GET', '/settings')), plan_limits: false, keep_awake: 'agents' });
  let work = null;
  accountIds.work = null;
  for (const [name, email] of [['Work', 'work@example.com'], ['Personal', 'personal@example.com']]) {
    const a = await api('POST', '/accounts', { name, provider: 'claude' });
    const home = path.join(DATA, 'accounts', a.id);
    write(path.join(home, '.claude.json'), JSON.stringify({ oauthAccount: { emailAddress: email } }));
    accountIds[name.toLowerCase()] = a.id;
    if (name === 'Work') {
      work = a.id;
      // Its conversations, in its own config folder.
      fs.cpSync(path.join(DIR, 'work-account'), home, { recursive: true, preserveTimestamps: true });
    }
  }
  await api('POST', '/accounts/active', { provider: 'claude', account: work, move: false });
  for (const p of W.PROJECTS) {
    const made = await api('POST', '/projects', { mode: 'existing', path: projectPath(p.id) });
    const id = made.project?.id || p.id;
    await api('PATCH', `/projects/${id}`, { color: p.color }).catch(() => {});
    // Projects continue their earlier conversations when added: not wanted here.
    for (const s of made.continued || []) await api('DELETE', `/sessions/${s.id}`).catch(() => {});
  }
  const ids = {};
  for (const s of W.SESSIONS) {
    const made = await api('POST', '/sessions', { name: s.name, kind: s.kind, cwd: projectPath(s.project) });
    ids[s.name] = made.id;
    sessionIds[s.name] = made.id;
  }
  return ids;
}

// ----- the proxy: kimo, plus what this machine cannot show -----

// The demo home is shown as /Users/demo (and taken back on the way in).
const SHOWN = '/Users/demo';
fs.mkdirSync(HOME, { recursive: true });
const REAL = [...new Set([fs.realpathSync(HOME), HOME])].sort((a, b) => b.length - a.length);
const show = text => REAL.reduce((t, r) => t.split(r).join(SHOWN), text);
const unshow = text => text.split(SHOWN).join(REAL[0]);

function fixtures(req) {
  const url = new URL(req.url, 'http://x');
  const p = url.pathname;
  const hub = W.MACHINES.find(m => m.hub);
  if (req.method !== 'GET' && req.method !== 'HEAD') return null;
  const host = p.match(/^\/api\/hosts\/([^/]+)\/(.*)$/);
  if (host) {
    const [, id, rest] = host;
    const m = W.MACHINES.find(x => x.id === id);
    if (!m) return null;
    if (rest === 'projects') return W.REMOTE_PROJECTS[id] || [];
    if (rest === 'node') return W.node(m);
    if (rest === 'sessions') return W.REMOTE_SESSIONS[id] || [];
    if (rest === 'status') return { host: id, version: '0.1.15', sessions: (W.REMOTE_SESSIONS[id] || []).length };
    return null;
  }
  switch (p) {
    case '/api/network': return W.network(PORT);
    case '/api/network/tailnet': return W.tailnet();
    case '/api/node': return W.node(hub);
    case '/api/peers': return W.peers();
    case '/api/peers/keys': return [];
    case '/api/usage': return W.usage().map(u => ({ ...u, account: u.account && accountIds[u.account] }));
    case '/api/update': return { available: false, current: '0.1.15', latest: 'v0.1.15', installed: null, restart: false, error: null, manual: null };
    case '/api/devices': return [
      { id: 'iphone', name: 'iPhone', paired_at: Date.now() - 12 * day, role: 'owner' },
      { id: 'ipad', name: 'iPad mini', paired_at: Date.now() - 9 * day, role: 'owner' },
    ];
    case '/api/ports': return [
      { port: 5173, addresses: ['127.0.0.1'], pid: 4242, process: 'node', session: sessionIds['dev server'] || null, session_name: 'dev server', kimo: false },
      { port: 7777, addresses: ['0.0.0.0'], pid: 101, process: 'kimo', session: null, kimo: true },
    ];
    case '/api/fleet/status': return W.FLEET_STATUS;
    case '/api/fleet/devices': return W.fleetDevices();
    case '/api/fleet/jobs': return W.fleetJobs();
    case '/api/fleet/workflows': return W.fleetWorkflows();
    case '/api/fleet/events': return W.fleetEvents();
    case '/api/fleet/ssh-key': return { key: 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDemoKeyNotARealKeyDemoKeyNotARealKey studio' };
    default: return null;
  }
}

// What kimo answers, with this machine's figures replaced by the demo hub's.
const patch = {
  '/api/status': s => ({ ...s, host: 'studio', version: '0.1.15', fleet: true, mem_total_bytes: 32e9, mem_available_bytes: 18.4e9, kimo_rss_bytes: 7.6e6 }),
  // Plan limits come from the fixtures: the real setting stays off, so the demo kimo never
  // reads a Claude login from this machine's Keychain.
  '/api/settings': st => ({ ...st, plan_limits: true }),
  '/api/memory': m => ({ ...m, total_bytes: 32e9, available_bytes: 18.4e9, low_bytes: 4.8e9, critical_bytes: 1.6e9 }),
  '/api/sessions': (list, url) => {
    if (url.searchParams.get('all') !== '1') return list;
    const remote = Object.entries(W.REMOTE_SESSIONS).flatMap(([host, ss]) => ss.map(s => ({ ...s, id: `${host}/${s.id}`, host, local: false })));
    return [...list, ...remote];
  },
};

function proxy(req, res) {
  const fixed = fixtures(req);
  if (fixed !== null) {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    return res.end(JSON.stringify(fixed));
  }
  const target = unshow(decodeURIComponent(req.url)).split('?');
  const pathAndQuery = target[0].split('/').map(encodeURIComponent).join('/') + (target[1] ? `?${new URLSearchParams(target.slice(1).join('?')).toString()}` : '');
  const url = new URL(req.url, 'http://x');
  const change = patch[url.pathname];
  const headers = { ...req.headers, host: `localhost:${KIMO_PORT}` };
  delete headers['content-length'];
  delete headers.origin;
  const up = httpRequest({ host: '127.0.0.1', port: KIMO_PORT, method: req.method, path: pathAndQuery, headers }, r => {
    const json = /json/.test(r.headers['content-type'] || '');
    const stream = /event-stream/.test(r.headers['content-type'] || '');
    if (stream) {
      res.writeHead(r.statusCode, r.headers);
      r.on('data', c => res.write(show(c.toString())));
      r.on('end', () => res.end());
      return;
    }
    if (!json) {
      res.writeHead(r.statusCode, r.headers);
      return r.pipe(res);
    }
    const chunks = [];
    r.on('data', c => chunks.push(c));
    r.on('end', () => {
      let body = Buffer.concat(chunks).toString();
      if (change && r.statusCode === 200) body = JSON.stringify(change(JSON.parse(body), url));
      body = show(body);
      res.writeHead(r.statusCode, { ...r.headers, 'content-length': Buffer.byteLength(body) });
      res.end(body);
    });
  });
  up.on('error', () => { res.writeHead(502); res.end(); });
  const body = [];
  req.on('data', c => body.push(c));
  req.on('end', () => up.end(body.length ? unshow(Buffer.concat(body).toString()) : undefined));
}

function startProxy() {
  const server = createServer(proxy);
  // Terminals and other websockets go straight through.
  server.on('upgrade', (req, socket, head) => {
    const up = connect(KIMO_PORT, '127.0.0.1', () => {
      const lines = [`${req.method} ${req.url} HTTP/1.1`];
      for (let i = 0; i < req.rawHeaders.length; i += 2) {
        const [k, v] = [req.rawHeaders[i], req.rawHeaders[i + 1]];
        // kimo sees the page as its own (the demo is local only).
        if (k.toLowerCase() === 'origin') continue;
        lines.push(`${k}: ${k.toLowerCase() === 'host' ? `localhost:${KIMO_PORT}` : v}`);
      }
      up.write(`${lines.join('\r\n')}\r\n\r\n`);
      if (head.length) up.write(head);
      up.pipe(socket);
      socket.pipe(up);
    });
    up.on('error', () => socket.destroy());
    socket.on('error', () => up.destroy());
  });
  return new Promise(r => server.listen(PORT, '127.0.0.1', () => r(server)));
}

// ----- screenshots -----

async function setTheme(theme) {
  await api('PUT', '/settings', { ...(await api('GET', '/settings')), theme });
}

/** A minimal Chrome DevTools client: one page, commands and their answers. */
async function chrome() {
  const bin = process.env.CHROME || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'kimo-chrome-'));
  const proc = spawn(bin, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--hide-scrollbars', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
  const wsUrl = await new Promise((resolve, reject) => {
    let err = '';
    proc.stderr.on('data', d => {
      err += d;
      const m = err.match(/ws:\/\/[^\s]+/);
      if (m) resolve(m[0]);
    });
    setTimeout(() => reject(new Error('Chrome did not start')), 15000);
  });
  const port = new URL(wsUrl).port;
  const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const page = targets.find(t => t.type === 'page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise(r => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', e => {
    const msg = JSON.parse(e.data);
    if (msg.id && waiting.has(msg.id)) {
      waiting.get(msg.id)(msg);
      waiting.delete(msg.id);
    }
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const n = ++id;
    waiting.set(n, m => (m.error ? reject(new Error(`${method}: ${m.error.message}`)) : resolve(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const close = () => { ws.close(); proc.kill(); setTimeout(() => fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5 }), 500); };
  return { send, close };
}

const PHONE = [
  { name: 'phone-agents', hash: 'agents' },
  { name: 'phone-projects', hash: 'projects' },
  { name: 'phone-terminal', hash: 'agents', open: 'invoices api' },
  { name: 'phone-history', hash: 'projects', click: '[data-do=history]' },
  { name: 'phone-usage', hash: 'usage', wait: 2500 },
  { name: 'phone-accounts', hash: 'accounts' },
  { name: 'phone-fleet', hash: 'fleet' },
  { name: 'phone-map', hash: 'map', wait: 6000 },
];

async function capturePhone() {
  const c = await chrome();
  await c.send('Page.enable');
  await c.send('Runtime.enable');
  await c.send('Emulation.setDeviceMetricsOverride', { width: 430, height: 932, deviceScaleFactor: 2, mobile: true });
  await c.send('Emulation.setTouchEmulationEnabled', { enabled: true });
  for (const theme of ['light', 'dark']) {
    await setTheme(theme);
    await c.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const shot of PHONE.filter(wanted)) {
      // The install hint stays dismissed, as after its normal "not now".
      await c.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/?shot=${shot.name}#${shot.hash}` });
      await sleep(2200);
      await c.send('Runtime.evaluate', { expression: "try { localStorage.setItem('kimo-install-hidden', '1') } catch {}" });
      if (shot.open) {
        await c.send('Runtime.evaluate', { expression: `[...document.querySelectorAll('#sessions li')].find(li => li.textContent.includes(${JSON.stringify(shot.open)}))?.querySelector('.k-name')?.click()` });
      }
      if (shot.click) {
        await c.send('Runtime.evaluate', { expression: `document.querySelector(${JSON.stringify(shot.click)})?.click()` });
      }
      await sleep(shot.wait || 1600);
      const { data } = await c.send('Page.captureScreenshot', { format: 'png' });
      const file = path.join(OUT, `${shot.name}${theme === 'dark' ? '-dark' : ''}.png`);
      fs.writeFileSync(file, Buffer.from(data, 'base64'));
      console.log(`  ${path.relative(ROOT, file)}`);
    }
  }
  c.close();
}

const WINDOW = [
  { name: 'app-claude', session: 'invoices api' },
  { name: 'app-needs-you', session: 'field-app ios' },
  { name: 'app-changes', session: 'invoices api', panel: 'changes' },
  { name: 'app-files', session: 'invoices api', panel: 'files' },
  { name: 'app-history', session: 'invoices api', history: 'invoices' },
  { name: 'app-continue', session: 'segmentation notes', panel: 'agents' },
  { name: 'app-usage', view: 'usage', wait: 12 },
  { name: 'app-subscriptions', view: 'subscriptions' },
  { name: 'app-skills', view: 'skills' },
  { name: 'app-settings', view: 'settings' },
  { name: 'app-devices', view: 'devices' },
];

async function captureWindow(ids) {
  for (const theme of ['light', 'dark']) {
    await setTheme(theme);
    for (const shot of WINDOW.filter(wanted)) {
      const file = path.join(OUT, `${shot.name}${theme === 'dark' ? '-dark' : ''}.png`);
      const env = {
        ...process.env, HOME, KIMO_DATA_DIR: DATA, KIMO_CAPTURE: file, KIMO_CAPTURE_WAIT: String(shot.wait || 5),
        KIMO_CAPTURE_VIEW: shot.view || 'terminals', KIMO_CAPTURE_SESSION: shot.session ? ids[shot.session] : '',
        KIMO_CAPTURE_PANEL: shot.panel || '', KIMO_CAPTURE_HISTORY: shot.history ? projectPath(shot.history) : '',
        PATH: `${BIN}:${process.env.PATH}`,
      };
      await new Promise(resolve => {
        const child = spawn(KIMO, ['--port', String(PORT), 'app'], { env, stdio: 'ignore' });
        const timer = setTimeout(() => child.kill(), 30_000);
        child.on('exit', () => { clearTimeout(timer); resolve(); });
      });
      console.log(`  ${path.relative(ROOT, file)}${fs.existsSync(file) ? '' : ' (not saved)'}`);
    }
  }
}

// ----- run -----

if (!fs.existsSync(KIMO)) {
  console.error('build kimo first: cargo build (the window is needed for window screenshots)');
  process.exit(1);
}
seedHome();
const kimo = await startKimo();
const stop = () => { kimo.kill(); };
process.on('exit', stop);
process.on('SIGINT', () => process.exit(0));
const ids = await seedKimo();
await startProxy();
console.log(`demo kimo with invented data: http://127.0.0.1:${PORT}/  (data in ${DIR})`);
if (capture) {
  fs.mkdirSync(OUT, { recursive: true });
  await sleep(4000); // the agents' activity settles
  if (capture === true || capture === 'phone') await capturePhone();
  if (capture === true || capture === 'window') await captureWindow(ids);
  await setTheme('system');
  process.exit(0);
}
