// The SDK keeps an action on the kimo chosen when it started, even if useHost() changes
// during it (fetch is a recording stub: nothing is sent anywhere).
import assert from 'node:assert/strict';
import { Kimo } from '../../web/sdk/kimo.js';

const calls = [];
globalThis.fetch = async (url, init) => {
  calls.push({ url: String(url), body: init?.body });
  return { ok: true, json: async () => ({ path: '/tmp/x' }) };
};

const api = new Kimo({ url: 'http://kimo.test', token: 't', host: 'host-a' });
const sending = api.send('s1', 'hello');
api.useHost('host-b');
await sending;
assert.deepEqual(calls.map(c => c.url), [
  'http://kimo.test/api/hosts/host-a/sessions/s1/input',
  'http://kimo.test/api/hosts/host-a/sessions/s1/input',
]);

calls.length = 0;
api.useHost('host-a');
let release;
const file = { name: 'photo.png', arrayBuffer: () => new Promise(r => { release = () => r(new Uint8Array([1, 2, 3]).buffer); }) };
const uploading = api.sendFile('s1', file);
api.useHost('host-b');
release();
await uploading;
assert.equal(calls[0].url, 'http://kimo.test/api/hosts/host-a/sessions/s1/files');

// The client itself now points at host-b for new actions.
calls.length = 0;
await api.type('s1', 'x');
assert.equal(calls[0].url, 'http://kimo.test/api/hosts/host-b/sessions/s1/input');
console.log('sdk hosts ok');
