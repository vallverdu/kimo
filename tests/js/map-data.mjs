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
