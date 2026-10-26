// Projects view: folders agents work in, one-tap agents per project, and the folder picker.
import { $, esc, shortPath, toast } from './ui.js';

export function initProjects({ api, state, run, terminal }) {
  const sheet = $('sheet-project'), form = sheet.querySelector('form'), folderSheet = $('sheet-folder');

  const under = (path, s) => s.cwd === path || s.cwd.startsWith(`${path}/`);
  const trees = p => p.worktrees?.length ? p.worktrees : [{ path: p.path, branch: p.branch, primary: true }];
  const inside = p => state.sessions.filter(s => trees(p).some(w => under(w.path, s)));
  const actions = (path, extra = '') => `<div class="k-actions" data-cwd="${esc(path)}">
          <button class="ui-button k-small" data-new="claude">+ claude</button>
          <button class="ui-button ghost k-small" data-new="codex">+ codex</button>
          <button class="ui-button ghost k-small" data-new="shell">+ shell</button>${extra}</div>`;

  function render() {
    $('project-list').innerHTML = state.projects.length ? state.projects.map(p => {
      const sessions = inside(p);
      const running = sessions.filter(s => s.status === 'running').length;
      const chips = list => list.length ? `<ul class="k-chips">${list.map(s => `<li><button data-open="${esc(s.id)}"><span class="ui-badge ${s.status}"></span>${esc(s.name)}</button></li>`).join('')}</ul>` : '';
      const [main, ...others] = trees(p);
      const mainSessions = sessions.filter(s => under(main.path, s) && !others.some(w => under(w.path, s)));
      const mark = p.color ? `<i class="k-mark" style="background:${esc(p.color)}"></i>` : '';
      return `<li data-project="${esc(p.id)}">
        <span class="k-name" style="cursor:default">${mark}${esc(p.name)}</span>
        <span class="ui-badge ${running ? 'running' : 'stopped'}">${running ? `${running} running` : 'idle'}</span>
        <div class="k-meta"><code title="${esc(p.path)}">${esc(shortPath(p.path))}</code>${p.branch ? `<span>git · ${esc(p.branch)}</span>` : '<span>no git</span>'}${p.exists ? '' : '<span class="k-warn">folder missing</span>'}</div>
        ${chips(mainSessions)}
        ${actions(main.path, `
          <button class="ui-button ghost k-small" data-do="history" title="earlier Claude and Codex conversations here">history</button>
          ${p.branch ? '<button class="ui-button ghost k-small" data-do="worktree" title="a branch in its own folder, for an agent">+ worktree</button>' : ''}
          <button class="ui-button k-small k-danger" data-do="forget">forget</button>`)}
        ${others.map(w => `<div class="k-tree" data-tree="${esc(w.path)}">
          <div class="k-meta"><span>worktree · ${esc(w.branch || '?')}</span><code title="${esc(w.path)}">${esc(shortPath(w.path))}</code></div>
          ${chips(sessions.filter(s => under(w.path, s)))}
          ${actions(w.path, '<button class="ui-button k-small k-danger" data-do="remove-worktree">remove</button>')}</div>`).join('')}
        </li>`;
    }).join('') : '<li class="ui-empty">No projects yet. Tap + to create one, add a folder or clone a repository.</li>';
  }

  $('project-list').addEventListener('click', async e => {
    const p = state.projects.find(x => x.id === e.target.closest('li[data-project]')?.dataset.project);
    if (!p) return;
    const open = e.target.closest('[data-open]')?.dataset.open;
    if (open) return terminal.open(state.sessions.find(s => s.id === open));
    const kind = e.target.closest('[data-new]')?.dataset.new;
    if (kind) {
      const cwd = e.target.closest('[data-cwd]')?.dataset.cwd || p.path;
      const branch = cwd === p.path ? '' : ` ${trees(p).find(w => w.path === cwd)?.branch || ''}`;
      const taken = new Set(state.sessions.map(s => s.name));
      let name = `${p.name}${branch} ${kind}`;
      for (let n = 2; taken.has(name); n++) name = `${p.name}${branch} ${kind} ${n}`;
      const s = await run(api.createSession({ name, kind, cwd }));
      if (s) terminal.open(s);
      return;
    }
    if (e.target.closest('[data-do=history]')) return showHistory(p);
    if (e.target.closest('[data-do=worktree]')) {
      const name = prompt(`New worktree of ${p.name}: branch name (an existing branch is checked out)`);
      if (name?.trim()) run(api.addWorktree(p.id, { name: name.trim() }), r => `Worktree ready: ${shortPath(r.worktree.path)}`);
      return;
    }
    const tree = e.target.closest('[data-do=remove-worktree]') && e.target.closest('[data-tree]')?.dataset.tree;
    if (tree && confirm(`Remove the worktree ${shortPath(tree)}? git refuses if it has uncommitted changes; the branch stays.`)) {
      return run(api.removeWorktree(p.id, tree), 'Worktree removed');
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
    // A clone terminal, or the folder's last conversation that kimo continued.
    const n = made.continued?.length || 0;
    if (made.reopened) toast(n ? `${made.project.name} was already a project: opened it` : `${made.project.name} is already a project`);
    else if (n) toast(`Continuing ${n === 1 ? 'its last conversation' : `its last ${n} conversations`}`);
    const open = made.session || made.continued?.[0];
    if (open) terminal.open(open);
  });

  // ----- earlier conversations -----

  const historySheet = $('sheet-history');
  const ago = ms => {
    const m = Math.max(0, Math.round((Date.now() - ms) / 60000));
    return m < 60 ? `${m} min ago` : m < 1440 ? `${Math.round(m / 60)} h ago` : `${Math.round(m / 1440)} d ago`;
  };
  let historyProject = null;
  async function showHistory(p) {
    historyProject = p;
    $('history-title').textContent = `Earlier conversations · ${p.name}`;
    $('history-list').innerHTML = '<li class="k-hint">Looking…</li>';
    historySheet.showModal();
    try {
      const list = await api.conversations(p.path);
      $('history-list').innerHTML = list.length ? list.map(c => `<li data-conv="${esc(c.id)}" data-agent="${esc(c.agent)}" data-session="${esc(c.session || '')}">
        <div><strong>${esc(c.title)}</strong><small>${esc(c.agent)} · ${ago(c.updated_at)} · ${c.messages} asks</small></div>
        <button type="button" class="ui-button ${c.session ? 'ghost ' : ''}k-small">${c.session ? 'open' : 'continue'}</button></li>`).join('')
        : '<li class="k-hint">No Claude or Codex conversations in this folder yet.</li>';
    } catch (err) { toast(err.message, true); }
  }
  historySheet.addEventListener('click', async e => {
    const li = e.target.closest('button') && e.target.closest('li[data-conv]');
    if (!li) return;
    historySheet.close();
    if (li.dataset.session) {
      const s = state.sessions.find(x => x.id === li.dataset.session);
      if (s) terminal.open(s);
      return;
    }
    const s = await run(api.continueConversation({ agent: li.dataset.agent, cwd: historyProject.path, id: li.dataset.conv, name: li.querySelector('strong').textContent.slice(0, 32), start: true }), 'Continuing the conversation');
    if (s) terminal.open(s);
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
