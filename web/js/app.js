// kimo UI: agents, projects, accounts, devices and apps; sheets; live updates.
// Talks to kimo only through the SDK.
import { kimo as api, pairCode, setToken } from './api.js';
import { terminal } from './terminal.js';
import { $, esc, mb, shortPath, toast } from './ui.js';
import { initProjects } from './projects.js';
import { initDevices } from './devices.js';
import { initUsage } from './usage.js';
import { initMachine } from './machine.js';

const state = { sessions: [], accounts: [], apps: [], projects: [], devices: [], network: null, status: null, usage: [], hosts: [], view: 'agents' };
const machine = initMachine({
  api, state,
  openSession(id) { const s = state.sessions.find(x => x.id === id); if (s) terminal.open(s); },
  switchHost(id) { $('host').value = id || ''; $('host').dispatchEvent(new Event('change')); },
});

const kTokens = n => n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n);
/** "3 d 4 h", "2 h 10 min", "5 min" until a unix-seconds time. */
function until(at) {
  const left = Math.max(0, at - Date.now() / 1000), d = Math.floor(left / 86400), h = Math.floor(left % 86400 / 3600), m = Math.floor(left % 3600 / 60);
  return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
}
const clock = at => new Date(at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
/** One plan limit: use, reset and where the pace leads. */
export function limitText(l) {
  const parts = [`${l.name} ${Math.round(l.used_percent)}%`];
  if (l.resets_at) parts.push(`resets in ${until(l.resets_at)}`);
  if (l.used_percent >= 100) parts.push('used up');
  else if (l.pace?.runs_out_at) parts.push(`at this pace runs out in ${until(l.pace.runs_out_at)} (${clock(l.pace.runs_out_at)})`);
  else if (l.pace) parts.push(`at this pace about ${Math.round(l.pace.projected_percent)}% by then`);
  return parts.join(' · ');
}
function usageText(u) {
  if (!u) return '';
  const limits = u.limits?.limits || [];
  if (limits.length) return limits.map(limitText).join('\n');
  if (u.limits_note) return u.limits_note;
  return u.tokens_7d ? `${kTokens(u.tokens_5h)} tokens in 5 h · ${kTokens(u.tokens_7d)} in 7 days` : 'no usage through kimo in 7 days';
}
const VIEWS = ['agents', 'projects', 'usage', 'accounts', 'devices', 'apps'];

// ----- rendering -----

/** An account's name; no id = this computer's own login of the provider. */
function accountName(id, provider) {
  const a = id ? state.accounts.find(a => a.id === id) : state.accounts.find(a => a.system && a.provider === provider);
  return a?.name ?? id ?? "this computer's login";
}

/**
 * Account choices of a provider (`system` = this computer's own login). With `active`, the
 * first choice is "the active subscription" (value ''), as for new agents.
 */
function accountOptions(provider, selected, { active = false } = {}) {
  const list = state.accounts.filter(a => a.provider === provider);
  if (!list.some(a => a.system)) list.unshift({ id: 'system', system: true, name: "this computer's login" });
  const current = list.find(a => a.active) || list.find(a => a.system);
  const options = [
    ...(active ? [{ value: '', name: `active subscription (${current?.name ?? 'none'})` }] : []),
    ...list.map(a => ({ value: a.system ? 'system' : a.id, name: a.name + (a.signed_in === false ? ' (not signed in)' : '') })),
  ];
  const chosen = selected ?? (active ? '' : 'system');
  return options.map(o => `<option value="${esc(o.value)}" ${o.value === chosen ? 'selected' : ''}>${esc(o.name)}</option>`).join('');
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
  const m = state.memory, low = m && m.pressure !== 'ok';
  const idleBytes = m ? list.filter(x => m.idle.includes(x.id)).reduce((n, x) => n + x.usage.rss_bytes, 0) : 0;
  const waiting = list.filter(x => x.activity === 'waiting');
  $('alerts').innerHTML = [
    waiting.length ? `<div class="ui-alert"><span>${waiting.length} agent${waiting.length === 1 ? ' needs' : 's need'} you: ${waiting.map(x => esc(x.name)).join(', ')}</span><button class="ui-button k-small" data-open-waiting="${esc(waiting[0].id)}">open</button></div>` : '',
    low ? `<div class="ui-alert"><span>${m.pressure === 'critical' ? 'Memory is almost full' : 'Memory is low'}: ${mb(m.available_bytes)} free of ${mb(m.total_bytes)}, agents use ${mb(m.agents_bytes)}. ${m.idle.length ? 'Idle agents continue their conversation when opened again.' : 'No agent is idle: stop or pause one you do not need.'}</span>${m.idle.length ? `<button class="ui-button k-small" data-do="free-memory">stop ${m.idle.length} idle (${mb(idleBytes)})</button>` : ''}</div>` : '',
    updateAlert(),
  ].join('');
  machine.render();
}

// A new kimo version: install it from here, or restart onto one installed already (running
// agents restart and continue).
function updateAlert() {
  const r = state.release;
  if (!r?.available && !r?.restart) return '';
  const notes = r.available && r.url ? ` <a href="${esc(r.url)}" target="_blank" rel="noopener">release notes</a>` : '';
  const auto = state.settings && !state.settings.updates?.auto && !(r.available && r.manual)
    ? '<button class="ui-button k-small" data-do="auto-update">update by itself from now on</button>' : '';
  const busy = state.updating ? `<span>${esc(state.updating)}</span>` : '';
  if (r.restart && !r.available) {
    return `<div class="ui-alert info"><span>kimo ${esc(r.installed)} is installed: restart kimo to use it. Running agents restart and continue their conversations.</span>
      ${busy || `<button class="ui-button k-small" data-do="restart-kimo">restart</button>${auto}`}</div>`;
  }
  if (r.manual) return `<div class="ui-alert info"><span>kimo ${esc(r.latest)} is ready.${notes} ${esc(r.manual)}.</span></div>`;
  return `<div class="ui-alert info"><span>kimo ${esc(r.latest)} is ready. Running agents restart and continue their conversations.${notes}</span>
    ${busy || `<button class="ui-button k-small" data-do="update">update</button>${auto}`}</div>`;
}

async function installUpdate(download = true) {
  const before = state.release.current;
  state.updating = download ? 'Downloading…' : 'Restarting…';
  render();
  try {
    if (download) await api.installUpdate();
    state.updating = 'Restarting…';
    render();
    await api.restartKimo();
    // The page reconnects to the new kimo by itself; reload once it answers.
    for (let i = 0; i < 120; i++) {
      await new Promise(r => setTimeout(r, 1000));
      const s = await api.status().catch(() => null);
      if (s && s.version !== before) return location.reload();
    }
    throw new Error('the new kimo did not come back: see ~/.kimo/kimo.log');
  } catch (e) {
    state.updating = null;
    render();
    toast(e.message, true);
  }
}

async function updateByItself() {
  try {
    const st = await api.settings();
    st.updates = { ...st.updates, auto: true };
    state.settings = await api.saveSettings(st);
    render();
    toast('kimo will update by itself when no agent is working');
  } catch (e) { toast(e.message, true); }
}

// New versions: the kimo checks every hour; read its answer now and then.
function checkRelease() {
  api.release().then(r => { state.release = r; render(); }).catch(() => {});
}

/** Light or dark, as chosen in Settings → Appearance (`system` follows the device). */
function applyTheme(theme) {
  const root = document.documentElement;
  if (theme === 'light' || theme === 'dark') root.dataset.theme = theme;
  else delete root.dataset.theme;
  const meta = document.querySelector('meta[name=theme-color]');
  if (meta) meta.content = getComputedStyle(root).getPropertyValue('--color-background').trim() || '#ffffff';
}

// Memory breakdowns opened in the list (session id → parts).
const parts = {};

// The memory report; announces agents kimo stopped to free memory since the last one.
async function loadMemory() {
  const m = await api.memory().catch(() => null);
  if (!m) return;
  const newest = Math.max(0, ...m.freed.map(f => f.at));
  if (state.seenFreed != null) {
    for (const f of m.freed.filter(f => f.at > state.seenFreed && f.reason !== 'asked')) {
      toast(`Stopped ${f.name} to free ${mb(f.bytes)} (${f.reason === 'idle' ? 'idle for a while' : 'memory was low'}). It continues when you open it.`);
    }
  }
  state.seenFreed = Math.max(newest, state.seenFreed ?? 0);
  state.memory = m;
  renderStats();
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
    ${s.freed && !live ? `<span class="k-act asleep" title="kimo stopped it to free ${mb(s.freed.bytes)} (${{ idle: 'idle for a while', asked: 'you asked' }[s.freed.reason] || 'memory was low'}); it continues when you open it">asleep</span>` : ''}
    <div class="k-meta"><span>${esc(s.kind)}</span>${agent ? `<span>${esc(accountName(s.account, s.kind))}</span>` : ''}<code title="${esc(s.cwd)}">${esc(shortPath(s.cwd))}</code><span title="kimo id: kimo send / read / wait">id <code>${esc(s.id)}</code></span>${s.conversation ? `<span title="the agent's own conversation id">resume <code>${esc(s.conversation.slice(0, 8))}…</code></span>` : ''}
      ${live ? `<span><button class="k-ram${state.memory?.heavy.includes(s.id) ? ' heavy' : ''}" data-do="memory" title="what uses the memory"><span class="k-num">${mb(ram)}</span> ram</button></span><span><span class="k-num">${s.usage.cpu_percent}%</span> cpu</span><span><span class="k-num">${s.usage.processes}</span> procs</span>` : ''}</div>
    ${parts[s.id] ? `<div class="k-parts">${parts[s.id].map((p, i) => `<span>${esc(i ? p.label : `${p.label} itself`)} <span class="k-num">${mb(p.bytes)}</span></span>`).join('')}</div>` : ''}
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
  const providers = [['claude', 'Claude'], ['codex', 'Codex']];
  $('account-list').innerHTML = providers.map(([provider, title]) => {
    const list = state.accounts.filter(a => a.provider === provider);
    const rows = list.map(a => {
      const u = state.usage.find(u => (a.system ? u.account == null : u.account === a.id) && u.provider === provider);
      const key = a.system ? 'system' : a.id;
      return `<li data-account="${esc(key)}" data-provider="${provider}" data-name="${esc(a.name)}" class="${a.active ? 'k-active' : ''}">
        <span class="k-name" style="cursor:default">${esc(a.name)}</span>
        <span>${a.active ? '<span class="ui-badge running">active</span>' : ''}${a.signed_in === false ? ' <span class="ui-badge paused">not signed in</span>' : ' <span class="ui-badge">✓ signed in</span>'}</span>
        <div class="k-meta">${a.system ? "<span>this computer's login</span>" : ''}${a.email && a.email !== a.name ? `<span>${esc(a.email)}</span>` : ''}<span><span class="k-num">${a.sessions}</span> agents</span></div>
        <div class="k-usage k-pace">${esc(usageText(u)).replace(/\n/g, '<br>')}</div>
        <div class="k-actions">
          ${a.active ? '' : '<button class="ui-button k-small" data-do="use">use</button>'}
          ${a.system ? '' : a.signed_in ? '<button class="ui-button ghost k-small" data-do="login" title="only if its login expired">sign in again</button>' : '<button class="ui-button k-small" data-do="login">sign in</button>'}
          ${!a.system && !a.sessions ? '<button class="ui-button k-small k-danger" data-do="delete-account">remove</button>' : ''}
        </div></li>`;
    });
    const limitsOff = provider === 'claude' && list.length && state.settings && !state.settings.claude_limits;
    return `<h3 class="ui-section-title">${title}</h3><ul class="k-sessions">${rows.join('') || `<li class="ui-empty">Not signed in to ${title} on this computer. Tap + to add an account.</li>`}</ul>
      ${limitsOff ? `<div class="ui-alert info"><span>See each Claude subscription's 5-hour and weekly use, and when it runs out at this pace.${navigator.platform.startsWith('Mac') ? '' : ' On a Mac, macOS asks once on the kimo machine to let kimo read Claude\'s login.'}</span><button class="ui-button k-small" data-do="claude-limits">show plan limits</button></div>` : ''}`;
  }).join('');
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
    loadMemory();
    return true;
  } catch (e) {
    // A kimo this one cannot reach with its key, or no longer paired: back to this one.
    if (api.host && (e.status === 401 || e.status === 404)) { useHost(null); return refresh(); }
    if (e.status === 401) {
      if (!$('sheet-token').open) {
        $('sheet-token').querySelector('[name=name]').value = deviceName();
        $('sheet-token').showModal();
      }
    }
    else toast(e.message, true);
    return false;
  }
}

// Live updates from /api/events; machine memory every few seconds.
let unsubscribe = null;
async function connect() {
  unsubscribe?.();
  if (!(await refresh())) return;
  // Owner-only extras: the colour scheme, new versions, other kimos (app keys get a 403).
  api.settings().then(st => { state.settings = st; applyTheme(st.theme); renderAccounts(); }).catch(() => {});
  loadHosts();
  checkRelease();
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
  if (e.target.closest('[data-do=restart-kimo]')) return installUpdate(false);
  if (e.target.closest('[data-do=auto-update]')) return updateByItself();
  if (e.target.closest('[data-do=free-memory]')) {
    return run(api.freeMemory(), r => `Stopped ${r.stopped.length} idle agent${r.stopped.length === 1 ? '' : 's'}: memory freed`).then(loadMemory);
  }
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
  if (action === 'memory') {
    if (parts[id]) { delete parts[id]; return renderSessions(); }
    return api.sessionMemory(id).then(p => { parts[id] = p; renderSessions(); }).catch(err => toast(err.message, true));
  }
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
  run(api.setAccount(id, e.target.value || null), s => `Now on ${accountName(s.account, s.kind)}`);
});

terminal.onToggle = s => s && run(api.act(s.id, s.status === 'paused' ? 'resume' : 'pause'));
terminal.onClose = refresh;

// ----- account actions -----

$('account-list').addEventListener('click', async e => {
  if (e.target.closest('[data-do=claude-limits]')) {
    const st = await api.settings().catch(() => null);
    if (!st) return;
    state.settings = await run(api.saveSettings({ ...st, claude_limits: true }), 'Claude plan limits on') || st;
    state.usage = await api.usage().catch(() => state.usage);
    return renderAccounts();
  }
  const btn = e.target.closest('button[data-do]');
  const li = e.target.closest('li[data-account]');
  if (!btn || !li) return;
  const { account: id, provider, name } = li.dataset;
  if (btn.dataset.do === 'login') {
    const s = await run(api.login(id));
    if (s) terminal.open(s);
  } else if (btn.dataset.do === 'use') {
    const title = provider === 'codex' ? 'Codex' : 'Claude';
    if (!confirm(`Use ${name} for new ${title} agents?`)) return;
    const others = state.accounts.filter(a => a.provider === provider && (a.system ? 'system' : a.id) !== id).reduce((n, a) => n + a.sessions, 0);
    const move = others > 0 && confirm(`Also move the ${others} ${title} agent${others === 1 ? '' : 's'} on other subscriptions to ${name}? Running ones restart and continue their conversation.`);
    btn.disabled = true;
    run(api.setActiveAccount(provider, id, { move }), r => `${name} is the active subscription${r.moved ? ` · moved ${r.moved} agents` : ''}`);
  } else if (btn.dataset.do === 'delete-account') {
    if (confirm(`Remove ${name}? kimo deletes its sign-in on this computer; conversations stay.`)) run(api.deleteAccount(id), 'Subscription removed');
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
  form.elements.account.innerHTML = kind === 'shell' ? '' : accountOptions(kind, '', { active: true });
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
  if (!acc) return;
  accountSheet.close();
  e.target.reset();
  const login = await run(api.login(acc.id));
  if (login) terminal.open(login);
});

// Not paired: type the code kimo shows (an installed app cannot scan its own QR code), or
// paste a key.
$('sheet-token').querySelector('form').addEventListener('submit', async e => {
  e.preventDefault();
  const f = e.target.elements;
  const code = f.code.value.trim(), key = f.token.value.trim();
  if (!code && !key) return toast('Type the code kimo shows in Devices', true);
  if (code) {
    try {
      const device = await api.pair(code, f.name.value.trim() || deviceName());
      setToken(device.token);
      toast(`Paired as ${device.name}`);
    } catch (err) { return toast(err.message, true); }
  } else {
    setToken(key);
  }
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

const standalone = matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;
const ios = /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);

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

// ----- installing as an app -----

let installPrompt = null;
addEventListener('beforeinstallprompt', e => { e.preventDefault(); installPrompt = e; renderInstall(); });
addEventListener('appinstalled', () => { installPrompt = null; renderInstall(); });
const INSTALL_SEEN = 'kimo-install-hidden';
function renderInstall() {
  let hidden = false;
  try { hidden = localStorage.getItem(INSTALL_SEEN) === '1'; } catch { /* ignore */ }
  const touch = matchMedia('(pointer: coarse)').matches;
  let text = '', button = '';
  if (standalone || hidden || !touch) text = '';
  else if (installPrompt) { text = 'Install kimo as an app on this phone.'; button = '<button class="ui-button k-small" data-do="install">install</button>'; }
  else if (ios) text = 'Use kimo as an app: tap Share → Add to Home Screen. Then open it and type the pairing code kimo shows in Devices.';
  else if (!isSecureContext) text = 'To install kimo as an app this phone needs its HTTPS address: on the kimo machine, Devices → enable HTTPS (Tailscale), then scan the QR code again.';
  else text = 'Use kimo as an app: browser menu → Install app (or Add to Home screen).';
  $('install').innerHTML = text ? `<div class="ui-alert info"><span>${esc(text)}</span>${button}<button class="ui-button ghost k-small" data-do="install-hide">hide</button></div>` : '';
}
$('install').addEventListener('click', async e => {
  if (e.target.closest('[data-do=install]') && installPrompt) {
    installPrompt.prompt();
    await installPrompt.userChoice.catch(() => null);
    installPrompt = null;
  }
  if (e.target.closest('[data-do=install-hide]')) { try { localStorage.setItem(INSTALL_SEEN, '1'); } catch { /* ignore */ } }
  renderInstall();
});

// ----- other kimos (paired with this one) -----

const HOST = 'kimo-host';
function useHost(id) {
  api.useHost(id);
  try { id ? localStorage.setItem(HOST, id) : localStorage.removeItem(HOST); } catch { /* ignore */ }
  document.title = id ? `kimo · ${state.hosts.find(h => h.id === id)?.name ?? id}` : 'kimo · agents';
}
async function loadHosts() {
  state.hosts = await api.hosts().catch(() => []);
  const pick = $('host-pick');
  pick.hidden = !state.hosts.length;
  if (!state.hosts.length) return;
  $('host').innerHTML = [`<option value="">this kimo${state.status?.host ? ` (${esc(state.status.host)})` : ''}</option>`,
    ...state.hosts.map(h => `<option value="${esc(h.id)}" ${h.id === api.host ? 'selected' : ''}>${esc(h.name)}${h.online ? '' : ' (offline)'}</option>`)].join('');
}
$('host').addEventListener('change', e => {
  terminal.close?.();
  useHost(e.target.value || null);
  connect();
  machine.reset();
});

// ----- start -----

state.hosts = [];
try { const saved = localStorage.getItem(HOST); if (saved) api.useHost(saved); } catch { /* ignore */ }
renderInstall();

show(location.hash.slice(1));
document.querySelectorAll('[data-origin]').forEach(e => { e.textContent = location.origin; });
if (pairCode) {
  pairSheet.querySelector('[name=name]').value = deviceName();
  // On iPhone an installed app keeps its own storage: install first, then pair inside it.
  if (ios && !standalone) {
    $('pair-install').hidden = false;
    $('pair-install').textContent = `To use kimo as an app: tap Share → Add to Home Screen, open kimo from there and type the code ${pairCode}. Or pair this browser:`;
  }
  pairSheet.showModal();
} else {
  connect();
}
// The kimo serving this page was updated (from the window, another phone or by itself):
// reload to get its new web app.
let servedBy = null;
async function followUpdates() {
  const local = api.host ? await api.forHost(null).status().catch(() => null) : state.status;
  if (!local?.version) return;
  if (servedBy && local.version !== servedBy) location.reload();
  servedBy = local.version;
}
setInterval(async () => {
  if (document.visibilityState !== 'visible' || !unsubscribe) return;
  try { state.status = await api.status(); renderStats(); } catch { /* the event stream reports problems */ }
  loadMemory();
  followUpdates();
}, 5000);
setInterval(() => { if (document.visibilityState === 'visible' && unsubscribe) checkRelease(); }, 30 * 60 * 1000);
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && unsubscribe) checkRelease(); });
if ('serviceWorker' in navigator && isSecureContext) navigator.serviceWorker.register('sw.js').catch(() => {});
