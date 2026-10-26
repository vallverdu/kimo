// kimo UI: agents, projects, accounts, devices and apps; sheets; live updates.
// Talks to kimo only through the SDK.
import { kimo as api, pairCode, setToken } from './api.js';
import { terminal } from './terminal.js';
import { $, esc, mb, shortPath, toast } from './ui.js';
import { initProjects } from './projects.js';
import { initDevices } from './devices.js';
import { initUsage } from './usage.js';

const state = { sessions: [], accounts: [], apps: [], projects: [], devices: [], network: null, status: null, usage: [], view: 'agents' };

const kTokens = n => n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n);
function usageText(u) {
  if (!u) return '';
  const limits = u.limits?.limits || [];
  if (limits.length) {
    return limits.map(l => {
      const left = Math.max(0, l.resets_at - Date.now() / 1000), d = Math.floor(left / 86400), h = Math.floor(left % 86400 / 3600);
      const window = l.window_minutes === 10080 ? 'weekly' : l.window_minutes === 300 ? '5-hour' : `${l.window_minutes / 60} h`;
      return `${Math.round(l.used_percent)}% of ${window} limit, resets in ${d ? `${d} d ${h} h` : `${h} h`}`;
    }).join(' · ');
  }
  return u.tokens_7d ? `${kTokens(u.tokens_5h)} tokens in 5 h · ${kTokens(u.tokens_7d)} in 7 days` : 'no usage through kimo in 7 days';
}
const VIEWS = ['agents', 'projects', 'usage', 'accounts', 'devices', 'apps'];

// ----- rendering -----

function accountName(id) {
  return id ? (state.accounts.find(a => a.id === id)?.name ?? id) : 'default login';
}

function accountOptions(provider, selected) {
  const options = [{ id: '', name: `default ${provider} login` }, ...state.accounts.filter(a => a.provider === provider)];
  return options.map(a => `<option value="${esc(a.id)}" ${a.id === (selected ?? '') ? 'selected' : ''}>${esc(a.name)}${a.signed_in === false ? ' (not signed in)' : ''}</option>`).join('');
}

function renderStats() {
  const s = state.status, list = state.sessions;
  const count = st => list.filter(x => x.status === st).length;
  const tiles = [
    [count('running'), 'running'],
    [count('paused'), 'paused'],
    [list.length - count('running') - count('paused'), 'stopped'],
    [s ? mb(s.agents_rss_bytes) : '–', 'agents memory'],
    [s ? mb(s.mem_available_bytes) : '–', 'free memory'],
    [s ? mb(s.kimo_rss_bytes) : '–', 'kimo itself'],
  ];
  $('stats').innerHTML = tiles.map(([v, l]) => `<div class="ui-stat"><strong>${esc(v)}</strong><span>${esc(l)}</span></div>`).join('');
  const low = s && s.mem_total_bytes && s.mem_available_bytes / s.mem_total_bytes < 0.15;
  const waiting = list.filter(x => x.activity === 'waiting');
  $('alerts').innerHTML = [
    waiting.length ? `<div class="ui-alert"><span>${waiting.length} agent${waiting.length === 1 ? ' needs' : 's need'} you: ${waiting.map(x => esc(x.name)).join(', ')}</span><button class="ui-button k-small" data-open-waiting="${esc(waiting[0].id)}">open</button></div>` : '',
    low ? `<div class="ui-alert"><span>Memory is low (${mb(s.mem_available_bytes)} free). Stop idle agents: they continue where they left off.</span></div>` : '',
    updateAlert(),
  ].join('');
}

// A new kimo version: install it from here (running agents restart and continue).
function updateAlert() {
  const r = state.release;
  if (!r?.available) return '';
  const notes = r.url ? ` <a href="${esc(r.url)}" target="_blank" rel="noopener">release notes</a>` : '';
  if (r.manual) return `<div class="ui-alert info"><span>kimo ${esc(r.latest)} is ready.${notes} ${esc(r.manual)}.</span></div>`;
  return `<div class="ui-alert info"><span>kimo ${esc(r.latest)} is ready. Running agents restart and continue their conversations.${notes}</span>
    ${state.updating ? `<span>${esc(state.updating)}</span>` : '<button class="ui-button k-small" data-do="update">update</button>'}</div>`;
}

async function installUpdate() {
  state.updating = 'Downloading…';
  render();
  try {
    await api.installUpdate();
    state.updating = 'Restarting…';
    render();
    await api.restartKimo();
    // The page reconnects to the new kimo by itself; reload once it answers.
    for (let i = 0; i < 120; i++) {
      await new Promise(r => setTimeout(r, 1000));
      const s = await api.status().catch(() => null);
      if (s && s.version !== state.release.current) return location.reload();
    }
    throw new Error('the new kimo did not come back: see ~/.kimo/kimo.log');
  } catch (e) {
    state.updating = null;
    render();
    toast(e.message, true);
  }
}

/** Light or dark, as chosen in Settings → Appearance (`system` follows the device). */
function applyTheme(theme) {
  const root = document.documentElement;
  if (theme === 'light' || theme === 'dark') root.dataset.theme = theme;
  else delete root.dataset.theme;
  const meta = document.querySelector('meta[name=theme-color]');
  if (meta) meta.content = getComputedStyle(root).getPropertyValue('--color-background').trim() || '#ffffff';
}

function sessionRow(s) {
  const agent = s.kind !== 'shell';
  const live = s.status === 'running' || s.status === 'paused';
  const ram = s.usage.rss_bytes, total = state.status?.mem_total_bytes || 1;
  const buttons = [
    `<button class="ui-button k-small" data-do="open">open</button>`,
    s.status === 'running' ? `<button class="ui-button ghost k-small" data-do="pause">pause</button>` : '',
    s.status === 'paused' ? `<button class="ui-button ghost k-small" data-do="resume">resume</button>` : '',
    live ? `<button class="ui-button ghost k-small" data-do="stop">stop</button>` : `<button class="ui-button ghost k-small" data-do="start">${s.started_once ? 'continue' : 'start'}</button>`,
    agent && !s.command ? `<select data-do="account" aria-label="Account">${accountOptions(s.kind, s.account)}</select>` : '',
    !live ? `<button class="ui-button k-small k-danger" data-do="delete">delete</button>` : '',
  ];
  return `<li data-id="${esc(s.id)}">
    <button class="k-name" data-do="open">${esc(s.name)}</button>
    <span class="ui-badge ${s.status}">${s.status}${s.status === 'exited' && s.exit_code != null ? ` ${s.exit_code}` : ''}</span>
    ${s.activity === 'waiting' ? '<span class="k-act waiting">needs you</span>' : s.activity === 'working' ? '<span class="k-act">working</span>' : ''}
    <div class="k-meta"><span>${esc(s.kind)}</span>${agent ? `<span>${esc(accountName(s.account))}</span>` : ''}<code title="${esc(s.cwd)}">${esc(shortPath(s.cwd))}</code><span title="kimo id: kimo send / read / wait">id <code>${esc(s.id)}</code></span>${s.conversation ? `<span title="the agent's own conversation id">resume <code>${esc(s.conversation.slice(0, 8))}…</code></span>` : ''}
      ${live ? `<span><span class="k-num">${mb(ram)}</span> ram</span><span><span class="k-num">${s.usage.cpu_percent}%</span> cpu</span><span><span class="k-num">${s.usage.processes}</span> procs</span>` : ''}</div>
    ${live ? `<div class="k-bar" title="share of machine memory"><span style="width:${Math.min(100, ram / total * 100).toFixed(1)}%"></span></div>` : ''}
    <div class="k-actions">${buttons.join('')}</div>
  </li>`;
}

function renderSessions() {
  // What needs attention first: waiting, working, other running, paused, then the rest.
  const rank = s => s.activity === 'waiting' ? 0 : s.activity === 'working' ? 1 : s.status === 'running' ? 2 : s.status === 'paused' ? 3 : 4;
  const sorted = [...state.sessions].sort((a, b) => rank(a) - rank(b));
  $('sessions').innerHTML = sorted.length
    ? sorted.map(sessionRow).join('')
    : '<li class="ui-empty">No sessions yet. Tap + to start an agent.</li>';
}

function renderAccounts() {
  const defaults = ['claude', 'codex'].map(p => ({ id: 'default', provider: p, name: `default ${p} login`, signed_in: null, sessions: state.sessions.filter(s => s.kind === p && !s.account).length }));
  const rows = [...defaults, ...state.accounts].map(a => {
    const targets = [{ id: 'default', name: `default ${a.provider} login` }, ...state.accounts.filter(b => b.provider === a.provider)]
      .filter(b => !(b.id === a.id && a.id !== 'default') && !(a.id === 'default' && b.id === 'default'));
    const signed = a.signed_in === null ? '' : `<span class="ui-badge ${a.signed_in ? 'running' : 'paused'}" ${a.email ? `title="${esc(a.email)}"` : ''}>${a.signed_in ? 'signed in' : 'not signed in'}</span>`;
    return `<li data-account="${esc(a.id)}" data-provider="${a.provider}">
      <span class="k-name" style="cursor:default">${esc(a.name)}</span>${signed || '<span></span>'}
      <div class="k-meta"><span>${a.provider}</span>${a.email ? `<span>${esc(a.email)}</span>` : ''}<span><span class="k-num">${a.sessions}</span> sessions</span>
        <span class="k-usage">${esc(usageText(state.usage.find(u => (u.account ?? 'default') === a.id && u.provider === a.provider)))}</span></div>
      <div class="k-actions">
        ${a.id !== 'default' ? `<button class="ui-button ${a.signed_in ? 'ghost ' : ''}k-small" data-do="login">sign in</button>` : ''}
        ${a.sessions && targets.length ? `<select data-do="move-target" aria-label="Move to">${targets.map(t => `<option value="${esc(t.id)}">${esc(t.name)}</option>`).join('')}</select><button class="ui-button ghost k-small" data-do="move">move sessions</button>` : ''}
        ${a.id !== 'default' && !a.sessions ? `<button class="ui-button k-small k-danger" data-do="delete-account">delete</button>` : ''}
      </div></li>`;
  });
  $('account-list').innerHTML = rows.join('');
}

function renderApps() {
  $('app-list').innerHTML = state.apps.length
    ? state.apps.map(a => `<li data-app="${esc(a.id)}">
      <span class="k-name" style="cursor:default">${esc(a.name)}</span><span class="ui-badge ${a.scope === 'control' ? 'running' : 'stopped'}">${esc(a.scope)}</span>
      <div class="k-meta">${a.origins.length ? a.origins.map(o => `<code>${esc(o)}</code>`).join('') : '<span>no browser origin (scripts and servers only)</span>'}<span>key ${esc(a.token_hint)}</span></div>
      <div class="k-actions"><button class="ui-button k-small k-danger" data-do="revoke">revoke</button></div></li>`).join('')
    : '<li class="ui-empty">No apps yet. Tap + to give a PWA its own key.</li>';
}

function render() {
  projectsView.render();
  renderStats();
  renderSessions();
  renderAccounts();
  renderApps();
  const current = terminal.current && state.sessions.find(s => s.id === terminal.current.id);
  if (current) terminal.update(current);
}

// ----- data -----

async function refresh() {
  try {
    [state.sessions, state.accounts, state.status, state.apps, state.projects, state.usage] = await Promise.all([
      api.sessions(), api.accounts(), api.status(), api.clients().catch(() => []), api.projects().catch(() => []),
      api.usage().catch(() => []),
    ]);
    render();
    return true;
  } catch (e) {
    if (e.status === 401) { if (!$('sheet-token').open) $('sheet-token').showModal(); }
    else toast(e.message, true);
    return false;
  }
}

// Live updates from /api/events; machine memory every few seconds.
let unsubscribe = null;
async function connect() {
  unsubscribe?.();
  if (!(await refresh())) return;
  // Owner-only extras: the colour scheme and new versions (app keys get a 403, ignored).
  api.settings().then(st => applyTheme(st.theme)).catch(() => {});
  api.release().then(r => { state.release = r; render(); }).catch(() => {});
  unsubscribe = api.subscribe(({ sessions, accounts }) => {
    state.sessions = sessions;
    state.accounts = accounts;
    render();
  });
}

async function run(promise, done) {
  try {
    const result = await promise;
    if (done) toast(typeof done === 'function' ? done(result) : done);
    await refresh();
    return result;
  } catch (e) {
    toast(e.message, true);
  }
}

const projectsView = initProjects({ api, state, run, terminal });
const devicesView = initDevices({ api, state, run });
const usageView = initUsage({ api, state, run });

// ----- views -----

function show(view) {
  state.view = VIEWS.includes(view) ? view : 'agents';
  for (const v of VIEWS) $(`view-${v}`).hidden = state.view !== v;
  document.querySelectorAll('[data-view]').forEach(a => a.classList.toggle('active', a.dataset.view === state.view));
  $('add').setAttribute('aria-label', { agents: 'New session', projects: 'New project', accounts: 'New account', devices: 'Pair a device', apps: 'New app' }[state.view]);
  $('add').hidden = state.view === 'usage';
  if (state.view === 'usage') usageView.load();
  if (state.view === 'devices') devicesView.load();
  else devicesView.hideQr();
}
addEventListener('hashchange', () => show(location.hash.slice(1)));

// ----- session actions -----

$('alerts').addEventListener('click', e => {
  if (e.target.closest('[data-do=update]')) return installUpdate();
  const id = e.target.closest('[data-open-waiting]')?.dataset.openWaiting;
  const s = id && state.sessions.find(x => x.id === id);
  if (s) terminal.open(s);
});

$('sessions').addEventListener('click', e => {
  const btn = e.target.closest('button[data-do]');
  const id = e.target.closest('li[data-id]')?.dataset.id;
  if (!btn || !id) return;
  const s = state.sessions.find(x => x.id === id);
  const action = btn.dataset.do;
  if (action === 'open') return terminal.open(s);
  if (action === 'delete') {
    if (confirm(`Delete session "${s.name}"? The conversation stays in the agent's history.`)) run(api.deleteSession(id), 'Deleted');
    return;
  }
  btn.disabled = true;
  run(api.act(id, action), { pause: 'Paused: no CPU', resume: 'Resumed', stop: 'Stopped: memory freed', start: 'Started' }[action]);
});

$('sessions').addEventListener('change', e => {
  if (e.target.dataset.do !== 'account') return;
  const id = e.target.closest('li[data-id]').dataset.id;
  e.target.disabled = true;
  run(api.setAccount(id, e.target.value || null), s => `Now on ${accountName(s.account)}`);
});

terminal.onToggle = s => s && run(api.act(s.id, s.status === 'paused' ? 'resume' : 'pause'));
terminal.onClose = refresh;

// ----- account actions -----

$('account-list').addEventListener('click', async e => {
  const btn = e.target.closest('button[data-do]');
  const li = e.target.closest('li[data-account]');
  if (!btn || !li) return;
  const id = li.dataset.account;
  if (btn.dataset.do === 'login') {
    const s = await run(api.login(id));
    if (s) terminal.open(s);
  } else if (btn.dataset.do === 'move') {
    const to = li.querySelector('[data-do=move-target]').value;
    btn.disabled = true;
    run(api.moveSessions(id, to === 'default' ? null : to), r => `Moved ${r.moved} session${r.moved === 1 ? '' : 's'}`);
  } else if (btn.dataset.do === 'delete-account') {
    if (confirm('Delete this account and its stored credentials?')) run(api.deleteAccount(id), 'Account deleted');
  }
});

// ----- apps -----

const appSheet = $('sheet-app'), keySheet = $('sheet-key');

$('app-list').addEventListener('click', e => {
  const btn = e.target.closest('button[data-do=revoke]');
  const id = e.target.closest('li[data-app]')?.dataset.app;
  if (btn && id && confirm('Revoke this key? The app stops working until it gets a new one.')) run(api.deleteClient(id), 'Key revoked');
});

appSheet.querySelector('form').addEventListener('submit', async e => {
  e.preventDefault();
  const f = e.target.elements;
  const origins = f.origins.value.split(/[\s,]+/).filter(Boolean);
  const app = await run(api.createClient({ name: f.name.value, origins, scope: f.scope.value }));
  if (!app) return;
  appSheet.close();
  e.target.reset();
  const snippet = `import { Kimo } from '${location.origin}/sdk/kimo.js';\n` +
    `const kimo = new Kimo({ url: '${location.origin}', token: '${app.token}' });\n` +
    `kimo.subscribe(({ sessions }) => console.log(sessions));`;
  $('key-token').value = app.token;
  $('key-snippet').value = snippet;
  keySheet.showModal();
});

keySheet.addEventListener('click', async e => {
  const target = e.target.closest('[data-copy]');
  if (!target) return;
  try { await navigator.clipboard.writeText($(target.dataset.copy).value); toast('Copied'); }
  catch { $(target.dataset.copy).select(); toast('Select and copy it by hand', true); }
});

// ----- sheets -----

const sessionSheet = $('sheet-session'), accountSheet = $('sheet-account');
const RECENT = 'kimo-folders';
const recent = () => { try { return JSON.parse(localStorage.getItem(RECENT)) || []; } catch { return []; } };

function syncKind() {
  const form = sessionSheet.querySelector('form');
  const kind = form.elements.kind.value;
  form.querySelector('[data-for=agent]').hidden = kind === 'shell';
  form.elements.account.innerHTML = kind === 'shell' ? '' : accountOptions(kind);
}

$('add').addEventListener('click', () => {
  if (state.view === 'accounts') return accountSheet.showModal();
  if (state.view === 'apps') return appSheet.showModal();
  if (state.view === 'projects') return projectsView.open();
  if (state.view === 'devices') return devicesView.showQr();
  const form = sessionSheet.querySelector('form');
  form.elements.project.innerHTML = '<option value="">any folder</option>' +
    state.projects.map(p => `<option value="${esc(p.path)}">${esc(p.name)}</option>`).join('');
  form.elements.cwd.value = recent()[0] || '';
  $('recent-folders').innerHTML = recent().map(f => `<option value="${esc(f)}">`).join('');
  syncKind();
  sessionSheet.showModal();
});
sessionSheet.querySelectorAll('[name=kind]').forEach(r => r.addEventListener('change', syncKind));
sessionSheet.querySelector('[name=project]').addEventListener('change', e => {
  if (e.target.value) sessionSheet.querySelector('[name=cwd]').value = e.target.value;
});
document.querySelectorAll('.ui-sheet [value=cancel]').forEach(b => b.addEventListener('click', () => b.closest('dialog').close()));

sessionSheet.querySelector('form').addEventListener('submit', async e => {
  e.preventDefault();
  const f = e.target.elements;
  const body = { name: f.name.value, kind: f.kind.value, cwd: f.cwd.value, account: f.account.value || null };
  const s = await run(api.createSession(body));
  if (!s) return;
  try { localStorage.setItem(RECENT, JSON.stringify([body.cwd, ...recent().filter(x => x !== body.cwd)].filter(Boolean).slice(0, 8))); } catch { /* ignore */ }
  sessionSheet.close();
  e.target.reset();
  terminal.open(s);
});

accountSheet.querySelector('form').addEventListener('submit', async e => {
  e.preventDefault();
  const f = e.target.elements;
  const acc = await run(api.createAccount(f.name.value, f.provider.value), 'Account added: sign in next');
  if (acc) { accountSheet.close(); e.target.reset(); }
});

$('sheet-token').querySelector('form').addEventListener('submit', e => {
  setToken(e.target.elements.token.value.trim());
  $('sheet-token').close();
  connect();
});

// ----- pairing this device (opened from a QR code) -----

function deviceName() {
  const ua = navigator.userAgent;
  if (/iPhone/.test(ua)) return 'iPhone';
  if (/iPad/.test(ua)) return 'iPad';
  if (/Android/.test(ua)) return /Mobile/.test(ua) ? 'Android phone' : 'Android tablet';
  if (/Mac/.test(ua)) return 'Mac';
  if (/Windows/.test(ua)) return 'Windows PC';
  return 'Linux computer';
}

const pairSheet = $('sheet-pair');
pairSheet.querySelector('form').addEventListener('submit', async e => {
  e.preventDefault();
  try {
    const device = await api.pair(pairCode, e.target.elements.name.value);
    setToken(device.token);
    pairSheet.close();
    toast(`Paired as ${device.name}`);
    connect();
  } catch (err) { toast(err.message, true); }
});

// ----- start -----

show(location.hash.slice(1));
document.querySelectorAll('[data-origin]').forEach(e => { e.textContent = location.origin; });
if (pairCode) {
  pairSheet.querySelector('[name=name]').value = deviceName();
  pairSheet.showModal();
} else {
  connect();
}
setInterval(async () => {
  if (document.visibilityState !== 'visible' || !unsubscribe) return;
  try { state.status = await api.status(); renderStats(); } catch { /* the event stream reports problems */ }
}, 5000);
if ('serviceWorker' in navigator && isSecureContext) navigator.serviceWorker.register('sw.js').catch(() => {});
