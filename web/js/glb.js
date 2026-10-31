// Reads a device model (plain binary glTF, as scripts/device-models/build.py writes it): the node
// tree flattened into world space, triangles grouped by colour. Pure: no three.js, no DOM.
// Not a general glTF reader; src/core/glb.rs is the same thing for the native window.

const IDENTITY = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

function multiply(a, b) {
  const out = new Array(16).fill(0);
  for (let c = 0; c < 4; c++) for (let r = 0; r < 4; r++) for (let k = 0; k < 4; k++) out[c * 4 + r] += a[k * 4 + r] * b[c * 4 + k];
  return out;
}

function local(node) {
  if (node.matrix) return node.matrix;
  const t = node.translation || [0, 0, 0];
  const [x, y, z, w] = node.rotation || [0, 0, 0, 1];
  const s = node.scale || [1, 1, 1];
  return [
    (1 - 2 * (y * y + z * z)) * s[0], 2 * (x * y + z * w) * s[0], 2 * (x * z - y * w) * s[0], 0,
    2 * (x * y - z * w) * s[1], (1 - 2 * (x * x + z * z)) * s[1], 2 * (y * z + x * w) * s[1], 0,
    2 * (x * z + y * w) * s[2], 2 * (y * z - x * w) * s[2], (1 - 2 * (x * x + y * y)) * s[2], 0,
    t[0], t[1], t[2], 1,
  ];
}

const INDEX_TYPES = { 5121: Uint8Array, 5123: Uint16Array, 5125: Uint32Array };

/** @returns {{color: number[], positions: Float32Array, normals: Float32Array, indices: Uint32Array}[]} */
export function parseGlb(buffer) {
  const view = new DataView(buffer);
  if (buffer.byteLength < 20 || view.getUint32(0, true) !== 0x46546c67) throw new Error('not a GLB file');
  const jsonLength = view.getUint32(12, true);
  if (view.getUint32(16, true) !== 0x4e4f534a || 20 + jsonLength > buffer.byteLength) throw new Error('a damaged GLB file');
  const json = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, 20, jsonLength)));
  const at = 20 + jsonLength;
  const binStart = at + 8;
  const binLength = at + 8 <= buffer.byteLength ? view.getUint32(at, true) : 0;
  if (binStart + binLength > buffer.byteLength) throw new Error('a damaged GLB file');

  // A copy of an accessor's numbers (buffer views need not be aligned for typed arrays).
  const read = (i, Type, items) => {
    const a = json.accessors[i];
    const v = json.bufferViews[a.bufferView || 0];
    const start = binStart + (v.byteOffset || 0) + (a.byteOffset || 0);
    const size = Type.BYTES_PER_ELEMENT * items;
    const stride = v.byteStride || size;
    if (stride < size) throw new Error("an accessor's items overlap");
    if (a.count > 0 && start + stride * (a.count - 1) + size > binStart + binLength) throw new Error('an accessor runs past the end of the file');
    const out = new Type(a.count * items);
    if (stride === size) out.set(new Type(buffer.slice(start, start + size * a.count)));
    else for (let n = 0; n < a.count; n++) out.set(new Type(buffer.slice(start + n * stride, start + n * stride + size)), n * items);
    return out;
  };

  // A damaged or hostile file (a node that is its own grandchild, one mesh placed a million
  // times) stops here instead of freezing the page.
  let visits = 0;
  let vertices = 0;
  const parts = new Map();
  const walk = (index, parent, depth) => {
    if (depth > 64) throw new Error('the node tree is too deep');
    if (++visits > 100_000) throw new Error('too many nodes');
    const node = json.nodes[index];
    const world = multiply(parent, local(node));
    const m = world;
    for (const p of node.mesh === undefined ? [] : json.meshes[node.mesh].primitives) {
      if ((p.mode ?? 4) !== 4 || p.attributes.POSITION === undefined) continue;
      const pos = read(p.attributes.POSITION, Float32Array, 3);
      const nor = p.attributes.NORMAL === undefined ? null : read(p.attributes.NORMAL, Float32Array, 3);
      const count = pos.length / 3;
      if ((vertices += count) > 4_000_000) throw new Error('too many vertices for a map model');
      const idx = p.indices === undefined
        ? Uint32Array.from({ length: count }, (_, i) => i)
        : read(p.indices, INDEX_TYPES[json.accessors[p.indices].componentType], 1);
      if (idx.some(i => i >= count)) throw new Error('an index points past the vertices');
      const c = json.materials?.[p.material]?.pbrMetallicRoughness?.baseColorFactor || [0.55, 0.55, 0.57, 1];
      const key = c.slice(0, 3).join();
      if (!parts.has(key)) parts.set(key, { color: c.slice(0, 3), chunks: [], vertices: 0, triangles: 0 });
      const part = parts.get(key);
      const positions = new Float32Array(count * 3);
      const normals = new Float32Array(count * 3);
      for (let i = 0; i < count; i++) {
        const [x, y, z] = [pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]];
        positions[i * 3] = m[0] * x + m[4] * y + m[8] * z + m[12];
        positions[i * 3 + 1] = m[1] * x + m[5] * y + m[9] * z + m[13];
        positions[i * 3 + 2] = m[2] * x + m[6] * y + m[10] * z + m[14];
        const [a, b, d] = nor ? [nor[i * 3], nor[i * 3 + 1], nor[i * 3 + 2]] : [0, 1, 0];
        const [nx, ny, nz] = [m[0] * a + m[4] * b + m[8] * d, m[1] * a + m[5] * b + m[9] * d, m[2] * a + m[6] * b + m[10] * d];
        const len = Math.hypot(nx, ny, nz) || 1;
        normals[i * 3] = nx / len;
        normals[i * 3 + 1] = ny / len;
        normals[i * 3 + 2] = nz / len;
      }
      part.chunks.push({ positions, normals, idx, base: part.vertices });
      part.vertices += count;
      part.triangles += idx.length;
    }
    for (const child of node.children || []) walk(child, world, depth + 1);
  };
  for (const n of json.scenes?.[json.scene ?? 0]?.nodes || []) walk(n, IDENTITY, 0);

  return [...parts.values()].map(part => {
    const positions = new Float32Array(part.vertices * 3);
    const normals = new Float32Array(part.vertices * 3);
    const indices = new Uint32Array(part.triangles);
    let filled = 0;
    for (const c of part.chunks) {
      positions.set(c.positions, c.base * 3);
      normals.set(c.normals, c.base * 3);
      for (let i = 0; i < c.idx.length; i++) indices[filled++] = c.idx[i] + c.base;
    }
    return { color: part.color, positions, normals, indices };
  });
}
