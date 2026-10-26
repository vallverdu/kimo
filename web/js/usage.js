// Usage view: tokens per day (Claude and Codex), where they went (projects) and on which
// subscription, for every agent conversation on this machine.
import { $, esc, shortPath, toast } from './ui.js';

const SERIES = [['claude', 'var(--k-claude)'], ['codex', 'var(--k-codex)']];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const SVG = 'http://www.w3.org/2000/svg';

const dollars = v => v >= 1000 ? `$${Math.round(v).toLocaleString('en-US')}` : `$${(v || 0).toFixed(2)}`;
const until = at => {
  const left = Math.max(0, at - Date.now() / 1000), d = Math.floor(left / 86400), h = Math.floor(left % 86400 / 3600), m = Math.floor(left % 3600 / 60);
  return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
};
const clock = at => new Date(at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
export const compact = n => n >= 1e9 ? `${(n / 1e9).toFixed(1)}B` : n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n);
const both = t => (t?.claude || 0) + (t?.codex || 0);
const shortDate = d => { const [, m, day] = d.split('-'); return `${Number(day)} ${MONTHS[Number(m) - 1]}`; };
const nice = x => { if (x <= 0) return 1; const p = 10 ** Math.floor(Math.log10(x)); return [1, 2, 5, 10].map(m => m * p).find(v => v >= x); };
function ago(ms) {
  if (!ms) return '';
  const min = Math.floor((Date.now() - ms) / 60000);
  return min < 1 ? 'just now' : min < 60 ? `${min} min ago` : min < 2880 ? `${Math.floor(min / 60)} h ago` : `${Math.floor(min / 1440)} d ago`;
}
function el(name, attrs = {}) {
  const e = document.createElementNS(SVG, name);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}
/** A column segment with only its top corners rounded (r ≤ 4). */
function topRounded(x, y, w, h, r) {
  r = Math.min(r, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

export function initUsage({ api, state, run }) {
  let days = 14;
  try { days = Number(localStorage.getItem('kimo-usage-days')) || 14; } catch { /* private mode */ }

  async function load() {
    try {
      [state.daily, state.usage] = await Promise.all([api.dailyUsage(days), api.usage().catch(() => state.usage)]);
      render();
    } catch (e) { toast(e.message, true); }
  }

  function chart(list) {
    const box = $('usage-chart');
    box.replaceChildren();
    const width = Math.max(260, box.clientWidth), height = 200, left = 40, top = 16, bottom = 22;
    const plotW = width - left, plotH = height - top - bottom;
    const max = Math.max(0, ...list.map(d => both(d.tokens)));
    const step = nice(max / 4), ceil = Math.max(1, Math.ceil(max / step)) * step;
    const y = v => top + plotH - v / ceil * plotH;
    const svg = el('svg', { viewBox: `0 0 ${width} ${height}`, width, height, role: 'img', 'aria-label': 'Tokens per day, Claude and Codex' });
    for (let v = 0; v <= ceil + 0.5; v += step) {
      const yy = Math.round(y(v)) + 0.5;
      svg.append(el('line', { x1: left, x2: width, y1: yy, y2: yy, class: v === 0 ? 'k-base' : 'k-grid' }));
      const t = el('text', { x: left - 6, y: yy + 3, 'text-anchor': 'end', class: 'k-axis' });
      t.textContent = compact(v);
      svg.append(t);
    }
    const n = Math.max(1, list.length), slot = plotW / n, bar = Math.max(2, Math.min(24, slot * 0.62));
    const every = Math.max(1, Math.ceil(n / 6));
    const peak = list.reduce((best, d, i) => both(d.tokens) > both(list[best]?.tokens) ? i : best, 0);
    list.forEach((d, i) => {
      const cx = left + slot * (i + 0.5), x = cx - bar / 2, base = top + plotH;
      const h1 = d.tokens.claude / ceil * plotH, h2 = d.tokens.codex / ceil * plotH;
      const g = el('g', { class: 'k-col', tabindex: 0, 'data-i': i,
        'aria-label': `${shortDate(d.date)}: claude ${compact(d.tokens.claude)}, codex ${compact(d.tokens.codex)}, ${d.prompts} prompts` });
      // The hit target is the whole slot, not the painted bar.
      g.append(el('rect', { x: left + slot * i, y: top, width: slot, height: plotH, class: 'k-hit' }));
      // Claude below, Codex above, split by a 2px surface gap; the top segment is rounded.
      if (h1 > 0) {
        const hh = Math.max(1, h1);
        g.append(el('path', { d: h2 > 0 ? `M${x},${base}V${base - hh}H${x + bar}V${base}Z` : topRounded(x, base - hh, bar, hh, 4), fill: SERIES[0][1] }));
      }
      if (h2 > 0) {
        const start = h1 > 0 ? base - h1 - 2 : base, hh = Math.max(1, h2);
        g.append(el('path', { d: topRounded(x, start - hh, bar, hh, 4), fill: SERIES[1][1] }));
      }
      if (i === peak && max > 0) {
        const t = el('text', { x: cx, y: base - h1 - h2 - (h1 && h2 ? 2 : 0) - 4, 'text-anchor': 'middle', class: 'k-peak' });
        t.textContent = compact(both(d.tokens));
        g.append(t);
      }
      if (i === n - 1 || (i % every === 0 && n - 1 - i >= Math.ceil(every / 2))) {
        const t = el('text', { x: cx, y: height - 6, 'text-anchor': 'middle', class: 'k-axis' });
        t.textContent = shortDate(d.date);
        svg.append(t);
      }
      svg.append(g);
    });
    box.append(svg);
    const tip = document.createElement('div');
    tip.className = 'k-tip';
    tip.hidden = true;
    box.append(tip);
    const showTip = g => {
      box.querySelectorAll('.k-col.on').forEach(c => c.classList.remove('on'));
      const d = list[g.dataset.i];
      g.classList.add('on');
      tip.replaceChildren();
      const head = document.createElement('div');
      head.className = 'k-tip-date';
      head.textContent = shortDate(d.date);
      tip.append(head);
      for (const [name, color] of [...SERIES].reverse()) {
        const row = document.createElement('div');
        row.innerHTML = `<i style="background:${color}"></i><strong></strong> <span></span>`;
        row.querySelector('strong').textContent = compact(d.tokens[name]);
        row.querySelector('span').textContent = name;
        tip.append(row);
      }
      const p = document.createElement('div');
      p.className = 'k-tip-date';
      p.textContent = `${d.prompts} prompts`;
      tip.append(p);
      tip.hidden = false;
      const i = Number(g.dataset.i), cx = left + slot * (i + 0.5);
      const tw = tip.offsetWidth;
      tip.style.left = `${Math.min(width - tw, Math.max(0, cx - tw / 2))}px`;
    };
    box.querySelectorAll('.k-col').forEach(g => {
      g.addEventListener('pointerenter', () => showTip(g));
      g.addEventListener('pointerdown', () => showTip(g));
      g.addEventListener('focus', () => showTip(g));
    });
    box.onpointerleave = e => { if (e.pointerType === 'mouse') { tip.hidden = true; box.querySelectorAll('.k-col.on').forEach(c => c.classList.remove('on')); } };
  }

  function render() {
    const r = state.daily;
    document.querySelectorAll('#usage-range [data-days]').forEach(b => b.classList.toggle('active', Number(b.dataset.days) === days));
    if (!r) return;
    const tile = (value, label, note) => `<div class="ui-stat"><strong>${esc(value)}</strong><span>${esc(label)}</span><small>${esc(note)}</small></div>`;
    const split = t => `claude ${compact(t.claude)} · codex ${compact(t.codex)}`;
    $('usage-stats').innerHTML = [
      tile(compact(both(r.today)), 'today', split(r.today)),
      tile(compact(both(r.last_7_days)), 'last 7 days', split(r.last_7_days)),
      tile(compact(both(r.total)), `last ${days} days`, split(r.total)),
      tile(r.prompts, 'prompts', `in ${r.conversations} conversations`),
      tile(dollars(r.cost_total), 'cost at API prices', `today ${dollars(r.cost_today)} · 7 days ${dollars(r.cost_7_days)}`),
    ].join('');
    $('usage-models').innerHTML = (r.models || []).length ? r.models.map(m => `<li><span class="k-name" style="cursor:default">${esc(m.model)}</span><span class="k-num">${dollars(m.cost)}</span>
      <div class="k-meta"><span>in ${compact(m.tokens.input)}</span><span>out ${compact(m.tokens.output)}</span><span>cache writes ${compact(m.tokens.cache_write)}</span><span>cache reads ${compact(m.tokens.cache_read)}</span><span>${m.answers} answers</span></div></li>`).join('')
      : '<li class="k-empty">No answers in these days.</li>';
    const paced = (state.usage || []).filter(u => u.limits?.limits?.length);
    $('usage-pace').innerHTML = paced.length ? paced.map(u => `<li><span class="k-name" style="cursor:default">${esc(u.name)}</span><span>${esc(u.provider)}</span>
      <div class="k-meta k-pace">${u.limits.limits.map(l => {
        const out = l.pace?.runs_out_at;
        const pace = l.used_percent >= 100 ? 'used up' : out ? `at this pace runs out in ${until(out)} (${clock(out)})` : l.pace ? `about ${Math.round(l.pace.projected_percent)}% by the reset` : '';
        return `<span class="${out || l.used_percent >= 100 ? 'k-warn' : ''}">${esc(l.name)} <span class="k-num">${Math.round(l.used_percent)}%</span>${l.resets_at ? ` · resets in ${until(l.resets_at)}` : ''}${pace ? ` · ${pace}` : ''}</span>`;
      }).join('')}</div></li>`).join('')
      : '<li class="k-empty">No plan limits known yet: Codex reports them as it works; Claude\'s show once turned on in the window (Subscriptions).</li>';
    chart(r.days);
    $('usage-table').innerHTML = `<thead><tr><th>day</th><th>claude</th><th>codex</th><th>prompts</th></tr></thead><tbody>${
      [...r.days].reverse().map(d => `<tr><td>${esc(d.date)}</td><td>${compact(d.tokens.claude)}</td><td>${compact(d.tokens.codex)}</td><td>${d.prompts}</td></tr>`).join('')}</tbody>`;

    const share = (claude, codex, scale) => {
      const w = v => (v / Math.max(1, scale) * 100).toFixed(2);
      return `<span class="k-share">${claude ? `<i style="width:${w(claude)}%;background:var(--k-claude)"></i>` : ''}${codex ? `<i style="width:${w(codex)}%;background:var(--k-codex)"></i>` : ''}</span>`;
    };
    const top = both(r.places[0]?.tokens);
    $('usage-places').innerHTML = r.places.length ? r.places.map(p => `<li data-path="${esc(p.path)}">
      <span class="k-name" style="cursor:default">${esc(p.name)}</span>
      <span class="k-num">${compact(both(p.tokens))}</span>
      ${share(p.tokens.claude, p.tokens.codex, top)}
      <div class="k-meta"><span>${esc(shortPath(p.path || '?'))}</span><span>${p.prompts} prompts</span><span>${p.conversations} conversations</span><span>${esc(ago(p.last_active))}</span>
        ${p.project ? '<span>project</span>' : p.path ? '<button class="ui-button ghost k-small" data-do="add-project">add as project</button>' : ''}</div></li>`).join('')
      : '<li class="k-empty">No agent work in these days.</li>';

    const outside = both(r.outside_kimo);
    const atop = Math.max(outside, ...r.accounts.map(a => a.tokens));
    $('usage-accounts').innerHTML = [
      ...r.accounts.map(a => `<li><span class="k-name" style="cursor:default">${esc(a.name)}</span><span class="k-num">${compact(a.tokens)}</span>
        ${share(a.provider === 'claude' ? a.tokens : 0, a.provider === 'codex' ? a.tokens : 0, atop)}
        <div class="k-meta"><span>${esc(a.provider)}</span><span>${a.prompts} prompts</span><span>${dollars(a.cost)}</span></div></li>`),
      outside ? `<li><span class="k-name" style="cursor:default">outside kimo</span><span class="k-num">${compact(outside)}</span>
        ${share(r.outside_kimo.claude, r.outside_kimo.codex, atop)}
        <div class="k-meta"><span>conversations not run by kimo: account not known</span></div></li>` : '',
    ].join('');
  }

  $('usage-range').addEventListener('click', e => {
    const b = e.target.closest('[data-days]');
    if (!b) return;
    days = Number(b.dataset.days);
    try { localStorage.setItem('kimo-usage-days', String(days)); } catch { /* ignore */ }
    render();
    load();
  });
  $('usage-places').addEventListener('click', e => {
    const path = e.target.closest('[data-do=add-project]') && e.target.closest('li[data-path]')?.dataset.path;
    if (path) run(api.createProject({ mode: 'existing', path, resume: false }), 'Project added').then(load);
  });
  let resize = null;
  addEventListener('resize', () => { clearTimeout(resize); resize = setTimeout(() => state.view === 'usage' && state.daily && chart(state.daily.days), 150); });

  return { load };
}
