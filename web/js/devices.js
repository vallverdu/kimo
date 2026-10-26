// Devices view: where kimo is reachable, QR pairing for phones, and paired devices.
import { $, esc, toast } from './ui.js';

export function initDevices({ api, state, run }) {
  let timer = null, code = null;

  // What the phone needs to reach kimo over each kind of address.
  const tips = {
    zerotier: 'ZeroTier: the phone needs the ZeroTier One app joined to the same network (network id from my.zerotier.com), the phone authorized there (Auth ✓), and the app\'s switch on. Test first: open the address in the phone\'s browser.',
    tailscale: 'Tailscale: the phone needs the Tailscale app signed in to the same tailnet and connected.',
    lan: 'Local network: the phone must be on the same Wi-Fi (not guest Wi-Fi or mobile data).',
  };

  function showTip() {
    const a = state.network?.addresses.find(a => a.url === $('pair-address').value);
    const tip = a && tips[a.kind];
    $('pair-tip').hidden = !tip;
    $('pair-tip').textContent = tip || '';
  }

  async function load() {
    try {
      [state.network, state.devices] = await Promise.all([api.network(), api.devices()]);
      render();
    } catch (e) { toast(e.message, true); }
  }

  function render() {
    const net = state.network;
    if (!net) return;
    const remote = net.addresses.filter(a => a.kind !== 'localhost');
    const chosen = $('pair-address').value;
    $('pair-address').innerHTML = remote.length
      ? remote.map(a => `<option value="${esc(a.url)}" ${a.url === chosen ? 'selected' : ''}>${esc(a.label)} · ${esc(a.url)}</option>`).join('')
      : '<option value="">only this machine: start kimo with --bind all</option>';
    $('address-list').innerHTML = net.addresses.map(a => `<li>
      <span>${esc(a.label)}</span><span><a href="${esc(a.url)}" target="_blank" rel="noopener">${esc(a.url)}</a>${a.https ? ' <span class="ui-badge running">https</span>' : ''}</span></li>`).join('');
    $('pair-firewall').hidden = !net.firewall;
    $('pair-firewall').textContent = net.firewall || '';
    showTip();
    const ts = net.tailscale;
    $('https-panel').hidden = !ts || !!ts.https_url;
    if (ts && !ts.https_url) $('https-host').textContent = ts.dns_name || 'your tailnet';
    $('device-list').innerHTML = state.devices.length ? state.devices.map(d => `<li data-device="${esc(d.id)}">
      <span class="k-name" style="cursor:default">${esc(d.name)}</span><span class="ui-badge running">paired</span>
      <div class="k-meta"><span>since ${new Date(d.paired_at).toLocaleString()}</span></div>
      <div class="k-actions"><button class="ui-button k-small k-danger" data-do="unpair">unpair</button></div></li>`).join('')
      : '<li class="ui-empty">No devices yet. Show the QR code and scan it with your phone.</li>';
  }

  function hideQr() {
    clearInterval(timer);
    code = null;
    $('pair-box').hidden = true;
    $('pair-show').hidden = false;
  }

  // The QR code for the chosen address (another address keeps the same one-time code).
  async function drawQr() {
    const url = `${$('pair-address').value}/#pair=${code}`;
    $('pair-qr').innerHTML = await api.qr(url);
    $('pair-link').textContent = url;
    $('pair-link').href = url;
  }

  async function showQr() {
    const base = $('pair-address').value;
    if (!base) return toast('kimo is only reachable from this machine', true);
    try {
      const started = await api.startPairing();
      code = started.code;
      const expires_in = started.expires_in;
      await drawQr();
      $('pair-box').hidden = false;
      $('pair-show').hidden = true;
      const before = state.devices.length, until = Date.now() + expires_in * 1000;
      clearInterval(timer);
      const tick = async () => {
        const left = Math.max(0, Math.round((until - Date.now()) / 1000));
        $('pair-expires').textContent = `valid for ${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}, once`;
        if (!left) return hideQr();
        state.devices = await api.devices().catch(() => state.devices);
        if (state.devices.length > before) {
          toast(`Paired: ${state.devices[state.devices.length - 1].name}`);
          hideQr();
        }
        render();
      };
      tick();
      timer = setInterval(tick, 2000);
    } catch (e) { toast(e.message, true); }
  }

  $('pair-show').addEventListener('click', showQr);
  $('pair-cancel').addEventListener('click', hideQr);
  $('pair-address').addEventListener('change', () => {
    showTip();
    if (code) drawQr().catch(e => toast(e.message, true));
  });
  $('https-enable').addEventListener('click', async e => {
    e.target.disabled = true;
    const r = await run(api.enableHttps(), r => `HTTPS ready: ${r.url}`);
    e.target.disabled = false;
    if (r) load();
  });
  $('device-list').addEventListener('click', e => {
    const id = e.target.closest('[data-do=unpair]') && e.target.closest('li[data-device]')?.dataset.device;
    if (id && confirm('Unpair this device? It loses access until it is paired again.')) run(api.deleteDevice(id), 'Device unpaired').then(load);
  });

  return { load, render, showQr, hideQr };
}
