// Demonstration data for the map: a stand-in for the SDK calls the map makes (status, tailnet,
// hosts, sessions, fleet devices, and each kimo's projects and node facts) over an invented
// world, so the map runs without a kimo, e.g. on the website. Pass it the world of
// scripts/demo/world.mjs, the same one the screenshots show. Agents start and finish work on
// every poll, and "wake" brings an off machine up a few polls later. Nothing here is real.

/** A stand-in for `new Kimo(...)` with the calls the map makes, over `world`. */
export function demoApi(world) {
  const { MACHINES, PROJECTS, SESSIONS, REMOTE_PROJECTS, REMOTE_SESSIONS } = world;
  const hub = MACHINES.find(m => m.hub);
  const byName = name => MACHINES.find(m => m.name === name || m.id === name);
  const sessions = [
    ...SESSIONS.map((s, i) => ({
      id: `s${i + 1}`, host: hub.name, name: s.name, kind: s.kind, status: 'running', cwd: `/Users/demo/dev/${s.project}`,
      activity: s.screen.includes('Allow command?') ? 'waiting' : s.screen.includes('esc to interrupt') || s.kind === 'shell' ? 'working' : 'idle',
    })),
    ...Object.entries(REMOTE_SESSIONS).flatMap(([host, list]) => list.map(s => ({ ...s, id: `${host}/${s.id}`, host }))),
  ];
  // The training job keeps gpu-box busy; the rest follow their agents.
  const settle = () => {
    for (const m of MACHINES) {
      if (['off', 'waking'].includes(m.state) || m.id === 'gpu-box') continue;
      const working = sessions.some(s => s.host === m.name && s.activity === 'working');
      m.state = working ? 'busy' : m.hub ? 'ready' : 'idle';
    }
  };
  let step = 0;

  // One poll later: an agent starts or finishes, waking machines come closer to up.
  function advance() {
    step += 1;
    const up = sessions.filter(s => s.kind !== 'shell' && s.activity !== 'waiting' && byName(s.host)?.state !== 'off');
    for (const s of new Set([up[(step * 5) % up.length], up[(step * 3 + 1) % up.length]])) {
      if (s) s.activity = s.activity === 'working' ? 'idle' : 'working';
    }
    for (const m of MACHINES) {
      if (m.state !== 'waking' || --m.wakeIn > 0) continue;
      m.facts ||= { os: 'macos', arch: 'aarch64', model: 'Mac16,12', cpus: 10, mem: 16, free: 9.2, load: 0.6, disk: [512, 301] };
      delete m.seen;
    }
    for (const m of MACHINES) if (m.state === 'waking' && m.wakeIn <= 0) m.state = 'ready';
    settle();
  }
  settle();

  const answers = m => (m && m.state !== 'off' && m.state !== 'waking' && m.facts ? Promise.resolve() : Promise.reject(new Error(`${m?.name} does not answer`)));
  const host = name => {
    const m = name ? byName(name) : hub;
    const projects = () => (m.hub
      ? PROJECTS.map(p => ({ id: p.id, name: p.name, path: `/Users/demo/dev/${p.name}`, branch: 'main', worktrees: [] }))
      : REMOTE_PROJECTS[m.id] || []);
    return {
      forHost: other => host(other),
      status: async () => ({ host: hub.name, version: world.FLEET_STATUS.version }),
      tailnet: async () => world.tailnet(),
      hosts: async () => world.peers(),
      request: async (method, path) => {
        if (path.startsWith('/sessions')) {
          advance();
          return sessions.filter(s => byName(s.host)?.state !== 'off').map(({ host: _, ...s }) => s);
        }
        throw new Error(`the demo has no ${method} ${path}`);
      },
      projects: () => answers(m).then(projects),
      node: () => answers(m).then(() => world.node(m)),
      fleet: {
        devices: async () => world.fleetDevices(),
        wake: async id => {
          const w = byName(id);
          if (w && w.state === 'off') Object.assign(w, { state: 'waking', wakeIn: 3 });
          return { ok: true };
        },
      },
    };
  };
  return host(null);
}
