// Projects view: folders agents work in, one-tap agents per project, and the folder picker.
import { $, esc, shortPath, toast } from './ui.js';

export function initProjects({ api, state, run, terminal }) {
  const sheet = $('sheet-project'), form = sheet.querySelector('form'), folderSheet = $('sheet-folder');

  const inside = p => state.sessions.filter(s => s.cwd === p.path || s.cwd.startsWith(`${p.path}/`));

  function render() {
    $('project-list').innerHTML = state.projects.length ? state.projects.map(p => {
      const sessions = inside(p);
      const running = sessions.filter(s => s.status === 'running').length;
      return `<li data-project="${esc(p.id)}">
        <span class="k-name" style="cursor:default">${esc(p.name)}</span>
        <span class="ui-badge ${running ? 'running' : 'stopped'}">${running ? `${running} running` : 'idle'}</span>
        <div class="k-meta"><code title="${esc(p.path)}">${esc(shortPath(p.path))}</code>${p.branch ? `<span>git · ${esc(p.branch)}</span>` : '<span>no git</span>'}${p.exists ? '' : '<span class="k-warn">folder missing</span>'}</div>
        ${sessions.length ? `<ul class="k-chips">${sessions.map(s => `<li><button data-open="${esc(s.id)}"><span class="ui-badge ${s.status}"></span>${esc(s.name)}</button></li>`).join('')}</ul>` : ''}
        <div class="k-actions">
          <button class="ui-button k-small" data-new="claude">+ claude</button>
          <button class="ui-button ghost k-small" data-new="codex">+ codex</button>
          <button class="ui-button ghost k-small" data-new="shell">+ shell</button>
          <button class="ui-button k-small k-danger" data-do="forget">forget</button>
        </div></li>`;
    }).join('') : '<li class="ui-empty">No projects yet. Tap + to create one, add a folder or clone a repository.</li>';
  }

  $('project-list').addEventListener('click', async e => {
    const p = state.projects.find(x => x.id === e.target.closest('li[data-project]')?.dataset.project);
    if (!p) return;
    const open = e.target.closest('[data-open]')?.dataset.open;
    if (open) return terminal.open(state.sessions.find(s => s.id === open));
    const kind = e.target.closest('[data-new]')?.dataset.new;
    if (kind) {
      const taken = new Set(state.sessions.map(s => s.name));
      let name = `${p.name} ${kind}`;
      for (let n = 2; taken.has(name); n++) name = `${p.name} ${kind} ${n}`;
      const s = await run(api.createSession({ name, kind, cwd: p.path }));
      if (s) terminal.open(s);
      return;
    }
    if (e.target.closest('[data-do=forget]') && confirm(`Forget "${p.name}"? Its folder and files stay on disk.`)) {
      run(api.deleteProject(p.id), 'Project forgotten (files kept)');
    }
  });

  // ----- new project sheet -----

  function syncMode() {
    const mode = form.elements.mode.value;
    form.querySelector('[data-for=url]').hidden = mode !== 'clone';
    form.querySelector('[data-for=git]').hidden = mode !== 'new';
    form.querySelector('[data-for=name] span').textContent = mode === 'existing' ? 'Name (default: the folder name)' : mode === 'clone' ? 'Name (default: the repository name)' : 'Name';
    form.elements.name.required = mode === 'new';
    form.querySelector('[data-for=path] span').textContent = mode === 'existing' ? 'Folder' : 'Create inside';
    form.elements.path.required = mode === 'existing';
  }
  form.querySelectorAll('[name=mode]').forEach(r => r.addEventListener('change', syncMode));

  form.addEventListener('submit', async e => {
    e.preventDefault();
    const f = form.elements;
    const body = { mode: f.mode.value, name: f.name.value, path: f.path.value || null, git: f.git.checked, url: f.url.value || null };
    const made = await run(api.createProject(body), body.mode === 'clone' ? 'Cloning…' : 'Project ready');
    if (!made) return;
    sheet.close();
    form.reset();
    syncMode();
    if (made.session) terminal.open(made.session);
  });

  // ----- folder picker -----

  let pickTarget = null;
  async function browse(path) {
    try {
      const l = await api.browse(path);
      $('folder-path').textContent = shortPath(l.path, 30);
      $('folder-path').title = l.path;
      folderSheet.dataset.path = l.path;
      folderSheet.dataset.parent = l.parent || '';
      $('folder-list').innerHTML = (l.dirs.length ? l.dirs.map(d =>
        `<li><button type="button" data-path="${esc(d.path)}">${esc(d.name)}${d.git ? ' <small>git</small>' : ''}</button></li>`).join('')
        : '<li class="k-hint">No subfolders.</li>');
    } catch (err) { toast(err.message, true); }
  }
  folderSheet.addEventListener('click', e => {
    const dir = e.target.closest('[data-path]')?.dataset.path;
    if (dir) browse(dir);
    if (e.target.closest('[data-do=up]') && folderSheet.dataset.parent) browse(folderSheet.dataset.parent);
    if (e.target.closest('[data-do=use]')) {
      pickTarget.value = folderSheet.dataset.path.replace(/^\/home\/[^/]+/, '~');
      pickTarget.title = folderSheet.dataset.path;
      pickTarget.dispatchEvent(new Event('change'));
      folderSheet.close();
    }
  });
  function pickFolder(input) {
    pickTarget = input;
    browse(input.value);
    folderSheet.showModal();
  }
  document.querySelectorAll('[data-browse]').forEach(b => b.addEventListener('click', () => pickFolder(b.closest('form').elements[b.dataset.browse])));

  syncMode();
  return { render, open: () => { syncMode(); sheet.showModal(); } };
}
