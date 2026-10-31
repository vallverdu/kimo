// The map's machines: fleet machines drawn from their hardware model and named by their role,
// tailnet machines merged with the fleet, phones apart (pure functions, no three.js).
import assert from 'node:assert/strict';
import { buildMachines, kindOf, layout } from '../../web/js/map-data.js';

assert.equal(kindOf({ name: 'box', os: 'linux', model: 'nuc-11-extreme' }), 'nucExtreme');
assert.equal(kindOf({ name: 'board', os: 'linux', model: 'rpi5-hailo' }), 'rpi5hat');
assert.equal(kindOf({ name: 'jetson', os: 'linux' }), 'jetson');
assert.equal(kindOf({ name: 'iPhone', os: 'iOS' }), 'phone');

const tailnet = {
  tailscale: true,
  devices: [
    { name: 'pi4', host_name: 'pi4', dns_name: 'pi4.ts.net', ips: ['100.64.0.1'], os: 'linux', online: true, self: true, mine: true },
    { name: 'gpu', host_name: 'gpu', dns_name: 'gpu.ts.net', ips: ['100.64.0.2'], os: 'linux', online: false, self: false, mine: true },
    { name: 'iphone', host_name: 'iPhone', dns_name: 'iphone.ts.net', ips: ['100.64.0.3'], os: 'iOS', online: true, self: false, mine: true },
    { name: 'stranger', host_name: 'x', dns_name: 'x.ts.net', ips: ['100.64.0.9'], os: 'linux', online: true, self: false, mine: false },
  ],
};
const fleet = [
  { id: 'gpu', state: 'off', registry: { kimo: 'http://100.64.0.2:7777', model: 'nuc-11-extreme', role: 'gpu-worker' }, labels: ['cuda'], jobs: [] },
  { id: 'jetson', state: 'ready', registry: { kimo: 'http://10.147.17.9:7777', model: 'jetson-orin-nano', role: 'edge-target' }, labels: [], jobs: ['j-1'] },
];
const machines = buildMachines({ tailnet, fleet, status: { host: 'pi4' } });
const by = id => machines.find(m => m.id === id);
assert.equal(machines.length, 4, 'not the stranger');
assert.equal(by('gpu').kind, 'nucExtreme');
assert.equal(by('gpu').role, 'GPU worker');
assert.equal(by('gpu').status, 'off');
// In the fleet over ZeroTier only: still on the map, busy with its job.
assert.equal(by('jetson').kind, 'jetson');
assert.equal(by('jetson').role, 'Edge target');
assert.equal(by('jetson').status, 'busy');
assert.equal(by('iphone').zone, 'screens');
assert.ok(by('pi4').hub);
const zones = layout(machines);
assert.ok(zones.some(z => z.id === 'core') && zones.some(z => z.id === 'compute'));
console.log('map data ok');

// Fleet machines found over ZeroTier, on plugs, with shortcuts.
{
  const { buildMachines, meshMatrix, meshPairs } = await import('../../web/js/map-data.js');
  const list = buildMachines({
    tailnet: { tailscale: true, devices: [{ name: 'lab-ts', host_name: 'lab', dns_name: 'lab-ts.demo.ts.net', ips: ['100.64.0.5'], os: 'linux', online: true, self: false, mine: true }] },
    fleet: [
      { id: 'lab-ts', state: 'ready', registry: { zerotier: '10.147.17.21', tailscale: 'lab-ts', user: 'demo', alias: 'zlab', model: 'nuc-11-extreme' }, labels: [], jobs: [], plug: { address: '192.168.1.50', status: { on: true, watts: 41 } } },
      { id: 'guest', state: 'ready', registry: { zerotier: '10.147.17.60', user: 'demo', model: 'gaming-laptop' }, labels: ['site:client'], jobs: [] },
      { id: 'zero', state: 'off', registry: { zerotier: '10.147.17.70', user: 'demo', model: 'rpi-zero-2w' }, labels: [], jobs: [] },
    ],
    status: { host: 'mac' },
  });
  const m = id => list.find(x => x.id === id);
  assert.equal(m('lab-ts').networks.tailscale, '100.64.0.5', 'the tailnet entry and the fleet entry are one machine');
  assert.equal(m('lab-ts').networks.zerotier, '10.147.17.21');
  assert.equal(m('lab-ts').alias, 'zlab');
  assert.equal(m('lab-ts').plug.status.watts, 41);
  assert.equal(m('guest').kind, 'laptop');
  assert.equal(m('guest').status, 'online', 'ZeroTier only is still online');
  assert.equal(m('guest').alias, 'zguest');
  assert.equal(m('guest').zone, 'site:client');
  assert.equal(m('zero').kind, 'zero2');

  const link = (from, to, found, expect = 'key', network = 'zerotier') => ({ from, to, network, expect, found, as_intended: expect === 'key' ? found === 'ok' : expect === 'password' ? found === 'nokey' : true });
  const report = {
    nodes: ['a', 'b', 'k', 'x'].map(id => ({ id })),
    links: [
      link('a', 'b', 'ok'), link('b', 'a', 'ok'),
      link('a', 'b', 'check', 'key', 'tailscale'),
      link('a', 'k', 'ok'), link('k', 'a', 'nokey', 'password'),
      link('b', 'k', 'ok'), link('k', 'b', 'ok', 'password'),
      link('a', 'x', 'down'),
    ],
  };
  const pair = (a, b) => meshPairs(report).find(p => p.a === a && p.b === b);
  assert.equal(pair('a', 'b').state, 'good', 'a browser check on Tailscale is not a broken link');
  assert.equal(pair('a', 'b').check, true);
  assert.deepEqual(pair('a', 'b').networks, ['tailscale', 'zerotier']);
  assert.equal(pair('a', 'k').state, 'oneway');
  assert.equal(pair('b', 'k').state, 'violation');
  assert.equal(pair('a', 'x').state, 'problem');
  assert.deepEqual(pair('a', 'x').networks, []);
  assert.deepEqual(meshPairs(null), []);

  const { ids, rows } = meshMatrix(report);
  assert.deepEqual(ids, ['a', 'b', 'k', 'x']);
  const cell = (f, t) => rows[ids.indexOf(f)].cells[ids.indexOf(t)];
  assert.deepEqual(cell('a', 'b'), { word: 'ok', state: 'good' }, 'ZeroTier decides the cell');
  assert.deepEqual(cell('k', 'a'), { word: 'pw', state: 'good' });
  assert.deepEqual(cell('k', 'b'), { word: 'ok', state: 'violation' });
  assert.deepEqual(cell('a', 'x'), { word: 'down', state: 'problem' });
  assert.deepEqual(cell('a', 'a'), { word: '', state: 'none' });
  assert.deepEqual(cell('x', 'a'), { word: '?', state: 'none' });
  console.log('mesh ok');
}
