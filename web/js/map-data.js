// Network map data: the machines and phones on the tailnet, merged with what kimo knows about
// them (paired kimos, their facts, agents and projects) and the fleet coordinator's view
// (power state, jobs, registry), then laid out in zones. Pure: no I/O, no DOM.

export const norm = s => String(s || '').toLowerCase().replace(/[^a-z0-9]/g, '');
const base = path => String(path || '').replace(/\/+$/, '').split('/').pop() || path;
const hostOf = url => { try { return new URL(url).hostname; } catch { return ''; } };

// Phones by the name they give themselves: size (mm) and camera layout for the model.
const PHONES = [
  [/pixel.?8.?pro/, { size: [76.5, 162.6, 8.7], radius: 12, camera: 'bar' }],
  [/pixel/, { size: [70.8, 150.5, 8.9], radius: 12, camera: 'bar' }],
  [/redmi.?note.?10/, { size: [75.3, 161.8, 8.9], radius: 9, camera: 'rect' }],
  [/iphone.?12.?pro/, { size: [71.5, 146.7, 7.4], radius: 10, camera: 'square' }],
  [/iphone/, { size: [71.5, 146.7, 7.8], radius: 10, camera: 'square' }],
];

// The fleet's hardware models (kimo-fleet `models`) and how the map draws each one.
export const MODEL_KINDS = {
  'nuc-11-extreme': 'nucExtreme', 'nuc-11-enthusiast': 'nucEnthusiast', 'linux-gpu-pc': 'nucExtreme',
  'linux-pc': 'nucEnthusiast', 'jetson-orin-nano': 'jetson', 'rpi5-hailo': 'rpi5hat', rpi5: 'rpi5',
  rpi4: 'rpi4', 'macbook-air': 'macbookAir', 'mac-mini': 'macMini', mac: 'macbookAir',
};
// The fleet's roles, by name.
export const ROLE_NAMES = {
  'control-plane': 'Control plane', 'agent-host': 'Agent host', 'gpu-worker': 'GPU worker',
  'build-worker': 'Build worker', 'edge-target': 'Edge target', 'apple-builder': 'Apple builder', spare: 'Spare',
};

/**
 * Which model to draw: the fleet's hardware model when the machine has one, else from its
 * facts when its kimo reports them, else from its name and OS.
 */
export function kindOf({ name = '', os = '', facts, model: fleetModel }) {
  if (fleetModel && MODEL_KINDS[fleetModel]) return MODEL_KINDS[fleetModel];
  const model = `${facts?.model || ''}`.toLowerCase();
  const n = `${name} ${model}`.toLowerCase();
  const sys = os.toLowerCase();
  if (sys === 'ios') return /ipad/.test(n) ? 'ipadMini' : 'phone';
  if (sys === 'android') return /tab|pad/.test(n) ? 'ipadMini' : 'phone';
  if (facts?.jetson_l4t || /jetson|orin/.test(n)) return 'jetson';
  if (/zero/.test(n)) return 'zero2';
  if (/raspberry pi 5|rpi5|\bpi5/.test(n)) return facts?.hailo || /hat|hailo/.test(n) ? 'rpi5hat' : 'rpi5';
  if (/raspberry|rpi|\bpi\d?\b/.test(n)) return 'rpi4';
  if (/nuc11btm|nuc11bt|beast|extreme/.test(n)) return 'nucExtreme';
  if (sys === 'macos' || facts?.os === 'macos') return /mini|macmini/.test(n) ? 'macMini' : 'macbookAir';
  if (sys === 'windows') return 'macbookAir';
  return 'nucEnthusiast';
}

/** Which zone of the floor it belongs to. */
export function zoneOf(m) {
  const site = (m.labels || []).find(l => l.startsWith('site:'));
  if (site) return `site:${site.slice(5)}`;
  if (m.hub) return 'core';
  if (m.kind === 'phone' || m.kind === 'ipadMini') return 'screens';
  if (['rpi4', 'rpi5', 'rpi5hat', 'zero2'].includes(m.kind)) return 'boards';
  if (m.kind === 'macMini' || (m.kind === 'macbookAir' && m.os !== 'windows')) return 'apple';
  if (m.os === 'windows') return 'other';
  return 'compute';
}

function roleOf(m) {
  const role = m.fleet?.registry?.role;
  if (role) return ROLE_NAMES[role] || role;
  if (m.hub) return 'This kimo';
  if (m.kind === 'phone') return 'Phone';
  if (m.kind === 'ipadMini') return 'Tablet';
  const gpu = m.facts?.gpus?.[0]?.name?.replace(/^NVIDIA (GeForce )?/, '');
  if (gpu) return gpu;
  if (m.facts?.hailo) return m.facts.hailo;
  if (m.facts?.jetson_l4t) return 'Jetson';
  return '';
}

/** Projects with the agents in them, the busiest first. */
export function projectsOf(projects, sessions) {
  const out = (projects || []).map(p => ({
    name: p.name, path: p.path, branch: p.branch, roots: [p.path, ...(p.worktrees || []).map(w => w.path)], agents: [],
  }));
  for (const s of sessions || []) {
    const agent = { name: s.name, kind: s.kind, activity: s.status === 'running' ? s.activity || 'idle' : s.status };
    let p = out.find(x => x.roots.some(r => r && (s.cwd === r || s.cwd?.startsWith(`${r}/`))));
    if (!p) {
      p = out.find(x => !x.path && x.name === base(s.cwd));
      if (!p) out.push((p = { name: base(s.cwd), roots: [], agents: [] }));
    }
    p.agents.push(agent);
  }
  const rank = p => p.agents.filter(a => a.activity === 'working').length * 100 + p.agents.length;
  return out.sort((a, b) => rank(b) - rank(a) || a.name.localeCompare(b.name));
}

/**
 * Everything the map shows, keyed by machine.
 * @param {object} input
 * @param {{tailscale: boolean, devices: object[]}} [input.tailnet] GET /network/tailnet
 * @param {object[]} [input.peers] GET /peers: [{id, name, url, online, sessions}]
 * @param {object[]} [input.sessions] GET /sessions?all=1 (paired kimos' ids are "host/id")
 * @param {Map<string, {projects?: object[], facts?: object}>} [input.kimos] per machine id: its kimo's projects and /node facts
 * @param {object[]} [input.fleet] the coordinator's GET /fleet/devices, when there is one
 * @param {object} [input.status] this kimo's GET /status
 */
export function buildMachines({ tailnet, peers = [], sessions = [], kimos = new Map(), fleet = [], status } = {}) {
  const machines = new Map();
  const devices = (tailnet?.devices || []).filter(d => d.mine);
  for (const d of devices) {
    machines.set(d.name, {
      id: d.name, name: d.name, os: d.os, tailnet: d, hub: d.self,
      host_name: d.host_name, labels: [], peer: null, fleet: null,
    });
  }
  // Without Tailscale on this machine, the map still has this kimo.
  if (![...machines.values()].some(m => m.hub)) {
    const id = norm(status?.host) || 'this-kimo';
    machines.set(id, { id, name: status?.host || 'this kimo', os: '', tailnet: null, hub: true, labels: [], peer: null, fleet: null });
  }
  const byAddress = (url, ...names) => {
    const host = hostOf(url);
    for (const m of machines.values()) {
      const t = m.tailnet;
      if (t && (t.ips.includes(host) || t.dns_name === host || t.name === host.split('.')[0])) return m;
    }
    const wanted = names.map(norm).filter(Boolean);
    return [...machines.values()].find(m => wanted.includes(norm(m.id)) || wanted.includes(norm(m.host_name))) || null;
  };
  for (const p of peers) {
    const m = byAddress(p.url, p.name, p.id);
    if (m) m.peer = p;
  }
  for (const f of fleet || []) {
    let m = byAddress(f.registry?.kimo, f.id);
    if (!m) {
      // In the registry but not on the tailnet (yet).
      m = { id: f.id, name: f.id, os: '', tailnet: null, hub: false, labels: [], peer: null, fleet: null };
      machines.set(f.id, m);
    }
    m.fleet = f;
  }

  // Agents: this kimo's own, and "host/id" for paired ones.
  const hub = [...machines.values()].find(m => m.hub);
  const agentsOf = new Map();
  for (const s of sessions) {
    const slash = s.id?.indexOf('/') ?? -1;
    let m = hub;
    if (slash > 0) {
      const host = s.id.slice(0, slash);
      m = [...machines.values()].find(x => x.peer && (x.peer.name === host || x.peer.id === host)) || null;
    }
    if (m) agentsOf.set(m.id, [...(agentsOf.get(m.id) || []), s]);
  }

  for (const m of machines.values()) {
    const k = kimos.get(m.id) || {};
    m.facts = k.facts || m.fleet?.facts || null;
    m.labels = [...new Set([...(m.fleet?.labels || []), ...(m.facts?.labels || [])])];
    m.kind = kindOf({ name: `${m.name} ${m.host_name || ''}`, os: m.os || (m.facts?.os === 'macos' ? 'macOS' : m.facts?.os || ''), facts: m.facts, model: m.fleet?.registry?.model });
    if (m.kind === 'phone') {
      const spec = PHONES.find(([re]) => re.test(`${m.host_name} ${m.name}`.toLowerCase()));
      Object.assign(m, spec ? spec[1] : m.os === 'iOS' ? PHONES[4][1] : { size: [75, 160, 8.5], radius: 10, camera: 'rect' });
    }
    m.zone = zoneOf(m);
    m.role = roleOf(m);
    m.model = m.facts?.model || (m.host_name && m.host_name !== 'localhost' && norm(m.host_name) !== norm(m.name) ? m.host_name : '') || m.os || '';
    const agents = agentsOf.get(m.id) || [];
    m.agents = agents;
    m.projects = (k.projects || agents.length) ? projectsOf(k.projects || [], agents) : null;
    m.kimo = m.hub ? { online: true, version: status?.version } : m.peer ? { online: m.peer.online, sessions: m.peer.sessions } : null;
    const fleetJobs = (m.fleet?.jobs || []).length;
    m.working = agents.filter(s => s.activity === 'working').length + fleetJobs;
    m.status = statusOf(m);
  }
  return [...machines.values()];
}

function statusOf(m) {
  const t = m.tailnet;
  let s = t ? (t.online ? 'online' : 'off') : m.hub ? 'online' : 'unjoined';
  const f = m.fleet?.state;
  if (f === 'waking' || f === 'booted' || f === 'draining') s = 'waking';
  else if (f === 'unreachable') s = 'unreachable';
  else if (f === 'off' && !t?.online) s = 'off';
  else if (['ready', 'busy', 'idle'].includes(f)) s = 'online';
  if (m.kimo && !m.hub && m.kimo.online && s === 'off') s = 'online';
  return s === 'online' && m.working > 0 ? 'busy' : s;
}

export const ZONE_TITLES = { core: 'This kimo', compute: 'Compute', boards: 'Boards', apple: 'Apple', screens: 'Phones and tablets', other: 'Other' };
const ZONE_AT = { core: [0, 0], compute: [0, -2.3], boards: [3.0, 0.1], apple: [-3.0, 0.1], screens: [0, 2.1], other: [-3.6, -2.3] };
const ZONE_COLS = { core: 3, compute: 3, boards: 3, apple: 3, screens: 2, other: 3 };
const ZONE_STEP = { compute: 1.3, screens: 1.0 };

/** Floor positions (metres) and zone rectangles for the machines. */
export function layout(machines) {
  const groups = new Map();
  for (const m of machines) groups.set(m.zone, [...(groups.get(m.zone) || []), m]);
  const zones = [];
  let site = 0;
  for (const [zone, list] of groups) {
    list.sort((a, b) => (b.hub - a.hub) || a.name.localeCompare(b.name));
    const remote = zone.startsWith('site:');
    const cols = remote ? 2 : ZONE_COLS[zone] || 3;
    const step = ZONE_STEP[zone] || 0.9;
    const at = remote ? [6.2 + site++ * 2.2, -1.1] : ZONE_AT[zone] || [0, 0];
    const rows = Math.ceil(list.length / cols);
    const width = Math.min(cols, list.length);
    list.forEach((m, i) => {
      const c = i % cols;
      const r = Math.floor(i / cols);
      m.pos = [at[0] + (c - (width - 1) / 2) * step, at[1] + (r - (rows - 1) / 2) * step * 0.9];
      // Every other label sits higher, so neighbours' names do not overlap.
      m.lift = c % 2 ? 0.16 : 0;
      m.rot = remote || zone === 'core' ? -0.2 : zone === 'screens' ? -0.25 : 0.15 * ((i % 2) * 2 - 1);
    });
    const pad = Math.min(step / 2, 0.4) + 0.06;
    const xs = list.map(m => m.pos[0]);
    const zs = list.map(m => m.pos[1]);
    zones.push({
      id: zone,
      title: remote ? `Site · ${zone.slice(5)}` : ZONE_TITLES[zone] || zone,
      remote,
      from: [Math.min(...xs) - pad, Math.min(...zs) - pad],
      to: [Math.max(...xs) + pad, Math.max(...zs) + pad + 0.1],
    });
  }
  return zones;
}
