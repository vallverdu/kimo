// The machine at a glance, under the agents' figures: subscription use, keeping the computer
// awake, resources per project, open ports and the paired kimos. Each chip opens a sheet.
import { $, esc, mb, toast } from './ui.js';

const until = at => {
  const left = Math.max(0, at - Date.now() / 1000), d = Math.floor(left / 86400), h = Math.floor(left % 86400 / 3600), m = Math.floor(left % 3600 / 60);
  return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
};
const AWAKE = [
  ['on', 'On', 'Keep this computer awake all the time'],
  ['agents', 'While agents work', 'Stay awake while an agent is working, and two minutes after'],
  ['off', 'Off', 'The computer sleeps as usual'],
];
const awakeLabel = mode => ({ on: 'on', agents: 'while agents work', off: 'off' })[mode] || 'off';

/** The usage of an account (`system`: this computer's login of the provider). */
function usageOf(state, account) {
  return state.usage.find(u => u.provider === account.provider && (account.system ? !u.account : u.account === account.id));
}
/** The limit closest to running out. */
function worst(u) {
  return (u?.limits?.limits || []).reduce((w, l) => (!w || l.used_percent > w.used_percent ? l : w), null);
}
/** Sessions grouped by the project whose folder holds them, then "not in a project". */
function byProject(state) {
  const groups = state.projects.map(p => ({ name: p.name, sessions: [] }));
  const loose = { name: 'Not in a project', sessions: [] };
  for (const s of state.sessions) {
    const i = state.projects.findIndex(p => s.cwd === p.path || s.cwd.startsWith(`${p.path}/`));
    (i >= 0 ? groups[i] : loose).sessions.push(s);
  }
  return [...groups, loose].filter(g => g.sessions.length);
}

export function initMachine({ api, state, openSession, switchHost }) {
  let ports = null, open = null;
  const sheet = $('sheet-machine');

  async function loadPorts() {
    if (state.view !== 'agents' || document.visibilityState !== 'visible') return;
    ports = await api.ports().catch(() => null);
    render();
  }

  function chips() {
    const out = [];
    for (const provider of ['claude', 'codex']) {
      const active = state.accounts.find(a => a.provider === provider && a.active)
        || state.accounts.find(a => a.provider === provider && a.system);
      const l = active && worst(usageOf(state, active));
      if (l) out.push(['usage', `${provider === 'claude' ? 'Claude' : 'Codex'} ${Math.round(l.used_percent)}% · ${until(l.resets_at)}`, `${active.name}: ${l.name}`]);
    }
    const awake = state.status?.awake;
    if (awake?.supported) out.push(['awake', `${awake.active ? '●' : '○'} awake: ${awakeLabel(awake.mode)}`, 'keep this computer awake']);
    const running = state.sessions.filter(s => s.status === 'running' || s.status === 'paused');
    if (state.status) out.push(['resources', `${mb(state.status.agents_rss_bytes)} · ${running.length} running`, 'memory and CPU per project']);
    const n = ports?.filter(p => p.session).length;
    if (ports) out.push(['ports', `${n} port${n === 1 ? '' : 's'}`, `${ports.filter(p => p.session).length} from agents and terminals, ${ports.filter(p => !p.session).length} others`]);
    if (state.hosts?.length) out.push(['kimos', `${state.hosts.length + 1} kimos`, 'the kimos paired with this one']);
    return out;
  }

  function body(kind) {
    if (kind === 'usage') {
      const rows = state.accounts.map(a => {
        const l = worst(usageOf(state, a));
        return `<li><span><strong>${esc(a.name)}</strong>${a.active ? ' <small class="ui-badge">active</small>' : ''}<small>${esc(a.provider)}${l ? ` · ${esc(l.name)}` : ''}</small></span>
          <span>${l ? `${Math.round(l.used_percent)}% · ${esc(until(l.resets_at))}` : '<small>no limits known</small>'}</span></li>`;
      }).join('');
      return ['Subscriptions', `<ul class="k-machine-list">${rows}</ul>
        <p><a href="#usage" data-close>usage details</a> · <a href="#accounts" data-close>manage subscriptions</a></p>`];
    }
    if (kind === 'awake') {
      const mode = state.status?.awake?.mode || 'off';
      return ['Keep computer awake', `<div class="k-machine-choices">${AWAKE.map(([m, t, d]) =>
        `<button type="button" class="${m === mode ? 'active' : ''}" data-awake="${m}"><strong>${t}</strong><small>${d}</small></button>`).join('')}</div>`];
    }
    if (kind === 'resources') {
      const st = state.status;
      const cpu = state.sessions.reduce((n, s) => n + (s.usage?.cpu_percent || 0), 0);
      const groups = byProject(state).map(g => {
        const rss = g.sessions.reduce((n, s) => n + s.usage.rss_bytes, 0), gcpu = g.sessions.reduce((n, s) => n + s.usage.cpu_percent, 0);
        return `<li class="k-machine-group"><span><strong>${esc(g.name)}</strong></span><span>${gcpu.toFixed(1)}% · ${mb(rss)}</span></li>
          ${g.sessions.map(s => `<li><button type="button" class="k-name" data-session="${esc(s.id)}">${esc(s.name)}</button>
            <span><small>${esc(s.status)}</small> ${s.usage.cpu_percent.toFixed(1)}% · ${mb(s.usage.rss_bytes)}</span></li>`).join('')}`;
      }).join('');
      return ['Resources', `<p class="k-hint">${cpu.toFixed(1)}% CPU · agents ${mb(st.agents_rss_bytes)} · free ${mb(st.mem_available_bytes)} of ${mb(st.mem_total_bytes)} · kimo itself ${mb(st.kimo_rss_bytes)}</p>
        <ul class="k-machine-list">${groups || '<li>No terminals yet.</li>'}</ul>
        <p class="k-hint">Pause saves CPU and keeps memory; stop frees memory and the agent continues its conversation when started again.</p>`];
    }
    if (kind === 'ports') {
      const local = !api.host;
      const row = p => {
        const open = local && p.addresses.some(a => a === '*' || a === '0.0.0.0' || a === '::');
        const port = open ? `<a href="http://${esc(location.hostname)}:${p.port}" target="_blank" rel="noopener">${p.port}</a>` : p.port;
        return `<li><span><strong>${port}</strong> <small>${esc(p.process || 'unknown process')}</small></span><span><small>${esc(p.addresses.join(' '))}</small></span></li>`;
      };
      const mine = (ports || []).filter(p => p.session), others = (ports || []).filter(p => !p.session);
      const names = [...new Set(mine.map(p => p.session_name))];
      return ['Ports', `${names.map(n => `<h4>${esc(n)}</h4><ul class="k-machine-list">${mine.filter(p => p.session_name === n).map(row).join('')}</ul>`).join('') || '<p class="k-hint">No agent or terminal listens on a port.</p>'}
        <details><summary>Other ports (${others.length})</summary><ul class="k-machine-list">${others.map(row).join('')}</ul></details>`];
    }
    if (kind === 'kimos') {
      const rows = [{ id: '', name: `this kimo${state.status?.host && !api.host ? ` (${state.status.host})` : ''}`, online: true }, ...state.hosts].map(h =>
        `<li><button type="button" class="k-name" data-host="${esc(h.id)}">${(api.host || '') === h.id ? '● ' : ''}${esc(h.name)}</button><span><small>${h.online ? 'online' : 'offline'}</small></span></li>`).join('');
      return ['kimos', `<ul class="k-machine-list">${rows}</ul><p><a href="#devices" data-close>pair another kimo</a></p>`];
    }
    return ['', ''];
  }

  function render() {
    const list = chips();
    $('machine').innerHTML = list.map(([kind, text, title]) => `<li><button type="button" data-machine="${kind}" title="${esc(title)}">${esc(text)}</button></li>`).join('');
    if (open && sheet.open) {
      const [title, html] = body(open);
      sheet.querySelector('h3').textContent = title;
      sheet.querySelector('.k-machine-body').innerHTML = html;
    }
  }

  $('machine').addEventListener('click', e => {
    const kind = e.target.closest('[data-machine]')?.dataset.machine;
    if (!kind) return;
    open = kind;
    if (kind === 'ports') loadPorts();
    sheet.showModal();
    render();
  });
  sheet.addEventListener('close', () => { open = null; });
  sheet.addEventListener('click', async e => {
    if (e.target.closest('[data-close]')) return sheet.close();
    const mode = e.target.closest('[data-awake]')?.dataset.awake;
    if (mode) {
      try {
        const awake = await api.setAwake(mode);
        if (state.status) state.status.awake = awake;
        render();
        toast(`Keep awake: ${awakeLabel(mode)}`);
      } catch (err) { toast(err.message, true); }
      return;
    }
    const id = e.target.closest('[data-session]')?.dataset.session;
    if (id) { sheet.close(); return openSession(id); }
    const host = e.target.closest('[data-host]');
    if (host) { sheet.close(); switchHost(host.dataset.host || null); }
  });

  loadPorts();
  setInterval(loadPorts, 15000);
  return { render, reset() { ports = null; loadPorts(); } };
}
