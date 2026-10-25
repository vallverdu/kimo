// kimo UI: agents and accounts views, sheets, polling. Talks to the server only through `api`.
const esc = v => String(v ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const $ = id => document.getElementById(id);

function toast(message, isError = false) {
  let el = document.querySelector('.ui-toast');
  if (!el) { el = document.createElement('div'); el.className = 'ui-toast'; el.setAttribute('role', 'status'); document.body.append(el); }
  el.textContent = message;
  el.classList.toggle('error', isError);
  el.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { el.hidden = true; }, 2800);
}

const mb = bytes => bytes >= 1 << 30 ? `${(bytes / (1 << 30)).toFixed(1)} GB` : `${Math.round(bytes / (1 << 20))} MB`;
const home = path => path.replace(/^\/home\/[^/]+/, '~');
const state = { sessions: [], accounts: [], status: null, view: 'agents' };

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
  $('alerts').innerHTML = low
    ? `<div class="ui-alert"><span>Memory is low (${mb(s.mem_available_bytes)} free). Stop idle agents: they continue where they left off.</span></div>`
    : '';
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
    <div class="k-meta"><span>${esc(s.kind)}</span>${agent ? `<span>${esc(accountName(s.account))}</span>` : ''}<code>${esc(home(s.cwd))}</code>
      ${live ? `<span><span class="k-num">${mb(ram)}</span> ram</span><span><span class="k-num">${s.usage.cpu_percent}%</span> cpu</span><span><span class="k-num">${s.usage.processes}</span> procs</span>` : ''}</div>
    ${live ? `<div class="k-bar" title="share of machine memory"><span style="width:${Math.min(100, ram / total * 100).toFixed(1)}%"></span></div>` : ''}
    <div class="k-actions">${buttons.join('')}</div>
  </li>`;
}

function renderSessions() {
  $('sessions').innerHTML = state.sessions.length
    ? state.sessions.map(sessionRow).join('')
    : '<li class="ui-empty">No sessions yet. Tap + to start an agent.</li>';
}

function renderAccounts() {
  const defaults = ['claude', 'codex'].map(p => ({ id: 'default', provider: p, name: `default ${p} login`, signed_in: null, sessions: state.sessions.filter(s => s.kind === p && !s.account).length }));
  const rows = [...defaults, ...state.accounts].map(a => {
    const targets = [{ id: 'default', name: `default ${a.provider} login` }, ...state.accounts.filter(b => b.provider === a.provider)]
      .filter(b => !(b.id === a.id && a.id !== 'default') && !(a.id === 'default' && b.id === 'default'));
    const signed = a.signed_in === null ? '' : `<span class="ui-badge ${a.signed_in ? 'running' : 'paused'}">${a.signed_in ? 'signed in' : 'not signed in'}</span>`;
    return `<li data-account="${esc(a.id)}" data-provider="${a.provider}">
      <span class="k-name" style="cursor:default">${esc(a.name)}</span>${signed || '<span></span>'}
      <div class="k-meta"><span>${a.provider}</span><span><span class="k-num">${a.sessions}</span> sessions</span></div>
      <div class="k-actions">
        ${a.id !== 'default' ? `<button class="ui-button ${a.signed_in ? 'ghost ' : ''}k-small" data-do="login">sign in</button>` : ''}
        ${a.sessions && targets.length ? `<select data-do="move-target" aria-label="Move to">${targets.map(t => `<option value="${esc(t.id)}">${esc(t.name)}</option>`).join('')}</select><button class="ui-button ghost k-small" data-do="move">move sessions</button>` : ''}
        ${a.id !== 'default' && !a.sessions ? `<button class="ui-button k-small k-danger" data-do="delete-account">delete</button>` : ''}
      </div></li>`;
  });
  $('accounts').innerHTML = rows.join('');
}

function render() {
  renderStats();
  renderSessions();
  renderAccounts();
  const current = terminal.current && state.sessions.find(s => s.id === terminal.current.id);
  if (current) terminal.update(current);
}

// ----- data -----

async function refresh() {
  try {
    [state.sessions, state.accounts, state.status] = await Promise.all([api.sessions(), api.accounts(), api.status()]);
    render();
  } catch (e) {
    if (e instanceof api.AuthError) { if (!$('sheet-token').open) $('sheet-token').showModal(); }
    else toast(`Cannot reach kimo: ${e.message}`, true);
  }
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

// ----- views -----

function show(view) {
  state.view = view === 'accounts' ? 'accounts' : 'agents';
  $('view-agents').hidden = state.view !== 'agents';
  $('view-accounts').hidden = state.view !== 'accounts';
  document.querySelectorAll('[data-view]').forEach(a => a.classList.toggle('active', a.dataset.view === state.view));
  $('add').setAttribute('aria-label', state.view === 'agents' ? 'New session' : 'New account');
}
addEventListener('hashchange', () => show(location.hash.slice(1)));

// ----- session actions -----

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

$('accounts').addEventListener('click', async e => {
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
  const form = sessionSheet.querySelector('form');
  form.elements.cwd.value = recent()[0] || '';
  $('recent-folders').innerHTML = recent().map(f => `<option value="${esc(f)}">`).join('');
  syncKind();
  sessionSheet.showModal();
});
sessionSheet.querySelectorAll('[name=kind]').forEach(r => r.addEventListener('change', syncKind));
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
  const acc = await run(api.createAccount({ name: f.name.value, provider: f.provider.value }), 'Account added: sign in next');
  if (acc) { accountSheet.close(); e.target.reset(); }
});

$('sheet-token').querySelector('form').addEventListener('submit', e => {
  api.setToken(e.target.elements.token.value.trim());
  $('sheet-token').close();
  refresh();
});

// ----- start -----

show(location.hash.slice(1));
refresh();
setInterval(() => { if (document.visibilityState === 'visible') refresh(); }, 2000);
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') refresh(); });
if ('serviceWorker' in navigator && isSecureContext) navigator.serviceWorker.register('sw.js').catch(() => {});
