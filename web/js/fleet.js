// Fleet view: the machines the coordinator beside this kimo manages (wake, switch off, keep
// on, take control, pair), the jobs and workflows placed on them, and what happened lately.
import { $, esc, mb, toast } from './ui.js';

const STATE_BADGE = {
  ready: 'running', busy: 'running', idle: 'running', waking: 'paused', booted: 'paused',
  draining: 'paused', off: 'stopped', unknown: 'stopped', unreachable: 'exited', manual: 'paused',
};
const JOB_BADGE = {
  running: 'running', dispatched: 'running', succeeded: 'running', waking: 'paused', ready: 'paused',
  blocked: 'stopped', waiting_device: 'paused', awaiting_approval: 'paused', failed: 'exited',
  cancelled: 'stopped', skipped: 'stopped',
};
const words = s => String(s || '').replace(/_/g, ' ');
const ago = ms => {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s} s ago` : s < 3600 ? `${Math.round(s / 60)} min ago` : s < 86400 ? `${Math.round(s / 3600)} h ago` : new Date(ms).toLocaleDateString();
};
const inMin = ms => `${Math.max(0, Math.round((ms - Date.now()) / 60000))} min`;

/** One line about what a machine has and does now. */
function factsLine(d) {
  const f = d.facts;
  if (!f) return '';
  const parts = [];
  for (const g of f.gpus || []) parts.push(`${g.name.replace(/^NVIDIA (GeForce )?/, '')} ${Math.round(g.memory_used_mb / 1024 * 10) / 10}/${Math.round(g.memory_total_mb / 1024)} GB${g.temperature_c != null ? ` ${g.temperature_c}°C` : ''}`);
  if (f.hailo) parts.push(f.hailo);
  if (f.jetson_l4t) parts.push(`L4T ${f.jetson_l4t}`);
  if (f.xcode) parts.push(`Xcode ${f.xcode}`);
  parts.push(`load ${f.load?.[0]?.toFixed(1) ?? '–'}/${f.cpus}`);
  if (f.mem_total_bytes) parts.push(`${mb(f.mem_available_bytes)} free of ${mb(f.mem_total_bytes)}`);
  if (f.disk) parts.push(`disk ${mb(f.disk.free_bytes)} free`);
  const s = f.sessions || {};
  if (s.running || s.jobs_running) parts.push(`${s.running || 0} terminals · ${s.jobs_running || 0} jobs`);
  if (f.logins?.length) parts.push(`${f.logins.length} logged in`);
  return parts.join(' · ');
}

export function initFleet({ api, state, run }) {
  let timer = null, logTimer = null;
  const data = { status: null, devices: [], jobs: [], workflows: [], events: [] };
  const fleet = api.fleet;

  async function load() {
    try {
      [data.status, data.devices, data.jobs, data.workflows, data.events] = await Promise.all([
        fleet.status(), fleet.devices(), fleet.jobs(), fleet.workflows(), fleet.events({ limit: 25 }),
      ]);
      render();
    } catch (e) {
      $('fleet-alerts').innerHTML = `<div class="ui-alert"><span>${esc(e.message)}</span></div>`;
    }
  }

  /** Is there a coordinator behind this kimo? (The tab shows only then.) */
  async function available() {
    try { await fleet.status(); return true; } catch { return false; }
  }

  function start() {
    clearInterval(timer);
    load();
    timer = setInterval(() => { if (document.visibilityState === 'visible') load(); }, 5000);
  }
  function stop() { clearInterval(timer); timer = null; }

  function deviceRow(d) {
    const up = ['ready', 'busy', 'idle', 'booted'].includes(d.power);
    const held = d.keep_on_until || d.never_auto_shutdown;
    const note = [];
    if (d.shutdown_at && !d.manual) note.push(`switches off in ${inMin(d.shutdown_at)} if idle`);
    if (d.keep_on_until) note.push(`kept on until ${new Date(d.keep_on_until).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`);
    if (d.never_auto_shutdown) note.push('never switched off');
    if (d.jobs.length) note.push(`${d.jobs.length} job${d.jobs.length === 1 ? '' : 's'}`);
    if (d.last_seen && !up) note.push(`seen ${ago(d.last_seen)}`);
    const error = d.error && (up || d.power === 'waking' || !d.paired) ? d.error : '';
    const actions = [
      !d.paired ? '<button class="ui-button k-small" data-do="pair">pair</button>' : '',
      d.can_wake && ['off', 'unreachable', 'unknown'].includes(d.power) ? '<button class="ui-button k-small" data-do="wake">wake</button>' : '',
      up && d.paired ? '<button class="ui-button ghost k-small" data-do="shutdown">switch off</button>' : '',
      up && d.auto_shutdown ? (held
        ? '<button class="ui-button ghost k-small" data-do="release">let it sleep</button>'
        : '<button class="ui-button ghost k-small" data-do="keep">keep on 2 h</button>') : '',
      `<button class="ui-button ghost k-small" data-do="${d.manual ? 'auto' : 'manual'}">${d.manual ? 'automatic' : 'manual'}</button>`,
    ].join('');
    return `<li data-device="${esc(d.id)}">
      <span class="k-name" style="cursor:default">${esc(d.id)}</span><span class="ui-badge ${STATE_BADGE[d.state] || 'stopped'}">${esc(words(d.state))}</span>
      <div class="k-meta"><span>${esc(d.labels.join(' · '))}</span>${d.facts?.model ? `<span>${esc(d.facts.model)}</span>` : ''}</div>
      ${factsLine(d) ? `<div class="k-meta"><span>${esc(factsLine(d))}</span></div>` : ''}
      ${note.length ? `<div class="k-meta"><span>${esc(note.join(' · '))}</span></div>` : ''}
      ${error ? `<div class="k-meta"><span class="k-warn">${esc(error)}</span></div>` : ''}
      <div class="k-actions">${actions}</div></li>`;
  }

  function jobRow(j) {
    const outputs = Object.keys(j.artifacts || {});
    const actions = [
      '<button class="ui-button ghost k-small" data-do="log">log</button>',
      j.state === 'awaiting_approval' ? '<button class="ui-button k-small" data-do="approve">approve</button><button class="ui-button ghost k-small k-danger" data-do="reject">reject</button>' : '',
      !['succeeded', 'failed', 'cancelled', 'skipped', 'awaiting_approval'].includes(j.state) ? '<button class="ui-button ghost k-small k-danger" data-do="cancel">cancel</button>' : '',
    ].join('');
    const answer = typeof j.result?.result === 'string' ? j.result.result : typeof j.result === 'string' ? j.result : '';
    return `<li data-job="${esc(j.id)}">
      <span class="k-name" style="cursor:default">${esc(j.spec.task)}</span><span class="ui-badge ${JOB_BADGE[j.state] || 'stopped'}">${esc(words(j.state))}</span>
      <div class="k-meta">${j.device ? `<span>on ${esc(j.device)}</span>` : ''}<span>${esc(j.spec.runner)}</span>${j.attempt > 1 ? `<span>attempt ${j.attempt}</span>` : ''}${j.exit_code != null ? `<span>exit ${j.exit_code}</span>` : ''}<span>${ago(j.updated_at)}</span></div>
      ${j.reason ? `<div class="k-meta"><span>${esc(j.reason)}</span></div>` : ''}
      ${answer ? `<div class="k-meta"><span>${esc(answer.slice(0, 300))}</span></div>` : ''}
      ${outputs.length ? `<div class="k-meta">${outputs.map(o => `<a class="k-link" href="${esc(fleet.artifactUrl(j.id, o))}" target="_blank" rel="noopener">${esc(o)}</a>`).join('')}</div>` : ''}
      <div class="k-actions">${actions}</div></li>`;
  }

  function render() {
    const st = data.status;
    $('fleet-alerts').innerHTML = [
      st?.registry_error ? `<div class="ui-alert"><span>${esc(st.registry_error)}</span></div>` : '',
      !data.devices.length ? `<div class="ui-alert info"><span>No machines yet. Add them to ${esc(st?.registry || 'devices.yaml')} on the coordinator, then pair each one with the code its kimo shows (kimo pair).</span></div>` : '',
    ].join('');
    $('fleet-devices').innerHTML = data.devices.map(deviceRow).join('') || '<li class="ui-empty">No machines.</li>';
    const waiting = data.jobs.filter(j => j.state === 'awaiting_approval');
    const recent = [...waiting, ...data.jobs.filter(j => j.state !== 'awaiting_approval')].slice(0, 30);
    $('fleet-jobs').innerHTML = recent.map(jobRow).join('') || '<li class="ui-empty">No jobs yet. Tap + to run one on whichever machine fits.</li>';
    $('fleet-workflows').innerHTML = data.workflows.slice(0, 10).map(w => {
      const done = w.steps.filter(([, id]) => data.jobs.find(j => j.id === id)?.state === 'succeeded').length;
      return `<li><span class="k-name" style="cursor:default">${esc(w.name)}</span><span class="ui-badge ${JOB_BADGE[w.state] || 'paused'}">${esc(w.state)}</span>
        <div class="k-meta"><span>${done}/${w.steps.length} steps</span><span>${ago(w.created_at)}</span><span><code>${esc(w.id)}</code></span></div>
        ${w.state === 'running' ? `<div class="k-actions"><button class="ui-button ghost k-small k-danger" data-cancel-workflow="${esc(w.id)}">cancel</button></div>` : ''}</li>`;
    }).join('') || '<li class="ui-empty">No workflows yet.</li>';
    $('fleet-events').innerHTML = data.events.map(e => `<li><span>${esc(e.device || e.job || '')}</span><span>${esc(e.detail)} <small>${ago(e.at)}</small></span></li>`).join('');
  }

  async function act(promise, done) {
    const r = await run(promise, done);
    load();
    return r;
  }

  $('fleet-devices').addEventListener('click', e => {
    const what = e.target.closest('[data-do]')?.dataset.do;
    const id = e.target.closest('li[data-device]')?.dataset.device;
    if (!what || !id) return;
    const d = data.devices.find(x => x.id === id);
    if (what === 'wake') return act(fleet.wake(id), `Waking ${id}`);
    if (what === 'shutdown') {
      if (!confirm(`Switch ${id} off? Its kimo refuses while terminals, jobs or people are on it.`)) return;
      return act(fleet.shutdown(id), `Switching ${id} off`);
    }
    if (what === 'keep') return act(fleet.hold(id, { hours: 2 }), `${id} stays on for 2 hours`);
    if (what === 'release') return act(fleet.hold(id, { hours: 0, never: false }), `${id} switches off when idle`);
    if (what === 'manual') return act(fleet.hold(id, { manual: true }), `${id}: nothing automatic until you give it back`);
    if (what === 'auto') return act(fleet.hold(id, { manual: false }), `${id} is automatic again`);
    if (what === 'pair') {
      const code = prompt(`Pairing code shown by kimo on ${id} (run kimo pair there, or Devices → show QR code):`);
      if (code) return act(fleet.pair(id, code), `Paired with ${id}`);
    }
    return d;
  });

  $('fleet-jobs').addEventListener('click', e => {
    const what = e.target.closest('[data-do]')?.dataset.do;
    const id = e.target.closest('li[data-job]')?.dataset.job;
    if (!what || !id) return;
    if (what === 'log') return openLog(id);
    if (what === 'cancel' && confirm('Cancel this job?')) return act(fleet.cancel(id), 'Cancelled');
    if (what === 'approve') return act(fleet.approve(id), 'Approved');
    if (what === 'reject' && confirm('Reject this step? The workflow stops here.')) return act(fleet.reject(id), 'Rejected');
  });
  $('fleet-workflows').addEventListener('click', e => {
    const id = e.target.closest('[data-cancel-workflow]')?.dataset.cancelWorkflow;
    if (id && confirm('Cancel the steps of this workflow that have not ended?')) act(fleet.cancelWorkflow(id), 'Workflow cancelled');
  });

  // A job's log, followed while it runs.
  const logSheet = $('sheet-fleet-log');
  async function openLog(id) {
    const job = data.jobs.find(j => j.id === id);
    logSheet.querySelector('h3').textContent = job ? `${job.spec.task} · ${words(job.state)}` : id;
    const pre = logSheet.querySelector('pre');
    pre.textContent = '';
    let offset = 0;
    clearTimeout(logTimer);
    const more = async () => {
      try {
        const chunk = await fleet.log(id, offset);
        pre.textContent += chunk.text;
        offset = chunk.next;
        pre.scrollTop = pre.scrollHeight;
        if (!chunk.done && logSheet.open) logTimer = setTimeout(more, chunk.next > chunk.offset ? 200 : 2000);
      } catch (e) { pre.textContent += `\n[${e.message}]`; }
    };
    logSheet.showModal();
    more();
  }
  logSheet.addEventListener('close', () => clearTimeout(logTimer));

  // A new job: a command on whichever machine has the labels, or a job or workflow in YAML.
  const jobSheet = $('sheet-fleet-job');
  const form = jobSheet.querySelector('form');
  const syncMode = () => {
    const yaml = form.elements.mode.value === 'yaml';
    form.querySelectorAll('[data-for=command]').forEach(el => { el.hidden = yaml; });
    form.querySelectorAll('[data-for=yaml]').forEach(el => { el.hidden = !yaml; });
  };
  form.querySelectorAll('[name=mode]').forEach(r => r.addEventListener('change', syncMode));
  form.addEventListener('submit', async e => {
    if (e.submitter?.value === 'cancel') return;
    e.preventDefault();
    const f = form.elements;
    let done;
    if (f.mode.value === 'yaml') {
      const text = f.yaml.value;
      done = /^\s*workflow\s*:/m.test(text)
        ? run(fleet.submitWorkflow(text), w => `Workflow ${w.name} started`)
        : run(fleet.submit(text), j => `Queued ${j.spec.task}`);
    } else {
      const labels = f.labels.value.split(/[\s,]+/).filter(Boolean);
      const job = { task: f.task.value.trim().replace(/[^\w-]+/g, '-') || 'job', run: f.run.value, requires: { labels } };
      if (f.cwd.value.trim()) job.cwd = f.cwd.value.trim();
      if (f.timeout.value.trim()) job.timeout = f.timeout.value.trim();
      if (f.gpu.checked) job.resources = { gpu: 1 };
      done = run(fleet.submit(job), j => `Queued ${j.spec.task}`);
    }
    if (await done) { jobSheet.close(); load(); }
  });

  function openNew() {
    syncMode();
    jobSheet.showModal();
  }

  return { load, start, stop, available, openNew };
}
