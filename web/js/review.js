// A project's files and changes on the phone: browse and read files, see what changed (with
// the diff of each file). Reading only; agents (or the window) do the editing and commits.
import { $, esc, toast } from './ui.js';

export function initReview({ api }) {
  const sheet = $('sheet-review');
  const body = sheet.querySelector('.k-review-body');
  const title = sheet.querySelector('h3');
  // The kimo and project the sheet was opened for; answers for anything else are dropped.
  let ctx = null, asked = 0;

  async function load(promise, render) {
    const ticket = ++asked;
    body.innerHTML = '<p class="k-hint">Loading…</p>';
    try {
      const v = await promise;
      if (ticket === asked) render(v);
    } catch (e) {
      if (ticket === asked) body.innerHTML = `<p class="k-hint">${esc(e.message)}</p>`;
    }
  }

  function showDir(dir) {
    title.textContent = `${ctx.project.name}${dir ? ` / ${dir}` : ''}`;
    load(ctx.to.files(ctx.project.id, dir), list => {
      const up = dir ? `<li><button type="button" class="k-name" data-dir="${esc(dir.split('/').slice(0, -1).join('/'))}">‹ back</button></li>` : '';
      body.innerHTML = `<ul class="k-machine-list">${up}${list.map(e => `<li><button type="button" class="k-name${e.ignored ? ' k-dim' : ''}" ${e.dir ? `data-dir="${esc(e.path)}"` : `data-file="${esc(e.path)}"`}>${e.dir ? '▸ ' : ''}${esc(e.name)}</button><span><small>${e.dir ? '' : size(e.size)}</small></span></li>`).join('') || '<li>Empty folder.</li>'}</ul>`;
    });
  }

  function showFile(path) {
    title.textContent = path;
    load(ctx.to.readFile(ctx.project.id, path), f => {
      const back = `<button type="button" class="ui-button ghost k-small" data-dir="${esc(path.split('/').slice(0, -1).join('/'))}">‹ files</button>`;
      body.innerHTML = back + (f.text === null
        ? `<p class="k-hint">${f.binary ? 'A binary file.' : 'Too large to show here.'}</p>`
        : `<pre class="k-code k-review-text">${esc(f.text)}</pre>`);
    });
  }

  function showChanges() {
    title.textContent = `${ctx.project.name}: changes`;
    load(ctx.to.gitStatus(ctx.project.id), g => {
      if (!g.repo) { body.innerHTML = '<p class="k-hint">Not a git repository.</p>'; return; }
      const head = `<p class="k-hint">${esc(g.branch || '(no branch)')}${g.upstream ? ` → ${esc(g.upstream)}` : ''}${g.ahead || g.behind ? ` · ↑${g.ahead} ↓${g.behind}` : ''}</p>`;
      body.innerHTML = head + (g.changes.length
        ? `<ul class="k-machine-list">${g.changes.map(c => `<li><button type="button" class="k-name" data-diff="${esc(c.path)}" data-staged="${c.staged && !c.unstaged ? 1 : ''}">${esc(c.path)}</button><span><strong class="k-letter k-letter-${esc(c.letter)}">${esc(c.letter)}</strong></span></li>`).join('')}</ul>`
        : '<p class="k-hint">No changes.</p>');
    });
  }

  function showDiff(path, staged) {
    title.textContent = path;
    load(ctx.to.gitDiff(ctx.project.id, path, { staged }), d => {
      const lines = d.diff.split('\n').map(l => {
        const cls = l.startsWith('+') && !l.startsWith('+++') ? 'k-add' : l.startsWith('-') && !l.startsWith('---') ? 'k-del' : l.startsWith('@@') ? 'k-hunk' : '';
        return `<span class="${cls}">${esc(l)}</span>`;
      }).join('\n');
      body.innerHTML = `<button type="button" class="ui-button ghost k-small" data-changes>‹ changes</button><pre class="k-code k-review-text">${lines || 'No changes.'}</pre>`;
    });
  }

  sheet.addEventListener('click', e => {
    const t = e.target.closest('[data-dir],[data-file],[data-diff],[data-changes]');
    if (!t || !ctx) return;
    if (t.dataset.file !== undefined) return showFile(t.dataset.file);
    if (t.dataset.diff !== undefined) return showDiff(t.dataset.diff, !!t.dataset.staged);
    if (t.dataset.changes !== undefined) return showChanges();
    showDir(t.dataset.dir);
  });
  // The close event comes after the fact: a sheet opened again meanwhile keeps its answers.
  sheet.addEventListener('close', () => { if (!sheet.open) { ctx = null; asked++; } });

  return {
    /** Open a project's files (`mode` 'files') or changes ('changes'). */
    open(project, mode) {
      ctx = { project, to: api.forHost(api.host) };
      sheet.showModal();
      if (mode === 'changes') showChanges(); else showDir('');
    },
  };
}

function size(bytes) {
  return bytes >= 1 << 20 ? `${(bytes / (1 << 20)).toFixed(1)} MB` : bytes >= 1024 ? `${Math.round(bytes / 1024)} kB` : `${bytes} B`;
}
