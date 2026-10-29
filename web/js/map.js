// Map view: the machines and phones on the tailnet in 3D, to scale, linked to this kimo, with
// what each one runs. Live: polls kimo every few seconds while the view is open; a pulse runs
// out when work starts on a machine and back when it ends. Loaded only when the view opens.
import * as THREE from '../vendor/three/three.module.min.js';
import { OrbitControls } from '../vendor/three/OrbitControls.js';
import { CSS2DRenderer, CSS2DObject } from '../vendor/three/CSS2DRenderer.js';
import { BUILDERS, M, MM, applyPalette, paintedTexture, repaint } from './map-models.js';
import { buildMachines, layout } from './map-data.js';
import { $, esc, mb } from './ui.js';

const STATUS = {
  online: ['Online', '--color-good'],
  busy: ['Working', '--color-accent'],
  waking: ['Waking', '--color-warn'],
  off: ['Off', '--color-muted'],
  unreachable: ['Unreachable', '--color-bad'],
  unjoined: ['Not on the tailnet', '--color-muted'],
};
const isUp = s => s === 'online' || s === 'busy';
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;
const pref = (key, fallback) => { try { return localStorage.getItem(`kimo-map-${key}`) ?? fallback; } catch { return fallback; } };
const keep = (key, value) => { try { localStorage.setItem(`kimo-map-${key}`, value); } catch { /* private mode */ } };
const ago = iso => {
  const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
  return s < 3600 ? `${Math.round(s / 60)} min ago` : s < 86400 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} days ago`;
};

export function initMap({ api, addToFleet = null }) {
  const local = api.forHost(null);
  const stage = $('map-stage');

  // ----- renderer, camera, controls -----
  const renderer = new THREE.WebGLRenderer({ antialias: true });
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  stage.appendChild(renderer.domElement);
  const labels = new CSS2DRenderer();
  labels.domElement.className = 'k-map-labels';
  stage.appendChild(labels.domElement);

  const scene = new THREE.Scene();
  scene.fog = new THREE.Fog(0xffffff, 14, 40);
  const camera = new THREE.PerspectiveCamera(34, 1, 0.02, 120);
  camera.position.set(0, 6, 7);
  const controls = new OrbitControls(camera, labels.domElement);
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.maxPolarAngle = 1.42;
  controls.minDistance = 0.25;
  controls.maxDistance = 30;
  controls.screenSpacePanning = true;

  scene.add(new THREE.HemisphereLight(0xffffff, 0xbfbfbf, 2.1));
  const sun = new THREE.DirectionalLight(0xffffff, 1.7);
  sun.position.set(4, 10, 6);
  sun.castShadow = true;
  sun.shadow.mapSize.set(2048, 2048);
  Object.assign(sun.shadow.camera, { left: -10, right: 12, top: 9, bottom: -9, near: 1, far: 40 });
  sun.shadow.bias = -0.0004;
  scene.add(sun);

  const shadowMat = new THREE.ShadowMaterial({ opacity: 0.12 });
  const floor = new THREE.Mesh(new THREE.PlaneGeometry(60, 60), shadowMat);
  floor.rotation.x = -Math.PI / 2;
  floor.receiveShadow = true;
  scene.add(floor);
  const gridMinor = new THREE.LineBasicMaterial({ transparent: true, opacity: 0.07 });
  const gridMajor = new THREE.LineBasicMaterial({ transparent: true, opacity: 0.2 });
  for (const [step, mat] of [[0.1, gridMinor], [1, gridMajor]]) {
    const pts = [];
    for (let v = -12; v <= 14 + 1e-6; v += step) pts.push(v, 0, -9, v, 0, 9);
    for (let v = -9; v <= 9 + 1e-6; v += step) pts.push(-12, 0, v, 14, 0, v);
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute(pts, 3));
    const lines = new THREE.LineSegments(g, mat);
    lines.position.y = 0.0004;
    scene.add(lines);
  }

  // Everything that depends on which machines exist; rebuilt when that changes.
  const world = new THREE.Group();
  scene.add(world);

  const zoneLine = new THREE.LineBasicMaterial();
  const zoneDashed = new THREE.LineDashedMaterial({ dashSize: 0.05, gapSize: 0.035 });
  const hubLine = new THREE.LineBasicMaterial();
  const selectLine = new THREE.LineBasicMaterial();
  const linkSolid = new THREE.LineBasicMaterial({ transparent: true, opacity: 0.6 });
  const linkDashed = new THREE.LineDashedMaterial({ dashSize: 0.05, gapSize: 0.05, transparent: true, opacity: 0.45 });
  const linkSelected = new THREE.LineBasicMaterial();
  const pulseGeo = new THREE.SphereGeometry(0.013, 14, 10);
  const pulseMats = { out: new THREE.MeshBasicMaterial(), back: new THREE.MeshBasicMaterial(), you: new THREE.MeshBasicMaterial() };
  let palette = null;

  function floorText(text, heightM) {
    const w = 1024;
    const h = 110;
    const spaced = text.toUpperCase().split('').join(' ');
    const tex = paintedTexture(w, h, (c, cw, ch, p) => {
      c.clearRect(0, 0, cw, ch);
      c.fillStyle = p.ink;
      c.font = '400 72px "Roboto Mono", ui-monospace, monospace';
      c.textBaseline = 'top';
      c.fillText(spaced, 0, 12);
    });
    const mat = new THREE.MeshBasicMaterial({ map: tex, transparent: true, depthWrite: false, toneMapped: false });
    const m = new THREE.Mesh(new THREE.PlaneGeometry(heightM * (w / h), heightM), mat);
    m.rotation.x = -Math.PI / 2;
    return m;
  }

  // ----- machines -----
  const nodes = new Map(); // id → { m, root, model, label, link, sel, size, top }
  let hub = null;
  let selected = null;

  function place(m) {
    const root = new THREE.Group();
    root.position.set(m.pos[0], 0, m.pos[1]);
    root.rotation.y = m.rot || 0;
    root.userData.id = m.id;
    const model = BUILDERS[m.kind](m);
    const box = new THREE.Box3().setFromObject(model);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const screen = m.kind === 'phone' || m.kind === 'ipadMini';
    const padH = screen ? 0 : 0.008;
    model.scale.setScalar(MM);
    model.position.set(-center.x * MM, padH, -center.z * MM);
    root.add(model);

    let sel;
    if (!screen) {
      const margin = THREE.MathUtils.clamp(Math.max(size.x, size.z) * MM * 0.22, 0.025, 0.06);
      const pw = size.x * MM + margin;
      const pd = size.z * MM + margin;
      const geo = new THREE.BoxGeometry(pw, padH, pd);
      const pad = new THREE.Mesh(geo, M.paper);
      pad.position.y = padH / 2;
      pad.receiveShadow = true;
      pad.add(new THREE.LineSegments(new THREE.EdgesGeometry(geo), M.ink));
      root.add(pad);
      const outline = (mat, grow) => {
        const l = new THREE.LineSegments(new THREE.EdgesGeometry(new THREE.PlaneGeometry(pw + grow, pd + grow)), mat);
        l.rotation.x = -Math.PI / 2;
        l.position.y = 0.001;
        root.add(l);
        return l;
      };
      if (m.hub) { outline(hubLine, 0.03); outline(hubLine, 0.06); }
      sel = outline(selectLine, m.hub ? 0.09 : 0.025);
      sel.visible = false;
    }
    const top = size.y * MM + padH;
    const anchor = new THREE.Object3D();
    anchor.position.set(0, top + 0.04 + (m.lift || 0), 0);
    root.add(anchor);

    const el = document.createElement('button');
    el.type = 'button';
    el.className = `k-map-tag${m.hub ? ' hub' : ''}`;
    el.innerHTML = '<i class="k-dot"></i><span class="k-map-tag-name"></span><span class="k-map-tag-role"></span>';
    el.addEventListener('click', e => { e.stopPropagation(); select(m.id, true); });
    const label = new CSS2DObject(el);
    label.position.set(0, 0.05, 0);
    anchor.add(label);

    world.add(root);
    const node = { m, root, model, label: el, sel, size, top, screen, link: null };
    nodes.set(m.id, node);
    return node;
  }

  function worldTop(n, lift = 0) {
    n.root.updateMatrixWorld(true);
    return n.root.localToWorld(new THREE.Vector3(0, n.top + lift, 0));
  }

  function build(machines, zones) {
    for (const n of nodes.values()) n.label.remove();
    world.clear();
    nodes.clear();
    for (const z of zones) {
      const [x0, z0] = z.from;
      const [x1, z1] = z.to;
      const g = new THREE.BufferGeometry().setFromPoints([[x0, z0], [x1, z0], [x1, z1], [x0, z1], [x0, z0]].map(([x, zz]) => new THREE.Vector3(x, 0.001, zz)));
      const line = new THREE.Line(g, z.remote ? zoneDashed : zoneLine);
      if (z.remote) line.computeLineDistances();
      world.add(line);
      const title = floorText(z.title, 0.12);
      title.position.set(x0 + 0.04 + title.geometry.parameters.width / 2, 0.002, z1 - 0.04 - title.geometry.parameters.height / 2);
      world.add(title);
    }
    machines.forEach(place);
    hub = [...nodes.values()].find(n => n.m.hub);
    for (const n of nodes.values()) {
      if (n === hub) continue;
      const a = worldTop(hub, 0.01);
      const b = worldTop(n, 0.01);
      const mid = a.clone().lerp(b, 0.5);
      mid.y += 0.15 + a.distanceTo(b) * 0.22;
      const curve = new THREE.QuadraticBezierCurve3(a, mid, b);
      const line = new THREE.Line(new THREE.BufferGeometry().setFromPoints(curve.getPoints(80)), linkSolid);
      line.computeLineDistances();
      world.add(line);
      n.link = { curve, line };
    }
    if (selected) selected = nodes.get(selected.m.id) || null;
    for (const n of nodes.values()) refresh(n);
    if (selected) select(selected.m.id, false);
  }

  function refresh(n) {
    const s = n.m.status;
    n.model.userData.ledMat.color.set(palette.status[s] || palette.muted);
    n.label.dataset.status = s;
    n.label.querySelector('.k-map-tag-name').textContent = n.m.name;
    n.label.querySelector('.k-map-tag-role').textContent = n.screen ? '' : n.m.role || '';
    n.label.title = `${n.m.name} · ${STATUS[s]?.[0] || s}`;
    if (n.link) n.link.line.material = n === selected ? linkSelected : isUp(s) ? linkSolid : linkDashed;
  }

  // ----- details -----
  function renderPanel() {
    const panel = $('map-panel');
    if (!selected) {
      panel.innerHTML = '<p class="k-hint">Tap a machine to see what it runs.</p>';
      return;
    }
    const m = selected.m;
    const t = m.tailnet;
    const f = m.facts;
    const rows = pairs => pairs.filter(([, v]) => v).map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join('');
    const gpus = (f?.gpus || []).map(g => `${g.name.replace(/^NVIDIA (GeForce )?/, '')} · ${Math.round(g.memory_used_mb / 102.4) / 10}/${Math.round(g.memory_total_mb / 1024)} GB${g.temperature_c != null ? ` · ${g.temperature_c}°C` : ''}`);
    const agent = a => `<li data-activity="${esc(a.activity || 'stopped')}"><i class="k-dot"></i><span>${esc(a.name)}</span><small>${esc(a.activity || 'stopped')}</small></li>`;
    const projects = m.projects
      ? m.projects.length
        ? `<ul class="k-map-projects">${m.projects.map(p => `<li><div><strong>${esc(p.name)}</strong>${p.branch ? ` <code>${esc(p.branch)}</code>` : ''}</div>
            ${p.path ? `<small>${esc(p.path)}</small>` : ''}
            ${p.agents.length ? `<ul class="k-map-agents">${p.agents.map(agent).join('')}</ul>` : ''}</li>`).join('')}</ul>`
        : '<p class="k-hint">No projects open in kimo.</p>'
      : `<p class="k-hint">${m.screen || m.kind === 'phone' || m.kind === 'ipadMini' ? 'A screen: it opens this kimo.' : m.peer ? (m.peer.online ? 'Reading its kimo…' : 'Its kimo does not answer.') : 'No kimo paired here: run kimo on it and pair it (Devices) to see its projects.'}</p>`;
    const jobs = (m.fleet?.jobs || []).map(j => `<li><span>${esc(j.spec?.task || j.id || j)}</span><small>${esc(String(j.state || '').replace(/_/g, ' '))}</small></li>`).join('');
    const kimo = m.hub ? `this kimo${m.kimo?.version ? ` · ${m.kimo.version}` : ''}` : m.peer ? `paired · ${m.peer.online ? 'answers' : 'does not answer'}` : 'not paired';
    panel.innerHTML = `
      <p class="k-map-eyebrow">${esc(m.role || '')}</p>
      <h3 class="k-map-title">${esc(m.name)}</h3>
      ${m.model ? `<p class="k-hint">${esc(m.model)}</p>` : ''}
      <p class="k-map-state" data-status="${esc(m.status)}"><i class="k-dot"></i>${esc(STATUS[m.status]?.[0] || m.status)}${m.working ? ` · ${m.working} working` : ''}</p>
      ${m.fleet?.can_wake && ['off', 'unreachable'].includes(m.fleet.state) ? '<p><button class="ui-button k-small" type="button" data-map-wake>wake</button></p>' : ''}
      ${!m.fleet && !m.hub && addToFleet && fleetOn && t && !['phone', 'ipadMini'].includes(m.kind) ? '<p><button class="ui-button k-small" type="button" data-map-add>add to the fleet</button></p>' : ''}
      <h4 class="k-map-h">Projects</h4>${projects}
      ${jobs ? `<h4 class="k-map-h">Fleet jobs</h4><ul class="k-map-agents">${jobs}</ul>` : ''}
      ${f ? `<h4 class="k-map-h">Machine</h4><dl class="k-map-dl">${rows([
        ['GPU', gpus.join(', ')], ['Load', f.load ? `${f.load[0].toFixed(1)} on ${f.cpus} CPUs` : ''],
        ['Memory', f.mem_total_bytes ? `${mb(f.mem_available_bytes)} free of ${mb(f.mem_total_bytes)}` : ''],
        ['Disk', f.disk ? `${mb(f.disk.free_bytes)} free` : ''], ['Accelerator', f.hailo || (f.jetson_l4t ? `Jetson L4T ${f.jetson_l4t}` : '')],
        ['System', [f.os, f.arch].filter(Boolean).join(' · ')],
      ])}</dl>` : ''}
      <h4 class="k-map-h">Network</h4><dl class="k-map-dl">${rows([
        ['Tailscale', t ? t.dns_name : 'not on the tailnet'], ['Address', t?.ips?.[0]],
        ['Last seen', t && !t.online && t.last_seen ? ago(t.last_seen) : ''], ['kimo', kimo],
        ['Labels', m.labels.join(' · ')],
      ])}</dl>`;
  }

  $('map-panel').addEventListener('click', async e => {
    if (e.target.closest('[data-map-add]') && selected) return addToFleet?.(selected.m.tailnet?.ips?.[0]);
    if (!e.target.closest('[data-map-wake]') || !selected) return;
    try { await local.fleet.wake(selected.m.id); log(`waking ${selected.m.name}`, 'out'); poll(); } catch (err) { log(err.message, 'bad'); }
  });

  function select(id, focus) {
    if (selected) {
      if (selected.sel) selected.sel.visible = false;
      selected.label.classList.remove('on');
    }
    const prev = selected;
    selected = nodes.get(id) || null;
    if (prev) refresh(prev);
    if (selected) {
      if (selected.sel) selected.sel.visible = true;
      selected.label.classList.add('on');
      refresh(selected);
      if (focus) focusOn(selected);
    }
    renderPanel();
  }

  // ----- camera moves: focus, all, top -----
  let flight = null;
  let top = pref('top', '0') === '1';
  const fly = (pos, target, seconds = 0.9) => {
    flight = { from: camera.position.clone(), to: pos, tFrom: controls.target.clone(), tTo: target, t: 0, seconds: reducedMotion ? 0.01 : seconds };
  };
  const VIEW_DIR = new THREE.Vector3(0.03, 0.62, 0.78).normalize();
  const TOP_DIR = new THREE.Vector3(0, 1, 0.0005).normalize();

  function focusOn(n) {
    const target = worldTop(n, -n.top * 0.5 + 0.06);
    const reach = Math.max(n.size.x, n.size.y, n.size.z) * MM;
    const dir = top ? TOP_DIR.clone() : camera.position.clone().sub(controls.target).normalize();
    if (!top) { dir.y = Math.max(dir.y, 0.55); dir.normalize(); }
    fly(target.clone().add(dir.multiplyScalar(Math.max(0.45, reach * 2.6))), target);
  }

  /** Frame every machine, from the side or from above: the closest view where all of them fit. */
  function fitAll(instant = false) {
    const box = new THREE.Box3();
    for (const n of nodes.values()) box.expandByObject(n.root);
    if (box.isEmpty()) return;
    box.max.y += 0.2; // room for the labels
    const target = box.getCenter(new THREE.Vector3()).setY(0);
    const dir = top ? TOP_DIR : VIEW_DIR;
    const corners = [];
    for (const x of [box.min.x, box.max.x]) for (const y of [box.min.y, box.max.y]) for (const z of [box.min.z, box.max.z]) corners.push(new THREE.Vector3(x, y, z));
    const probe = camera.clone();
    const fits = dist => {
      probe.position.copy(target).addScaledVector(dir, dist);
      probe.lookAt(target);
      probe.updateMatrixWorld();
      return corners.every(c => { const p = c.clone().project(probe); return Math.abs(p.x) < 0.9 && Math.abs(p.y) < 0.82 && p.z < 1; });
    };
    let lo = 0.5;
    let hi = 80;
    for (let i = 0; i < 24; i++) { const mid = (lo + hi) / 2; if (fits(mid)) hi = mid; else lo = mid; }
    const pos = target.clone().addScaledVector(dir, hi);
    if (instant) { camera.position.copy(pos); controls.target.copy(target); flight = null; } else fly(pos, target);
  }

  function setTop(on) {
    top = on;
    keep('top', on ? '1' : '0');
    $('map-top').setAttribute('aria-pressed', String(on));
    $('map-top').textContent = on ? '3D' : 'Top';
    controls.enableRotate = !on;
    controls.minPolarAngle = 0;
    controls.maxPolarAngle = on ? 0.0001 : 1.42;
    fitAll();
  }
  $('map-top').addEventListener('click', () => setTop(!top));
  $('map-all').addEventListener('click', () => { select(null); fitAll(); });
  const labelsBox = $('map-labels');
  labelsBox.checked = pref('labels', '1') === '1';
  const syncLabels = () => { labels.domElement.classList.toggle('k-map-nolabels', !labelsBox.checked); keep('labels', labelsBox.checked ? '1' : '0'); };
  labelsBox.addEventListener('change', syncLabels);
  syncLabels();

  // ----- picking -----
  const ray = new THREE.Raycaster();
  const pointer = new THREE.Vector2();
  function pick(ev) {
    const r = renderer.domElement.getBoundingClientRect();
    pointer.set(((ev.clientX - r.left) / r.width) * 2 - 1, -((ev.clientY - r.top) / r.height) * 2 + 1);
    ray.setFromCamera(pointer, camera);
    let o = ray.intersectObjects([...nodes.values()].map(n => n.root), true).find(h => h.object.isMesh)?.object;
    while (o && !o.userData.id) o = o.parent;
    return o ? nodes.get(o.userData.id) : null;
  }
  let down = null;
  labels.domElement.addEventListener('pointerdown', e => { down = { x: e.clientX, y: e.clientY }; });
  labels.domElement.addEventListener('pointerup', e => {
    if (!down || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 6) return;
    const n = pick(e);
    if (n) select(n.m.id, true);
  });

  // ----- traffic: pulses and a short log -----
  const pulses = [];
  function pulse(n, kind = 'out') {
    if (!n?.link || reducedMotion) return;
    const m = new THREE.Mesh(pulseGeo, pulseMats[kind]);
    world.add(m);
    pulses.push({ m, curve: n.link.curve, back: kind !== 'out', t: 0, seconds: 0.5 + n.link.curve.getLength() * 0.4 });
  }
  function log(text, kind = '') {
    const li = document.createElement('li');
    li.className = kind;
    li.innerHTML = `<time>${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time><span></span>`;
    li.querySelector('span').textContent = text;
    const list = $('map-log');
    list.prepend(li);
    while (list.children.length > 3) list.lastChild.remove();
  }

  // ----- data -----
  const kimos = new Map(); // machine id → { projects, facts }
  let lastDetail = 0;
  // A fleet coordinator answers (machines can be added to the fleet from here).
  let fleetOn = false;
  let previous = null;
  let signature = '';

  async function poll() {
    const [status, tailnet, peers, sessions, fleet] = await Promise.all([
      local.status().catch(() => null),
      local.tailnet().catch(() => null),
      local.hosts().catch(() => []),
      local.request('GET', '/sessions?all=1').catch(() => []),
      local.fleet.devices().catch(() => null),
    ]);
    // Projects and facts change slowly: every 20 s, from this kimo and the paired ones that answer.
    if (Date.now() - lastDetail > 20000) {
      lastDetail = Date.now();
      const first = buildMachines({ tailnet, peers, sessions, fleet: fleet || [], status });
      await Promise.all(first.filter(m => m.hub || m.peer?.online).map(async m => {
        const k = m.hub ? local : api.forHost(m.peer.id || m.peer.name);
        const [projects, facts] = await Promise.all([k.projects().catch(() => null), k.node().catch(() => null)]);
        kimos.set(m.id, { projects: projects || undefined, facts: facts || undefined });
      }));
    }
    fleetOn = !!fleet;
    const machines = buildMachines({ tailnet, peers, sessions, kimos, fleet: fleet || [], status });
    const zones = layout(machines);
    const sig = machines.map(m => `${m.id}:${m.kind}:${m.zone}`).join('|');
    if (sig !== signature) {
      const firstBuild = !signature;
      signature = sig;
      build(machines, zones);
      if (firstBuild) fitAll(true);
    } else {
      for (const m of machines) {
        const n = nodes.get(m.id);
        if (n) { n.m = m; refresh(n); }
      }
      if (selected) renderPanel();
    }
    // What changed since the last look.
    if (previous) {
      for (const m of machines) {
        const was = previous.get(m.id);
        const n = nodes.get(m.id);
        if (!was || !n) continue;
        if (m.working > was.working) { pulse(n, 'out'); log(`${m.name} · ${m.working} working`, 'out'); }
        if (m.working < was.working) { pulse(n, 'back'); log(`${m.name} · finished${m.working ? `, ${m.working} still working` : ''}`, 'back'); }
        if (isUp(m.status) && !isUp(was.status)) { pulse(n, 'you'); log(`${m.name} is online`); }
        if (!isUp(m.status) && isUp(was.status)) log(`${m.name} went ${STATUS[m.status]?.[0].toLowerCase() || m.status}`);
      }
    }
    previous = new Map(machines.map(m => [m.id, { working: m.working, status: m.status }]));
    const list = machines.filter(m => m.kind !== 'phone' && m.kind !== 'ipadMini');
    const screens = machines.length - list.length;
    const count = s => list.filter(m => s.includes(m.status)).length;
    $('map-summary').innerHTML = [
      `<span><b>${list.length}</b> machines</span>`,
      `<span><i class="k-dot" data-status="online"></i><b>${count(['online', 'busy'])}</b> online</span>`,
      count(['busy']) ? `<span><i class="k-dot" data-status="busy"></i><b>${count(['busy'])}</b> working</span>` : '',
      `<span><i class="k-dot" data-status="off"></i><b>${count(['off', 'unreachable', 'waking', 'unjoined'])}</b> off</span>`,
      `<span><b>${machines.filter(m => (m.kind === 'phone' || m.kind === 'ipadMini') && isUp(m.status)).length}/${screens}</b> phones and tablets online</span>`,
    ].join('');
    $('map-alerts').innerHTML = tailnet && !tailnet.tailscale
      ? '<div class="ui-alert info"><span>Tailscale is not installed on this machine, so the map shows only the kimos paired with it.</span></div>'
      : '';
  }

  // ----- theme -----
  function applyTheme() {
    const css = getComputedStyle(document.documentElement);
    const v = name => css.getPropertyValue(name).trim();
    palette = {
      bg: v('--color-background'), ink: v('--k-m-ink'), muted: v('--color-muted'), accent: v('--color-accent'), teal: v('--color-teal'),
      paper: v('--k-m-paper'), light: v('--k-m-light'), mid: v('--k-m-mid'), dark: v('--k-m-dark'), pcb: v('--k-m-pcb'),
      metal: v('--k-m-metal'), hole: v('--k-m-hole'), screen: v('--color-background'), screenInk: v('--color-ink'),
      screenMuted: v('--color-muted'), screenLine: v('--color-line'), shadow: parseFloat(v('--k-m-shadow')) || 0.12,
      status: Object.fromEntries(Object.entries(STATUS).map(([k, [, token]]) => [k, v(token)])),
    };
    applyPalette(palette);
    renderer.setClearColor(palette.bg);
    scene.fog.color.set(palette.bg);
    shadowMat.opacity = palette.shadow;
    for (const mat of [gridMinor, gridMajor, zoneLine, zoneDashed, linkSolid]) mat.color.set(palette.ink);
    linkDashed.color.set(palette.muted);
    for (const mat of [hubLine, selectLine, linkSelected, pulseMats.out]) mat.color.set(palette.accent);
    pulseMats.back.color.set(palette.teal);
    pulseMats.you.color.set(palette.ink);
    for (const n of nodes.values()) refresh(n);
  }
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', applyTheme);
  new MutationObserver(applyTheme).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  applyTheme();
  document.fonts?.ready.then(() => repaint(palette));

  // ----- loop -----
  function resize() {
    const w = stage.clientWidth;
    const h = stage.clientHeight;
    if (!w || !h) return;
    renderer.setSize(w, h, false);
    labels.setSize(w, h);
    camera.aspect = w / h;
    camera.fov = w < h ? 50 : 34;
    camera.updateProjectionMatrix();
  }
  new ResizeObserver(resize).observe(stage);

  const clock = new THREE.Clock();
  const ease = t => (t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2);
  let frameId = 0;
  function frame() {
    const dt = Math.min(clock.getDelta(), 0.05);
    const now = clock.elapsedTime;
    if (flight) {
      flight.t = Math.min(1, flight.t + dt / flight.seconds);
      const k = ease(flight.t);
      camera.position.lerpVectors(flight.from, flight.to, k);
      controls.target.lerpVectors(flight.tFrom, flight.tTo, k);
      if (flight.t >= 1) flight = null;
    }
    controls.update();
    for (let i = pulses.length - 1; i >= 0; i--) {
      const p = pulses[i];
      p.t += dt / p.seconds;
      if (p.t >= 1) { world.remove(p.m); pulses.splice(i, 1); continue; }
      p.curve.getPointAt(ease(p.back ? 1 - p.t : p.t), p.m.position);
    }
    for (const n of nodes.values()) {
      const s = n.m.status;
      const blink = (s === 'busy' || s === 'waking') && !reducedMotion;
      for (const l of n.model.userData.leds) l.visible = !blink || Math.sin(now * 9) > -0.2;
      if (!reducedMotion && isUp(s)) for (const f of n.model.userData.fans) f.rotation.y += dt * (s === 'busy' ? 26 : 9);
    }
    renderer.render(scene, camera);
    labels.render(scene, camera);
    frameId = requestAnimationFrame(frame);
  }

  let timer = null;
  function start() {
    resize();
    setTop(top);
    if (!frameId) { clock.getDelta(); frameId = requestAnimationFrame(frame); }
    const tick = () => poll().catch(e => log(e.message, 'bad'));
    tick();
    clearInterval(timer);
    timer = setInterval(() => { if (document.visibilityState === 'visible') tick(); }, 5000);
  }
  function stop() {
    clearInterval(timer);
    timer = null;
    cancelAnimationFrame(frameId);
    frameId = 0;
  }
  return { start, stop };
}
