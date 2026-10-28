// The fleet calls go to the kimo serving the page, even while the client points at a paired
// kimo (the coordinator sits beside the phone's own kimo); YAML goes as text (fetch is a
// recording stub: nothing is sent anywhere).
import assert from 'node:assert/strict';
import { Kimo } from '../../web/sdk/kimo.js';

const calls = [];
globalThis.fetch = async (url, init) => {
  calls.push({ url: String(url), method: init?.method, body: init?.body, type: init?.headers?.['Content-Type'] });
  return { ok: true, json: async () => ({}), text: async () => 'devices: {}\n' };
};

const api = new Kimo({ url: 'http://pi4.test', token: 't', host: 'nuc' });
await api.fleet.wake('nuc-extreme');
await api.fleet.hold('nuc-extreme', { hours: 2 });
await api.fleet.submit({ task: 'tests', run: 'cargo test', requires: { labels: ['cuda'] } });
await api.fleet.submitWorkflow('workflow: w\nsteps:\n  - task: a\n    run: true\n');
await api.fleet.events({ device: 'nuc-extreme', limit: 5 });
assert.deepEqual(calls.map(c => `${c.method} ${c.url}`), [
  'POST http://pi4.test/api/fleet/devices/nuc-extreme/wake',
  'POST http://pi4.test/api/fleet/devices/nuc-extreme/hold',
  'POST http://pi4.test/api/fleet/jobs',
  'POST http://pi4.test/api/fleet/workflows',
  'GET http://pi4.test/api/fleet/events?device=nuc-extreme&limit=5',
]);
assert.equal(calls[1].body, '{"hours":2}');
assert.equal(calls[2].type, 'application/json');
assert.equal(calls[3].type, 'text/yaml');
assert.ok(calls[3].body.startsWith('workflow: w'));
assert.equal(
  api.fleet.artifactUrl('j-1', 'out/model v2.onnx'),
  'http://pi4.test/api/fleet/jobs/j-1/artifacts/out/model%20v2.onnx?token=t',
);

// Jobs and facts belong to the kimo the client points at.
calls.length = 0;
await api.createJob({ command: 'make test', git: { repo: '~/dev/seg', branch: 'kimo/t' } });
await api.node();
assert.deepEqual(calls.map(c => c.url), [
  'http://pi4.test/api/hosts/nuc/jobs',
  'http://pi4.test/api/hosts/nuc/node',
]);
console.log('sdk fleet ok');
