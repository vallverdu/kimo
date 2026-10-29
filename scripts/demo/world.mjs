// An invented team of machines for screenshots, the website and demos. Nothing here is real:
// names, Tailscale addresses (100.101.x.x on demo.ts.net), LAN addresses, emails, projects,
// figures. The same world drives the phone and window screenshots (scripts/demo/run.mjs) and
// the website's 3D scene, so they tell one story.

export const TAILNET = 'demo.ts.net';
const now = Date.now();
const min = 60_000;

/** The machines. `hub` is the kimo the phone talks to (it also runs the fleet coordinator). */
export const MACHINES = [
  {
    id: 'studio', name: 'studio', os: 'macOS', hub: true, ip: '100.101.20.1', lan: '192.168.50.10',
    model: 'mac-mini', role: 'apple-builder', state: 'ready',
    facts: { os: 'macos', arch: 'aarch64', model: 'Mac16,10', cpus: 10, mem: 32, free: 18.4, load: 1.6, disk: [994, 412], xcode: '26.1' },
  },
  {
    id: 'gpu-box', name: 'gpu-box', os: 'linux', ip: '100.101.20.2', lan: '192.168.50.21', mac: '02:00:5e:10:20:21',
    model: 'nuc-11-extreme', role: 'gpu-worker', state: 'busy',
    facts: { os: 'linux', arch: 'x86_64', model: 'NUC11BTMi9', cpus: 16, mem: 32, free: 21.2, load: 7.8, disk: [916, 540],
      gpus: [{ name: 'NVIDIA GeForce RTX 3060', memory_total_mb: 12288, memory_used_mb: 9830, utilization_percent: 96, temperature_c: 71 }],
      nvidia_driver: '580.82.07', cuda: '13.0' },
  },
  {
    id: 'builder', name: 'builder', os: 'linux', ip: '100.101.20.3', lan: '192.168.50.22', mac: '02:00:5e:10:20:22',
    model: 'nuc-11-enthusiast', role: 'build-worker', state: 'idle',
    facts: { os: 'linux', arch: 'x86_64', model: 'NUC11PHi7', cpus: 8, mem: 16, free: 11.0, load: 0.4, disk: [480, 260],
      gpus: [{ name: 'NVIDIA GeForce RTX 2060', memory_total_mb: 6144, memory_used_mb: 310, utilization_percent: 0, temperature_c: 41 }],
      nvidia_driver: '580.82.07', cuda: '13.0' },
  },
  {
    id: 'vision-pi', name: 'vision-pi', os: 'linux', ip: '100.101.20.4',
    model: 'rpi5-hailo', role: 'agent-host', state: 'ready',
    facts: { os: 'linux', arch: 'aarch64', model: 'Raspberry Pi 5 Model B Rev 1.1', cpus: 4, mem: 8, free: 5.1, load: 0.9, disk: [256, 180], hailo: 'HAILO10H' },
  },
  {
    id: 'jetson', name: 'jetson', os: 'linux', ip: '100.101.20.5',
    model: 'jetson-orin-nano', role: 'edge-target', state: 'ready',
    facts: { os: 'linux', arch: 'aarch64', model: 'NVIDIA Jetson Orin Nano Developer Kit', cpus: 6, mem: 8, free: 4.2, load: 1.1, disk: [128, 70], jetson_l4t: '36.4.3' },
  },
  {
    id: 'watch-pi', name: 'watch-pi', os: 'linux', ip: '100.101.20.6',
    model: 'rpi4', role: 'spare', state: 'ready',
    facts: { os: 'linux', arch: 'aarch64', model: 'Raspberry Pi 4 Model B Rev 1.5', cpus: 4, mem: 4, free: 3.1, load: 0.1, disk: [64, 51] },
  },
  {
    id: 'travel-air', name: 'travel-air', os: 'macOS', ip: '100.101.20.7',
    model: 'macbook-air', role: 'apple-builder', state: 'off', seen: 3 * 3600_000,
    facts: null,
  },
];

/** Screens on the tailnet: they reach the fleet through the hub's kimo. */
export const SCREENS = [
  { id: 'iphone', name: 'iphone', host_name: 'iPhone', os: 'iOS', ip: '100.101.30.1', online: true },
  { id: 'ipad', name: 'ipad', host_name: 'iPad mini', os: 'iOS', ip: '100.101.30.2', online: true },
  { id: 'pixel', name: 'pixel', host_name: 'Pixel 8 Pro', os: 'android', ip: '100.101.30.3', online: false, seen: 26 * 3600_000 },
];

const ROLE_LABEL = {
  'gpu-worker': ['x86_64', 'linux', 'cuda'], 'build-worker': ['x86_64', 'linux', 'cuda'],
  'agent-host': ['arm64', 'linux', 'rpi', 'hailo-10h'], 'edge-target': ['arm64', 'linux', 'cuda', 'jetson', 'tensorrt'],
  'apple-builder': ['arm64', 'macos', 'xcode'], spare: ['arm64', 'linux', 'rpi'],
};
const GB = 1e9;

/** A machine's `/api/node`, as its kimo would report it. */
export function node(m) {
  const f = m.facts;
  if (!f) return null;
  return {
    host: m.name, version: '0.1.15', os: f.os, arch: f.arch, model: f.model,
    labels: ROLE_LABEL[m.role] || [f.arch === 'x86_64' ? 'x86_64' : 'arm64', f.os],
    nvidia_driver: f.nvidia_driver, cuda: f.cuda, jetson_l4t: f.jetson_l4t, hailo: f.hailo, xcode: f.xcode,
    cpus: f.cpus, load: [f.load, f.load * 0.9, f.load * 0.8], uptime_s: 86_400 * 3,
    mem_total_bytes: f.mem * GB, mem_available_bytes: f.free * GB,
    disk: { path: '/home/demo', total_bytes: f.disk[0] * GB, free_bytes: f.disk[1] * GB },
    gpus: f.gpus || [],
    nics: m.lan ? [{ name: f.os === 'macos' ? 'en0' : 'enp3s0', mac: m.mac || '02:00:5e:10:20:10', ipv4: m.lan, prefix: 24, broadcast: '192.168.50.255', wired: true, vpn: false }] : [],
    sessions: { total: 2, running: m.state === 'busy' ? 1 : 0, paused: 0, working: m.state === 'busy' ? 1 : 0, last_working_at: now - 2 * min, jobs_running: m.state === 'busy' ? 1 : 0 },
    logins: [], in_use: m.state === 'busy', awake: { mode: 'agents', active: m.state === 'busy', supported: true }, at: now,
  };
}

/** `tailscale status --json` on the hub. */
export function tailscaleStatus() {
  const dev = (d, self) => ({
    HostName: d.host_name || d.name, DNSName: `${d.name}.${TAILNET}.`, OS: d.os, TailscaleIPs: [d.ip],
    Online: self || d.online !== false && d.state !== 'off', Active: d.state === 'busy',
    LastSeen: d.seen ? new Date(now - d.seen).toISOString() : '0001-01-01T00:00:00Z', UserID: 1,
  });
  const hub = MACHINES.find(m => m.hub);
  const peers = {};
  for (const m of [...MACHINES.filter(x => !x.hub), ...SCREENS]) peers[m.id] = dev(m, false);
  return { Self: { ...dev(hub, true), UserID: 1 }, Peer: peers, CurrentTailnet: { Name: TAILNET } };
}

/** `/api/network/tailnet` on the hub. */
export function tailnet() {
  const out = [];
  for (const d of [...MACHINES, ...SCREENS]) {
    out.push({
      name: d.name, host_name: d.host_name || d.name, dns_name: `${d.name}.${TAILNET}`, ips: [d.ip], os: d.os,
      online: d.hub || (d.online ?? d.state !== 'off'), active: d.state === 'busy',
      last_seen: d.seen ? new Date(now - d.seen).toISOString() : undefined, self: !!d.hub, mine: true,
    });
  }
  return { tailscale: true, devices: out };
}

/** `/api/network` on the hub. */
export function network(port) {
  const hub = MACHINES.find(m => m.hub);
  return {
    addresses: [
      { https: false, kind: 'localhost', label: 'this machine', url: `http://localhost:${port}` },
      { https: true, kind: 'tailscale', label: 'Tailscale (HTTPS)', url: `https://${hub.name}.${TAILNET}` },
      { https: false, kind: 'tailscale', label: 'Tailscale', url: `http://${hub.ip}:${port}` },
      { https: false, kind: 'lan', label: 'Wi-Fi', url: `http://${hub.lan}:${port}` },
    ],
    port,
    tailscale: { dns_name: `${hub.name}.${TAILNET}`, https_url: `https://${hub.name}.${TAILNET}` },
  };
}

/** The kimos paired with the hub (`/api/peers`). */
export function peers() {
  return MACHINES.filter(m => !m.hub && m.facts).map(m => ({
    id: m.id, name: m.name, url: `http://${m.ip}:7777`, self_name: 'studio', online: m.state !== 'off', sessions: m.state === 'busy' ? 2 : 1,
  }));
}

/** Projects on other machines (the map reads them through /api/hosts/{id}/projects). */
export const REMOTE_PROJECTS = {
  'gpu-box': [{ id: 'segmentation', name: 'segmentation', path: '/home/demo/dev/segmentation', branch: 'kimo/train-v3', worktrees: [] }],
  builder: [{ id: 'invoices', name: 'invoices', path: '/home/demo/dev/invoices', branch: 'main', worktrees: [] }],
  'vision-pi': [{ id: 'counter', name: 'counter', path: '/home/demo/dev/counter', branch: 'main', worktrees: [] }],
  jetson: [{ id: 'edge-bench', name: 'edge-bench', path: '/home/demo/edge-bench', branch: 'main', worktrees: [] }],
  'watch-pi': [],
};
export const REMOTE_SESSIONS = {
  'gpu-box': [{ id: 'train-3f2a', name: 'segmentation train', kind: 'shell', cwd: '/home/demo/dev/segmentation', status: 'running', activity: 'working' }],
  builder: [{ id: 'tests-91bc', name: 'invoices tests', kind: 'codex', cwd: '/home/demo/dev/invoices', status: 'running', activity: 'idle' }],
  'vision-pi': [{ id: 'review-77d0', name: 'counter review', kind: 'claude', cwd: '/home/demo/dev/counter', status: 'running', activity: 'working' }],
  jetson: [],
  'watch-pi': [],
};

/** The coordinator's `/api/fleet/devices`. */
export function fleetDevices() {
  return MACHINES.map(m => {
    const facts = node(m);
    const registry = {
      kimo: `http://${m.ip}:7777`, mac: m.mac, broadcast: m.mac ? '192.168.50.255' : undefined,
      power: m.mac ? { wake: 'wol', sleep: 'shutdown', idle_minutes: 20, boot_deadline_s: 180, boots_per_day: 10 } : { wake: 'never', sleep: 'never', idle_minutes: 20, boot_deadline_s: 180, boots_per_day: 10 },
      labels: ROLE_LABEL[m.role] || [], resources: { gpu: facts?.gpus?.length || 0, cpu_slots: m.role === 'spare' ? 0 : 2 },
      cost: { 'gpu-worker': 40, 'build-worker': 18, 'agent-host': 3, 'edge-target': 5, 'apple-builder': 4, spare: 3 }[m.role],
      model: m.model, role: m.role,
    };
    return {
      id: m.id, kimo: registry.kimo, labels: [...new Set([...(registry.labels || []), m.role, ...(facts?.labels || [])])], registry,
      state: m.state, power: m.state, since: now - 40 * min, manual: false, never_auto_shutdown: false,
      shutdown_at: m.state === 'idle' ? now + 14 * min : undefined, last_seen: m.state === 'off' ? now - (m.seen || 0) : now - 5000,
      paired: true, can_wake: !!m.mac, auto_shutdown: !!m.mac, boots_today: m.mac ? 1 : 0,
      facts, jobs: m.id === 'gpu-box' ? ['j-train-v3'] : [], added: ['jetson', 'watch-pi'].includes(m.id),
    };
  });
}

const job = (id, task, state, extra = {}) => ({
  id, spec: { task, runner: extra.runner || 'command', requires: { labels: extra.labels || [] }, resources: {}, power: { wake_if_needed: true }, retries: 0, optional: false, show: extra.show || [] },
  state, workflow: extra.workflow, device: extra.device, attempt: extra.attempt || (state === 'ready' ? 0 : 1),
  reason: extra.reason, exit_code: extra.exit_code, created_at: now - (extra.ago || 30) * min, updated_at: now - (extra.updated || 2) * min,
  artifacts: extra.artifacts || {}, result: extra.result, evidence: extra.evidence || [],
});

/** The coordinator's jobs: a workflow from training to an approved release, and a few more. */
export function fleetJobs() {
  const w = 'w-seg-release';
  return [
    job('j-approve', 'approve-release', 'blocked', { runner: 'approval', workflow: w, show: ['REPORT.md'], ago: 50 }),
    job('j-analyse', 'analyse', 'blocked', { runner: 'agent', workflow: w, labels: ['agent-host'], ago: 50 }),
    job('j-bench-jetson', 'bench-jetson', 'blocked', { workflow: w, labels: ['jetson'], ago: 50 }),
    job('j-train-v3', 'train', 'running', { workflow: w, device: 'gpu-box', labels: ['cuda'], ago: 50, updated: 38 }),
    job('j-develop', 'develop-model', 'succeeded', { runner: 'agent', workflow: w, device: 'vision-pi', ago: 50, updated: 40, exit_code: 0,
      result: { result: 'Implemented the U-Net variant in models/seg and pushed kimo/develop-model.' } }),
    job('j-ios', 'ios-build', 'waiting_device', { labels: ['xcode', 'macbook'], reason: 'waiting for travel-air to be on', ago: 25 }),
    job('j-tests', 'invoices tests', 'succeeded', { device: 'builder', labels: ['x86_64'], ago: 90, updated: 80, exit_code: 0,
      artifacts: { 'report/junit.xml': { sha256: 'a'.repeat(64), bytes: 18_432 } } }),
    job('j-bench-pi', 'bench-hailo', 'succeeded', { device: 'vision-pi', labels: ['hailo-10h'], ago: 120, updated: 110, exit_code: 0 }),
  ];
}

export function fleetWorkflows() {
  return [{ id: 'w-seg-release', name: 'segmentation-to-ios', state: 'running', created_at: now - 50 * min,
    steps: [['develop-model', 'j-develop'], ['train', 'j-train-v3'], ['bench-jetson', 'j-bench-jetson'], ['analyse', 'j-analyse'], ['approve-release', 'j-approve']] }];
}

export function fleetEvents() {
  const e = (ago, device, job, kind, detail) => ({ id: Math.round(ago), at: now - ago * min, device, job, kind, detail });
  return [
    e(1, 'builder', null, 'power', 'switches off in 14 min if idle'),
    e(38, 'gpu-box', 'j-train-v3', 'job', 'running train (attempt 1)'),
    e(39, 'gpu-box', null, 'power', 'awake after 74 s'),
    e(41, 'gpu-box', null, 'power', 'magic packet sent to 192.168.50.255:9'),
    e(41, 'gpu-box', null, 'power', 'woken for work'),
    e(42, 'vision-pi', 'j-develop', 'job', 'develop-model succeeded'),
    e(25, null, 'j-ios', 'notice', 'ios-build waits: waiting for travel-air to be on'),
    e(80, 'builder', 'j-tests', 'job', 'invoices tests succeeded'),
  ].sort((a, b) => b.at - a.at);
}

export const FLEET_STATUS = { version: '0.1.15', host: 'studio', devices: MACHINES.length, registry: '~/.kimo-fleet/devices.yaml', jobs: { ready: 0, waiting_device: 1, running: 1, awaiting_approval: 0 } };

/** Plan limits for the subscriptions (`/api/usage`). */
export function usage() {
  const limit = (name, used, resetsH, pace) => ({ name, used_percent: used, resets_at: Math.round(now / 1000 + resetsH * 3600), pace: pace ? { projected_percent: pace } : null });
  const empty = { cache_read: 0, cache_write: 0, input: 0, output: 0 };
  return [
    { account: 'work', name: 'Work', provider: 'claude', tokens_5h: 1_840_000, tokens_7d: 21_400_000, messages_7d: 312, detail_7d: empty, last_activity: now - 2 * min,
      limits: { limits: [limit('5-hour', 34, 2.4, 61), limit('weekly', 48, 76, 82), limit('weekly Opus', 22, 76, 40)] } },
    { account: 'personal', name: 'Personal', provider: 'claude', tokens_5h: 0, tokens_7d: 3_100_000, messages_7d: 41, detail_7d: empty, last_activity: now - 26 * 3600_000,
      limits: { limits: [limit('5-hour', 0, 5, null), limit('weekly', 12, 112, 18)] } },
    { account: null, name: "this computer's login", provider: 'codex', tokens_5h: 620_000, tokens_7d: 8_900_000, messages_7d: 96, detail_7d: empty, last_activity: now - 6 * min,
      limits: { limits: [limit('5-hour', 18, 3.1, 40), limit('weekly', 27, 90, 55)] } },
  ];
}

export const PROJECTS = [
  { id: 'invoices', name: 'invoices', color: '#e86a5c' },
  { id: 'field-app', name: 'field-app', color: '#5c8ee8' },
  { id: 'segmentation', name: 'segmentation', color: '#5cb88a' },
];

/** What the hub's terminals show (ANSI), and what their agents are doing. */
const dim = s => `\x1b[2m${s}\x1b[0m`;
const bold = s => `\x1b[1m${s}\x1b[0m`;
const coral = s => `\x1b[38;5;209m${s}\x1b[0m`;
const green = s => `\x1b[32m${s}\x1b[0m`;
export const SESSIONS = [
  {
    name: 'invoices api', kind: 'claude', project: 'invoices',
    screen: [
      `${coral('✻')} ${bold('Claude Code')}  ${dim('~/dev/invoices')}`, '',
      `${bold('>')} Add tax rates per country to the invoice model, with tests`, '',
      `${coral('●')} I'll add a ${bold('TaxRate')} table and look rates up when an invoice is totalled.`, '',
      `${coral('●')} ${bold('Update')}(src/models/invoice.rs)`,
      `  ${dim('⎿')}  ${green('+ 18')} ${dim('lines: tax_rate(), totals with tax')}`,
      `${coral('●')} ${bold('Write')}(migrations/0007_tax_rates.sql)`,
      `${coral('●')} ${bold('Bash')}(cargo test invoice)`,
      `  ${dim('⎿')}  test invoice::totals_with_tax ... ${green('ok')}`,
      `     test invoice::unknown_country_is_an_error ... ${green('ok')}`,
      `     test result: ${green('ok')}. 24 passed; 0 failed`, '',
      `${coral('✻')} Writing the API docs for /invoices/{id}/totals… ${dim('(esc to interrupt)')}`,
    ].join('\r\n'),
  },
  {
    name: 'field-app ios', kind: 'codex', project: 'field-app',
    screen: [
      `${bold('codex')} ${dim('· gpt-5.3-codex · ~/dev/field-app')}`, '',
      `${bold('user')} The photo upload fails on slow networks. Make it resumable.`, '',
      `${bold('codex')} Uploads now go in 1 MB chunks with a resume token; a failed chunk is retried.`,
      `  ${dim('•')} Updated UploadService.swift and its tests`,
      `  ${dim('•')} xcodebuild test: 41 passed`, '',
      `${bold('Allow command?')} ${coral('git push origin kimo/resumable-upload')}`, '',
      `${coral('›')} 1. Yes`,
      `  2. Yes, and don't ask again for git push`,
      `  3. No, and tell Codex what to do differently`,
    ].join('\r\n'),
  },
  {
    name: 'segmentation notes', kind: 'claude', project: 'segmentation',
    screen: [
      `${coral('✻')} ${bold('Claude Code')}  ${dim('~/dev/segmentation')}`, '',
      `${bold('>')} Summarise the benchmark results once the Jetson run is in`, '',
      `${coral('●')} The training job is running on gpu-box (epoch 31 of 40). I'll compare`,
      `  results/jetson.json and results/pi5.json when the benchmarks finish.`, '',
      `${dim('>')} `,
    ].join('\r\n'),
  },
  {
    name: 'dev server', kind: 'shell', project: 'invoices',
    screen: [
      `${dim('~/dev/invoices $')} npm run dev`, '',
      `  ${green('➜')}  Local:   http://localhost:5173/`,
      `  ${green('➜')}  Network: http://192.168.50.10:5173/`,
      `  ${dim('ready in 412 ms')}`,
    ].join('\r\n'),
  },
];

/** Earlier conversations (for the usage logs and a project's history). */
export const CONVERSATIONS = [
  { project: 'invoices', agent: 'claude', prompt: 'Add tax rates per country to the invoice model, with tests', days: 0 },
  { project: 'field-app', agent: 'codex', prompt: 'The photo upload fails on slow networks. Make it resumable.', days: 0 },
  { project: 'invoices', agent: 'claude', prompt: 'Add PDF export for invoices with the company logo', days: 1 },
  { project: 'invoices', agent: 'claude', prompt: 'Why does the totals test fail on CI only?', days: 2 },
  { project: 'invoices', agent: 'codex', prompt: 'Refactor the customer search to use the new index', days: 4 },
  { project: 'invoices', agent: 'claude', prompt: 'Set up the invoices API with axum and SQLite', days: 9 },
  { project: 'field-app', agent: 'codex', prompt: 'Offline mode: queue edits and sync when back online', days: 3 },
  { project: 'field-app', agent: 'claude', prompt: 'Design the map screen for field visits', days: 6 },
  { project: 'segmentation', agent: 'claude', prompt: 'Implement the U-Net variant in models/seg', days: 1 },
  { project: 'segmentation', agent: 'codex', prompt: 'Export the model to ONNX and check opset 17', days: 5 },
];
