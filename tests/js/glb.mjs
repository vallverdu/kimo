// The device-model reader: a node tree flattened into world space, grouped by colour, damaged
// files refused. The same small file as src/core/glb.rs's test.
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { parseGlb } from '../../web/js/glb.js';

function glb(json, bin) {
  let text = Buffer.from(JSON.stringify(json));
  if (text.length % 4) text = Buffer.concat([text, Buffer.alloc(4 - (text.length % 4), 0x20)]);
  const out = Buffer.alloc(20 + text.length + 8 + bin.length);
  out.write('glTF', 0, 'latin1');
  out.writeUInt32LE(2, 4);
  out.writeUInt32LE(out.length, 8);
  out.writeUInt32LE(text.length, 12);
  out.write('JSON', 16, 'latin1');
  text.copy(out, 20);
  out.writeUInt32LE(bin.length, 20 + text.length);
  out.write('BIN\0', 24 + text.length, 'latin1');
  bin.copy(out, 28 + text.length);
  return out.buffer.slice(out.byteOffset, out.byteOffset + out.length);
}

const floats = Buffer.from(new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1]).buffer);
const shorts = Buffer.from(new Uint16Array([0, 1, 2, 0]).buffer);
const half = Math.SQRT1_2;
const model = {
  scene: 0,
  scenes: [{ nodes: [0] }],
  nodes: [{ translation: [0, 2, 0], children: [1, 2] }, { mesh: 0, rotation: [0, half, 0, half] }, { mesh: 0 }],
  meshes: [{ primitives: [{ attributes: { POSITION: 0, NORMAL: 1 }, indices: 2, material: 0 }] }],
  materials: [{ pbrMetallicRoughness: { baseColorFactor: [0.1, 0.2, 0.3, 1] } }],
  bufferViews: [{ byteOffset: 0, byteLength: 36 }, { byteOffset: 36, byteLength: 36 }, { byteOffset: 72, byteLength: 6 }],
  accessors: [
    { bufferView: 0, componentType: 5126, count: 3, type: 'VEC3' },
    { bufferView: 1, componentType: 5126, count: 3, type: 'VEC3' },
    { bufferView: 2, componentType: 5123, count: 3, type: 'SCALAR' },
  ],
};

const parts = parseGlb(glb(model, Buffer.concat([floats, shorts])));
assert.equal(parts.length, 1, 'one colour, one part');
const [p] = parts;
assert.deepEqual(p.color, [0.1, 0.2, 0.3]);
assert.equal(p.positions.length, 18, 'both nodes');
assert.deepEqual([...p.indices], [0, 1, 2, 3, 4, 5], 'the second node starts after the first');
// (1,0,0) turned 90° about y is (0,0,-1), then up by 2.
const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-5, `${a} ≈ ${b}`);
near(p.positions[3], 0); near(p.positions[4], 2); near(p.positions[5], -1);
near(p.normals[0], 1);

assert.throws(() => parseGlb(new ArrayBuffer(8)), /not a GLB/);
assert.throws(() => parseGlb(new Uint8Array(40).buffer), /not a GLB/);
assert.throws(
  () => parseGlb(glb({ ...model, accessors: [{ ...model.accessors[0], count: 3000000 }, model.accessors[1], model.accessors[2]] }, Buffer.concat([floats, shorts]))),
  /past the end/,
);

// A real model, when one has been built here.
const real = process.env.KIMO_TEST_GLB;
if (real && existsSync(real)) {
  const b = readFileSync(real);
  const list = parseGlb(b.buffer.slice(b.byteOffset, b.byteOffset + b.length));
  const count = list.reduce((n, x) => n + x.indices.length / 3, 0);
  assert.ok(count > 100, `${count} triangles`);


  console.log(`${real}: ${list.length} colours, ${count} triangles`);
}
console.log('glb ok');
