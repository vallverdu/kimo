// Network map: procedural models of the machines, built in millimetres from their published
// dimensions (scale them by MM). Drawn in kimo's 3D style: flat tones, ink outlines.
// Each builder returns a Group standing on y = 0, centred on x/z, with
// userData.leds (status lights), userData.fans (spun while up) and userData.screens.

import * as THREE from '../vendor/three/three.module.min.js';

export const MM = 0.001; // the scene is in metres

// Shared materials, recoloured by applyPalette() when the theme changes.
const tone = (polygonOffset = true) =>
  new THREE.MeshStandardMaterial({
    roughness: 0.92,
    metalness: 0,
    polygonOffset,
    polygonOffsetFactor: 1,
    polygonOffsetUnits: 1,
  });

export const M = {
  paper: tone(),
  light: tone(),
  mid: tone(),
  dark: tone(),
  pcb: tone(),
  metal: tone(),
  hole: tone(),
  ink: new THREE.LineBasicMaterial(),
  faint: new THREE.LineBasicMaterial({ transparent: true, opacity: 0.35 }),
};

// Canvas textures redrawn with the palette (screens, grilles, lid patterns).
const painted = [];

export function applyPalette(p) {
  M.paper.color.set(p.paper);
  M.light.color.set(p.light);
  M.mid.color.set(p.mid);
  M.dark.color.set(p.dark);
  M.pcb.color.set(p.pcb);
  M.metal.color.set(p.metal);
  M.hole.color.set(p.hole);
  M.ink.color.set(p.ink);
  M.faint.color.set(p.ink);
  for (const t of painted) {
    t.draw(t.ctx, t.canvas.width, t.canvas.height, p);
    t.texture.needsUpdate = true;
  }
}

export function paintedTexture(w, h, draw) {
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = 4;
  const entry = { canvas, ctx: canvas.getContext('2d'), texture, draw };
  painted.push(entry);
  return texture;
}

export function repaint(palette) {
  for (const t of painted) {
    t.draw(t.ctx, t.canvas.width, t.canvas.height, palette);
    t.texture.needsUpdate = true;
  }
}

// ---------------------------------------------------------------------------------------
// Building blocks

const EDGE_ANGLE = 28;

function mesh(parent, geo, mat, x = 0, y = 0, z = 0, { edges = true, rot } = {}) {
  const m = new THREE.Mesh(geo, mat);
  m.position.set(x, y, z);
  if (rot) m.rotation.set(rot[0] || 0, rot[1] || 0, rot[2] || 0);
  m.castShadow = true;
  m.receiveShadow = true;
  if (edges) m.add(new THREE.LineSegments(new THREE.EdgesGeometry(geo, EDGE_ANGLE), M.ink));
  parent.add(m);
  return m;
}

// A box whose bottom sits at y.
function box(parent, w, h, d, mat, x, y, z, opts) {
  return mesh(parent, new THREE.BoxGeometry(w, h, d), mat, x, y + h / 2, z, opts);
}

function cyl(parent, r, h, mat, x, y, z, opts = {}) {
  const geo = new THREE.CylinderGeometry(r, r, h, opts.segments || 28);
  return mesh(parent, geo, mat, x, y + h / 2, z, opts);
}

// Rounded rectangle w×h in the XY plane, extruded `t` along +z.
function roundedPanelGeo(w, h, t, r) {
  const s = new THREE.Shape();
  const x0 = -w / 2;
  const y0 = -h / 2;
  s.moveTo(x0 + r, y0);
  s.lineTo(x0 + w - r, y0);
  s.quadraticCurveTo(x0 + w, y0, x0 + w, y0 + r);
  s.lineTo(x0 + w, y0 + h - r);
  s.quadraticCurveTo(x0 + w, y0 + h, x0 + w - r, y0 + h);
  s.lineTo(x0 + r, y0 + h);
  s.quadraticCurveTo(x0, y0 + h, x0, y0 + h - r);
  s.lineTo(x0, y0 + r);
  s.quadraticCurveTo(x0, y0, x0 + r, y0);
  return new THREE.ExtrudeGeometry(s, { depth: t, bevelEnabled: false, curveSegments: 10 });
}

// A rounded slab lying flat: w (x) × d (z), t thick, bottom at y.
function slab(parent, w, d, t, r, mat, x, y, z) {
  const m = mesh(parent, roundedPanelGeo(w, d, t, r), mat, x, y, z, { rot: [-Math.PI / 2, 0, 0] });
  return m;
}

// Many small identical parts (pins, keys, fins) without outlines.
function many(parent, geo, mat, points, { outline = false } = {}) {
  const im = new THREE.InstancedMesh(geo, mat, points.length);
  const m4 = new THREE.Matrix4();
  points.forEach((p, i) => {
    m4.makeTranslation(p[0], p[1], p[2]);
    im.setMatrixAt(i, m4);
  });
  im.castShadow = true;
  im.receiveShadow = true;
  parent.add(im);
  if (outline) {
    const edges = new THREE.EdgesGeometry(geo, EDGE_ANGLE);
    const pos = edges.attributes.position.array;
    const out = new Float32Array(pos.length * points.length);
    points.forEach((p, i) => {
      for (let k = 0; k < pos.length; k += 3) {
        out[i * pos.length + k] = pos[k] + p[0];
        out[i * pos.length + k + 1] = pos[k + 1] + p[1];
        out[i * pos.length + k + 2] = pos[k + 2] + p[2];
      }
    });
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(out, 3));
    parent.add(new THREE.LineSegments(g, M.ink));
  }
  return im;
}

function led(group, w, h, d, x, y, z) {
  const mat = group.userData.ledMat;
  const m = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), mat);
  m.position.set(x, y + h / 2, z);
  group.add(m);
  group.userData.leds.push(m);
  return m;
}

function newDevice() {
  const g = new THREE.Group();
  g.userData.leds = [];
  g.userData.fans = [];
  g.userData.ledMat = new THREE.MeshBasicMaterial({ color: 0x2f8f4e });
  return g;
}

// A fan seen from above: frame ring, hub and blades (spun around y).
function fan(parent, r, h, x, y, z, group, { frame = true } = {}) {
  if (frame) {
    const ring = new THREE.RingGeometry(r * 0.96, r * 1.08, 40);
    mesh(parent, ring, M.dark, x, y + h, z, { rot: [-Math.PI / 2, 0, 0] });
  }
  const rotor = new THREE.Group();
  rotor.position.set(x, y, z);
  cyl(rotor, r * 0.32, h, M.mid, 0, 0, 0, { segments: 24 });
  const blade = new THREE.BoxGeometry(r * 0.62, h * 0.55, r * 0.22);
  for (let i = 0; i < 9; i++) {
    const a = (i / 9) * Math.PI * 2;
    const b = mesh(rotor, blade, M.light, Math.cos(a) * r * 0.62, h * 0.45, Math.sin(a) * r * 0.62, {
      rot: [0.35, -a, 0],
    });
    b.castShadow = false;
  }
  parent.add(rotor);
  group.userData.fans.push(rotor);
  return rotor;
}

// The darker opening on the face of a port.
function portFace(parent, w, h, x, y, z, face) {
  const t = 0.4;
  const dims = { x: [t, h, w], z: [w, h, t] }[face[0]];
  const sign = face[1] === '-' ? -1 : 1;
  const off = face[0] === 'x' ? [sign * t, 0, 0] : [0, 0, sign * t];
  box(parent, dims[0], dims[1], dims[2], M.hole, x + off[0], y, z + off[2], { edges: false });
}

// ---------------------------------------------------------------------------------------
// Raspberry Pi boards

function piBoard(g, { pi5 = false } = {}) {
  const W = 85;
  const D = 56;
  const stand = 3;
  const top = stand + 1.4;
  const holes = [
    [-W / 2 + 3.5, -D / 2 + 3.5],
    [-W / 2 + 61.5, -D / 2 + 3.5],
    [-W / 2 + 3.5, D / 2 - 3.5],
    [-W / 2 + 61.5, D / 2 - 3.5],
  ];
  for (const [x, z] of holes) cyl(g, 2.6, stand, M.metal, x, 0, z, { segments: 12 });
  slab(g, W, D, 1.4, 3, M.pcb, 0, stand, 0);

  // 40-pin header along the far long edge.
  const hx = -W / 2 + 3.5 + 29;
  const hz = -D / 2 + 3.5;
  box(g, 51, 2.5, 5, M.dark, hx, top, hz);
  const pins = [];
  for (let i = 0; i < 20; i++) for (let j = 0; j < 2; j++) pins.push([hx - 24.13 + i * 2.54, top + 2.5 + 3, hz - 1.27 + j * 2.54]);
  many(g, new THREE.BoxGeometry(0.64, 6, 0.64), M.metal, pins);

  // USB and Ethernet at the +x end; Pi 5 swaps their sides.
  const endX = W / 2;
  const eth = pi5 ? D / 2 - 10.2 : -D / 2 + 10.25;
  const usbA = pi5 ? -D / 2 + 9 : D / 2 - 9;
  const usbB = pi5 ? -D / 2 + 27 : D / 2 - 27;
  box(g, 21, 13.5, 16, M.metal, endX - 21 / 2 + 3, top, eth);
  portFace(g, 11, 8, endX + 3, top + 2.5, eth, 'x+');
  for (const z of [usbA, usbB]) {
    box(g, 17.5, 16, 13.5, M.metal, endX - 17.5 / 2 + 3, top, z);
    portFace(g, 11, 5, endX + 3, top + 1.6, z, 'x+');
    portFace(g, 11, 5, endX + 3, top + 9.4, z, 'x+');
  }

  // Power and display along the near long edge.
  const edgeZ = D / 2;
  const x0 = -W / 2 + 3.5;
  box(g, 9, 3.2, 7.5, M.metal, x0 + 7.7, top, edgeZ - 2.5);
  box(g, 7.5, 3.5, 7.5, M.metal, x0 + 7.7 + 14.8, top, edgeZ - 2.5);
  box(g, 7.5, 3.5, 7.5, M.metal, x0 + 7.7 + 14.8 + 13.5, top, edgeZ - 2.5);
  if (!pi5) cyl(g, 3, 6, M.dark, x0 + 7.7 + 14.8 + 13.5 + 7 + 7.5, top, edgeZ - 3.5, { segments: 16 });

  // Camera / display flat connectors and the microSD slot.
  box(g, 3, 5.5, 22, M.light, x0 + 7.7 + 14.8 + 13.5 + 7 + (pi5 ? 4 : 0), top, 4);
  box(g, 22, 5.5, 3, M.light, -W / 2 + 4, top, 6, { rot: [0, Math.PI / 2, 0] });
  box(g, 12, 1.4, 11, M.metal, -W / 2 + 4, stand - 1.4, 0);

  // SoC, RAM and the I/O chip.
  if (pi5) {
    box(g, 16, 1.2, 16, M.metal, -12, top, 2);
    box(g, 12, 1.0, 12, M.dark, 6, top, 8);
    box(g, 10, 1.0, 10, M.dark, 22, top, -6);
  } else {
    box(g, 15, 2.4, 15, M.metal, -12, top, 0);
    box(g, 10, 1.0, 14, M.dark, 6, top, 0);
    box(g, 8, 1.0, 8, M.dark, 22, top, 6);
  }
  led(g, 2, 1, 1.5, -W / 2 + 2.5, top, D / 2 - 9);
  return { top, hx, hz, W, D };
}

export function rpi4() {
  const g = newDevice();
  piBoard(g);
  return g;
}

// A Pi 5 with its active cooler, without a HAT.
export function rpi5() {
  const g = newDevice();
  const { top } = piBoard(g, { pi5: true });
  box(g, 40, 3, 30, M.metal, -12, top + 1.2, 2);
  fan(g, 13, 6, 14, top + 1.2, 6, g);
  return g;
}

export function rpi5hat() {
  const g = newDevice();
  const { top, hx, hz, W, D } = piBoard(g, { pi5: true });

  // Active cooler over the SoC.
  box(g, 40, 3, 30, M.metal, -12, top + 1.2, 2);
  const fins = [];
  for (let i = 0; i < 9; i++) fins.push([-28 + i * 4, top + 4.2 + 3.5, 2]);
  many(g, new THREE.BoxGeometry(1.2, 7, 28), M.metal, fins, { outline: true });
  fan(g, 13, 6, 14, top + 1.2, 6, g);

  // Stacking header and the AI HAT+ 2 on 16 mm standoffs.
  const hatY = top + 16;
  box(g, 51, 8.5, 5, M.dark, hx, top + 2.5, hz);
  for (const [x, z] of [
    [-W / 2 + 3.5, -D / 2 + 3.5],
    [-W / 2 + 61.5, -D / 2 + 3.5],
    [-W / 2 + 3.5, D / 2 - 3.5],
    [-W / 2 + 61.5, D / 2 - 3.5],
  ])
    cyl(g, 2.4, 16, M.metal, x, top, z, { segments: 12 });
  const hatX = -W / 2 + 32.5;
  slab(g, 65, 56.5, 1.4, 3, M.pcb, hatX, hatY, 0);
  box(g, 51, 2.5, 5, M.dark, hx, hatY + 1.4, hz);

  // Hailo-10H with its heatsink.
  box(g, 26, 2, 26, M.metal, hatX + 4, hatY + 1.4, 6);
  const hfins = [];
  for (let i = 0; i < 7; i++) hfins.push([hatX - 8 + i * 4, hatY + 3.4 + 4, 6]);
  many(g, new THREE.BoxGeometry(1.2, 8, 24), M.metal, hfins, { outline: true });
  box(g, 10, 1.2, 8, M.dark, hatX + 22, hatY + 1.4, -12);

  // PCIe ribbon from the HAT down to the Pi's connector at the -x end.
  box(g, 1, 14, 14, M.light, -W / 2 - 1, top + 2, 0);
  box(g, 6, 1, 14, M.light, -W / 2 + 2, hatY - 0.2, 0);
  led(g, 2, 1, 2, hatX + 26, hatY + 1.4, 22);
  return g;
}

export function zero2() {
  const g = newDevice();
  const W = 65;
  const D = 30;
  const stand = 2;
  const top = stand + 1.2;
  for (const [x, z] of [
    [-W / 2 + 3.5, -D / 2 + 3.5],
    [W / 2 - 3.5, -D / 2 + 3.5],
    [-W / 2 + 3.5, D / 2 - 3.5],
    [W / 2 - 3.5, D / 2 - 3.5],
  ])
    cyl(g, 2.4, stand, M.dark, x, 0, z, { segments: 12 });
  slab(g, W, D, 1.2, 3, M.pcb, 0, stand, 0);
  const holes = [];
  for (let i = 0; i < 20; i++) for (let j = 0; j < 2; j++) holes.push([-24.13 + i * 2.54, top + 0.05, -D / 2 + 3.5 - 1.27 + j * 2.54]);
  many(g, new THREE.CylinderGeometry(0.75, 0.75, 0.1, 10), M.metal, holes);
  box(g, 12, 1.4, 12, M.metal, -6, top, 2);
  box(g, 11.2, 3.3, 7.6, M.metal, -W / 2 + 12.4, top, D / 2 - 3.3);
  box(g, 8, 2.8, 5.6, M.metal, -W / 2 + 41.4, top, D / 2 - 2.6);
  box(g, 8, 2.8, 5.6, M.metal, -W / 2 + 54, top, D / 2 - 2.6);
  box(g, 4, 1.2, 17, M.light, W / 2 - 2.5, top, 0);
  box(g, 12, 1.3, 11, M.metal, -W / 2 + 5, top, 0);
  led(g, 2, 0.8, 1.2, W / 2 - 7, top, D / 2 - 7);
  return g;
}

// ---------------------------------------------------------------------------------------
// Jetson Orin Nano Super developer kit

export function jetson() {
  const g = newDevice();
  const W = 100;
  const D = 79;
  const stand = 5;
  const top = stand + 1.6;
  for (const x of [-W / 2 + 4, W / 2 - 4]) for (const z of [-D / 2 + 4, D / 2 - 4]) cyl(g, 2.6, stand, M.metal, x, 0, z, { segments: 12 });
  slab(g, W, D, 1.6, 3, M.dark, 0, stand, 0);

  // Rear I/O along -z.
  const rear = -D / 2;
  box(g, 16, 14, 15, M.metal, -34, top, rear + 7.5 - 2);
  box(g, 16, 14, 15, M.metal, -16, top, rear + 7.5 - 2);
  box(g, 16, 13.5, 21, M.metal, 4, top, rear + 10.5 - 2);
  box(g, 18, 6, 10, M.dark, 22, top, rear + 5 - 2);
  box(g, 9, 3.2, 8, M.metal, 34, top, rear + 4 - 2);
  cyl(g, 4.5, 11, M.dark, 44, top, rear + 7, { segments: 16 });
  for (const x of [-34, -16]) {
    portFace(g, 12, 5, x, top + 1.5, rear - 2 - 7.5 + 7.5, 'z-');
    portFace(g, 12, 5, x, top + 8, rear - 2 - 7.5 + 7.5, 'z-');
  }

  // Module, heatsink and fan.
  box(g, 70, 6, 45, M.dark, 0, top, 12);
  box(g, 70, 3, 48, M.metal, 0, top + 6, 12);
  const fins = [];
  for (let i = 0; i < 15; i++) fins.push([-33 + i * 4.7, top + 9 + 8, 12]);
  many(g, new THREE.BoxGeometry(1.4, 16, 46), M.metal, fins, { outline: true });
  box(g, 52, 2, 46, M.mid, 0, top + 25, 12);
  fan(g, 21, 6, 0, top + 27, 12, g);

  // Buttons and LEDs on the front edge.
  box(g, 4, 3, 3, M.dark, -40, top, D / 2 - 3);
  led(g, 3, 1.4, 2, -32, top, D / 2 - 3);
  return g;
}

// ---------------------------------------------------------------------------------------
// Intel NUC 11 Enthusiast (221 × 142 × 42 mm)

export function nucEnthusiast() {
  const g = newDevice();
  const W = 221;
  const D = 142;
  const H = 42;
  for (const x of [-W / 2 + 18, W / 2 - 18]) for (const z of [-D / 2 + 18, D / 2 - 18]) cyl(g, 7, 2.5, M.dark, x, 0, z, { segments: 16 });
  box(g, W, H - 6, D, M.dark, 0, 2.5, 0);
  const lidY = 2.5 + H - 6;
  slab(g, W - 4, D - 4, 5.5, 8, M.mid, 0, lidY, 0);

  // Lid: a raised diamond pattern with a light ring.
  const lidTex = paintedTexture(512, 330, (c, w, h, p) => {
    c.fillStyle = p.mid;
    c.fillRect(0, 0, w, h);
    c.strokeStyle = p.ink;
    c.globalAlpha = 0.28;
    c.lineWidth = 1.4;
    for (let i = -h; i < w; i += 22) {
      c.beginPath();
      c.moveTo(i, 0);
      c.lineTo(i + h, h);
      c.stroke();
      c.beginPath();
      c.moveTo(i + h, 0);
      c.lineTo(i, h);
      c.stroke();
    }
    c.globalAlpha = 1;
  });
  const lid = new THREE.Mesh(new THREE.PlaneGeometry(W - 30, D - 30), new THREE.MeshStandardMaterial({ map: lidTex, roughness: 0.9 }));
  lid.rotation.x = -Math.PI / 2;
  lid.position.set(0, lidY + 5.6, 0);
  lid.receiveShadow = true;
  g.add(lid);
  const ring = new THREE.Mesh(new THREE.RingGeometry(20, 23, 48), g.userData.ledMat);
  ring.rotation.x = -Math.PI / 2;
  ring.position.set(0, lidY + 5.7, 0);
  g.add(ring);
  g.userData.leds.push(ring);

  // Front (+z): USB, card reader, audio, power.
  const fz = D / 2;
  for (const x of [-70, -52]) {
    box(g, 13, 5.5, 1, M.hole, x, 16, fz, { edges: false });
  }
  box(g, 26, 2.5, 1, M.hole, -20, 18, fz, { edges: false });
  cyl(g, 2.2, 1, M.hole, 20, 16, fz + 0.5, { rot: [Math.PI / 2, 0, 0], edges: false });
  cyl(g, 4.5, 1.5, M.light, 80, 15, fz + 0.75, { rot: [Math.PI / 2, 0, 0] });
  led(g, 8, 1.4, 1, 80, 9, fz + 0.4);

  // Side vents and rear ports.
  const slots = [];
  for (let i = 0; i < 18; i++) {
    slots.push([-W / 2 - 0.3, 18, -D / 2 + 16 + i * 6.2]);
    slots.push([W / 2 + 0.3, 18, -D / 2 + 16 + i * 6.2]);
  }
  many(g, new THREE.BoxGeometry(0.8, 18, 3), M.hole, slots);
  const rz = -D / 2 - 0.5;
  [[-80, 14, 8], [-60, 13, 6], [-40, 13, 6], [-18, 16, 13], [4, 13, 6], [24, 13, 6], [48, 13, 6], [78, 10, 10]].forEach(([x, w, h]) =>
    box(g, w, h, 1, M.hole, x, 12, rz, { edges: false })
  );
  return g;
}

// ---------------------------------------------------------------------------------------
// Intel NUC 11 Extreme (357 deep × 189 high × 120 wide)

export function nucExtreme() {
  const g = newDevice();
  const W = 120;
  const H = 189;
  const D = 357;
  const foot = 10;
  for (const x of [-W / 2 + 12, W / 2 - 12]) box(g, 16, foot, D - 30, M.dark, x, 0, 0);
  box(g, W, H, D, M.dark, 0, foot, 0);

  // Perforated side panels and top grille.
  const perforated = (w, h, step) =>
    paintedTexture(w, h, (c, cw, ch, p) => {
      c.fillStyle = p.mid;
      c.fillRect(0, 0, cw, ch);
      c.fillStyle = p.hole;
      for (let y = step; y < ch - step / 2; y += step * 0.866) {
        const odd = Math.round(y / (step * 0.866)) % 2;
        for (let x = step + (odd ? step / 2 : 0); x < cw - step / 2; x += step) {
          c.beginPath();
          c.arc(x, y, step * 0.28, 0, Math.PI * 2);
          c.fill();
        }
      }
    });
  const side = new THREE.MeshStandardMaterial({ map: perforated(720, 380, 14), roughness: 0.9 });
  for (const s of [-1, 1]) {
    const plane = new THREE.Mesh(new THREE.PlaneGeometry(D - 36, H - 30), side);
    plane.rotation.y = s * Math.PI / 2;
    plane.position.set(s * (W / 2 + 0.3), foot + H / 2, 0);
    plane.receiveShadow = true;
    g.add(plane);
    plane.add(new THREE.LineSegments(new THREE.EdgesGeometry(plane.geometry), M.ink));
  }
  const grille = new THREE.Mesh(new THREE.PlaneGeometry(W - 24, D - 60), new THREE.MeshStandardMaterial({ map: perforated(240, 700, 12), roughness: 0.9 }));
  grille.rotation.x = -Math.PI / 2;
  grille.position.set(0, foot + H + 0.3, 0);
  g.add(grille);
  grille.add(new THREE.LineSegments(new THREE.EdgesGeometry(grille.geometry), M.ink));

  // Front (+z): panel, ports, status bar; an under-glow strip along the base.
  const fz = D / 2;
  box(g, W - 10, H - 10, 2, M.mid, 0, foot + 5, fz);
  for (const y of [foot + 120, foot + 134]) box(g, 13, 5.5, 1, M.hole, -28, y, fz + 2, { edges: false });
  box(g, 26, 2.5, 1, M.hole, 14, foot + 128, fz + 2, { edges: false });
  cyl(g, 6, 2, M.light, 36, foot + 160, fz + 3, { rot: [Math.PI / 2, 0, 0] });
  led(g, 60, 2.5, 1, 0, foot + 30, fz + 2.2);
  led(g, W - 30, 1.5, D - 60, 0, foot - 1.6, 0);

  // Rear (-z): two-slot GPU bracket and I/O.
  const rz = -D / 2 - 1;
  box(g, 22, 120, 1, M.mid, -20, foot + 20, rz);
  box(g, 22, 120, 1, M.mid, 6, foot + 20, rz);
  for (const y of [foot + 40, foot + 70, foot + 100]) box(g, 12, 7, 1, M.hole, -20, y, rz - 0.5, { edges: false });
  for (const y of [foot + 120, foot + 136, foot + 152, foot + 168]) box(g, 14, 6, 1, M.hole, 36, y, rz, { edges: false });
  return g;
}

// ---------------------------------------------------------------------------------------
// Apple machines (no logos)

export function macMini() {
  const g = newDevice();
  const S = 197;
  const H = 36;
  cyl(g, 80, 3, M.dark, 0, 0, 0, { segments: 48 });
  slab(g, S, S, H - 3, 28, M.metal, 0, 3, 0);
  box(g, 128, 20, 1, M.dark, 0, 9, -S / 2 - 0.3);
  [-52, -38, -24, -10, 6, 22, 40, 54].forEach((x) => box(g, 8, 6, 1, M.hole, x, 15, -S / 2 - 0.8, { edges: false }));
  led(g, 3, 2, 1, S / 2 - 26, 16, S / 2 - 1.4);
  return g;
}

function screenTexture(kind) {
  const [w, h] = kind === 'phone' ? [300, 620] : kind === 'tablet' ? [780, 540] : [900, 580];
  return paintedTexture(w, h, (c, cw, ch, p) => {
    const font = (px, weight = 400) => `${weight} ${px}px "Roboto Mono", ui-monospace, Menlo, monospace`;
    c.fillStyle = p.screen;
    c.fillRect(0, 0, cw, ch);
    const pad = cw * 0.06;
    c.fillStyle = p.screenInk;
    c.font = font(Math.round(cw * (kind === 'phone' ? 0.075 : 0.04)), 500);
    c.fillText('kimo', pad, pad + 18);
    c.fillStyle = p.accent;
    c.beginPath();
    c.arc(cw - pad - 6, pad + 10, 6, 0, Math.PI * 2);
    c.fill();
    // Placeholder rows: a screen, not data.
    const lh = ch * (kind === 'phone' ? 0.065 : 0.08);
    for (let i = 0; i < 6; i++) {
      const y = pad + 70 + i * lh;
      c.fillStyle = i === 0 ? p.accent : p.screenLine;
      c.fillRect(pad, y, (cw - pad * 2) * [0.5, 0.82, 0.64, 0.74, 0.4, 0.58][i], Math.max(4, lh * 0.22));
    }
  });
}

function screen(parent, w, h, kind, x, y, z, rot) {
  const mat = new THREE.MeshBasicMaterial({ map: screenTexture(kind), toneMapped: false });
  const m = new THREE.Mesh(new THREE.PlaneGeometry(w, h), mat);
  m.position.set(x, y, z);
  if (rot) m.rotation.set(...rot);
  parent.add(m);
  return m;
}

export function macbookAir() {
  const g = newDevice();
  const W = 304;
  const D = 215;
  const T = 5.6;
  slab(g, W, D, T, 14, M.metal, 0, 0, 0);
  box(g, 276, 0.6, 112, M.mid, 0, T, -36, { edges: false });
  const keys = [];
  for (let r = 0; r < 6; r++) for (let k = 0; k < 14; k++) keys.push([-123.5 + k * 19, T + 0.6 + 0.4, -84 + r * 18.6]);
  many(g, new THREE.BoxGeometry(16, 0.8, 15), M.dark, keys);
  box(g, 136, 0.3, 86, M.light, 0, T, 50);

  const lid = new THREE.Group();
  lid.position.set(0, T, -D / 2 + 2);
  lid.rotation.x = -(105 * Math.PI) / 180;
  slab(lid, W, D - 4, 5, 14, M.metal, 0, 0, (D - 4) / 2);
  screen(lid, 284, 184, 'laptop', 0, -0.2, (D - 4) / 2 + 2, [Math.PI / 2, 0, 0]);
  g.add(lid);
  led(g, 1.6, 1.6, 1.6, 0, T + 6, -D / 2 + 4);
  return g;
}

// A device standing on a small stand, screen facing +z, leaning back.
function standing(w, h, t, r, kind, lean) {
  const g = newDevice();
  const body = new THREE.Group();
  body.rotation.x = -lean;
  const bodyMesh = mesh(body, roundedPanelGeo(w, h, t, r), M.dark, 0, h / 2, 0);
  bodyMesh.castShadow = true;
  screen(body, w - 10, h - 10, kind, 0, h / 2, t + 0.2);
  g.add(body);
  g.userData.body = body;
  box(g, w * 0.5, 6, 46, M.light, 0, 0, -18);
  box(g, w * 0.5, 10, 4, M.light, 0, 0, t + 3);
  return g;
}

export const ipadMini = () => standing(195.4, 134.8, 6.3, 12, 'tablet', 0.32);

// A phone from its size and camera layout: a full-width bar (Pixel), a square block
// (iPhone) or a tall block (Redmi), on the back (-z).
export function phone({ size = [71.5, 147, 7.8], radius = 10, camera = 'square' } = {}) {
  const [w, h, t] = size;
  const g = standing(w, h, t, radius, 'phone', 0.3);
  const body = g.userData.body;
  const lens = (r, x, y) => mesh(body, new THREE.CylinderGeometry(r, r, 1.6, 20), M.hole, x, y, -2.4, { rot: [Math.PI / 2, 0, 0] });
  if (camera === 'bar') {
    box(body, w, 11, 2.6, M.mid, 0, h * 0.8, -1.3);
    lens(3.4, -w / 2 + 16, h * 0.8 + 5.5);
    lens(3.4, -w / 2 + 27, h * 0.8 + 5.5);
    lens(3.4, -w / 2 + 38, h * 0.8 + 5.5);
  } else if (camera === 'rect') {
    box(body, 22, 38, 1.8, M.mid, -w / 2 + 16, h - 46, -0.9);
    lens(4.5, -w / 2 + 16, h - 17);
    lens(3, -w / 2 + 16, h - 30);
  } else {
    box(body, 31, 31, 1.8, M.mid, -w / 2 + 20, h - 40, -0.9);
    lens(5, -w / 2 + 13, h - 18);
    lens(5, -w / 2 + 13, h - 33);
    lens(5, -w / 2 + 27, h - 25.5);
  }
  return g;
}

export const BUILDERS = { rpi4, rpi5, zero2, rpi5hat, jetson, nucEnthusiast, nucExtreme, macMini, macbookAir, ipadMini, phone };
