//! The right panel of the window (like Orca's): the shown project's files, its git changes
//! (stage, commit, push, pull request) and its agents; and files opened as tabs next to the
//! terminals, to read, edit and save. Everything goes through the project's files and git
//! API (`/projects/{id}/…`), so it works the same for a project on a paired kimo.

use super::*;
use std::collections::{HashMap, HashSet};

/// Which project folder the panel shows: the kimo, the project and (for a worktree) its folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Scope {
    pub host: String,
    pub project: String,
    pub root: Option<String>,
}

impl Scope {
    /// `?root=…` (or `&root=…`) for a worktree; nothing for the project's own folder.
    fn root_query(&self, first: bool) -> String {
        match &self.root {
            Some(r) => format!("{}root={}", if first { "?" } else { "&" }, query_escape(r)),
            None => String::new(),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Files,
    Changes,
    Agents,
}

#[derive(Default)]
pub(super) struct Details {
    pub open: bool,
    pub tab: Tab,
    pub scope: Option<Scope>,
    /// Folder entries by folder ("" = the project's own).
    pub tree: HashMap<String, Vec<Value>>,
    pub expanded: HashSet<String>,
    pub find: String,
    pub contents: bool,
    pub found: Vec<Value>,
    /// The search last asked for, and when the text last changed.
    pub asked: String,
    pub typed_at: Option<Instant>,
    pub git: Value,
    pub log: Vec<Value>,
    pub last_git: Option<Instant>,
    pub message: String,
    pub pr_title: String,
    pub busy: Option<&'static str>,
    pub conversations: Vec<Value>,
    pub conversations_for: Option<String>,
}

/// A file opened in a tab.
pub(super) struct Editor {
    pub scope: Scope,
    pub path: String,
    pub text: String,
    /// The text as last read or saved (unsaved changes = it differs).
    pub saved: String,
    /// When it was read (unix ms): saving refuses a newer copy on disk.
    pub modified: u64,
    /// Why it cannot be edited here (binary, too large, an error); `None` while editable.
    pub note: Option<String>,
    pub loading: bool,
    pub diff: Option<String>,
    pub show_diff: bool,
}

impl Editor {
    pub fn dirty(&self) -> bool {
        !self.loading && self.note.is_none() && self.text != self.saved
    }

    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// What came back from the API for the panel or an editor.
pub(super) enum Loaded {
    Dir {
        scope: Scope,
        dir: String,
        list: Value,
    },
    Found {
        scope: Scope,
        query: String,
        list: Value,
    },
    Git {
        scope: Scope,
        state: Value,
        log: Value,
    },
    Conversations {
        path: String,
        list: Value,
    },
    File {
        scope: Scope,
        path: String,
        file: Value,
    },
    Diff {
        scope: Scope,
        path: String,
        diff: String,
    },
    /// A git action finished: a note, then everything is read again.
    Done(String),
    /// A git action failed: why.
    Failed(String),
}

/// A git status letter's colour, as editors show them.
fn letter_color(letter: &str) -> Color32 {
    match letter {
        "M" | "R" | "C" => theme::warn(),
        "A" | "U" => theme::good(),
        "D" | "!" => theme::bad(),
        _ => theme::muted(),
    }
}

impl KimoApp {
    /// The project folder the panel shows: the open project, in the worktree of the active
    /// terminal when it works in one.
    pub(super) fn details_scope(&self) -> Option<Scope> {
        let project = self.project.clone()?;
        let host = self.host().id.clone();
        let path = self
            .projects
            .iter()
            .find(|p| s(p, "id") == project)
            .map(|p| s(p, "path").to_string())?;
        let root = self
            .terminals
            .get(self.active)
            .filter(|t| t.host == host)
            .and_then(|t| {
                self.sessions()
                    .into_iter()
                    .find(|x| s(x, "id") == t.session_id)
            })
            .and_then(|x| self.home_of(&x))
            .filter(|(p, w)| *p == project && *w != path)
            .map(|(_, w)| w);
        Some(Scope {
            host,
            project,
            root,
        })
    }

    /// Keep the panel's data current: everything again when its folder changes, git every
    /// few seconds, a search shortly after typing stops.
    pub(super) fn details_poll(&mut self) {
        if !self.details.open {
            return;
        }
        let scope = self.details_scope();
        if scope != self.details.scope {
            self.details.scope = scope.clone();
            self.details.tree.clear();
            self.details.expanded.clear();
            self.details.found.clear();
            self.details.asked.clear();
            self.details.git = Value::Null;
            self.details.log.clear();
            self.details.conversations.clear();
            self.details.conversations_for = None;
            if scope.is_some() {
                self.load_dir("");
                self.load_git();
            }
        }
        if scope.is_none() {
            return;
        }
        if self
            .details
            .last_git
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            self.load_git();
        }
        let text = self.details.find.trim().to_string();
        let asked = format!("{}{text}", if self.details.contents { "c:" } else { "n:" });
        if !text.is_empty()
            && asked != self.details.asked
            && self
                .details
                .typed_at
                .is_some_and(|t| t.elapsed() > Duration::from_millis(250))
        {
            self.details.asked = asked;
            let Some(sc) = self.details.scope.clone() else {
                return;
            };
            let contents = self.details.contents;
            let query = text.clone();
            self.job(move |c| {
                let path = format!(
                    "/projects/{}/find?q={}{}{}",
                    sc.project,
                    query_escape(&query),
                    if contents { "&contents=1" } else { "" },
                    sc.root_query(false)
                );
                let list = c.get(&path)?;
                Ok(Msg::Details(Loaded::Found {
                    scope: sc,
                    query,
                    list,
                }))
            });
        }
    }

    fn load_dir(&mut self, dir: &str) {
        let Some(sc) = self.details.scope.clone() else {
            return;
        };
        let dir = dir.to_string();
        self.job(move |c| {
            let list = c.get(&format!(
                "/projects/{}/files?dir={}{}",
                sc.project,
                query_escape(&dir),
                sc.root_query(false)
            ))?;
            Ok(Msg::Details(Loaded::Dir {
                scope: sc,
                dir,
                list,
            }))
        });
    }

    fn load_git(&mut self) {
        self.details.last_git = Some(Instant::now());
        let Some(sc) = self.details.scope.clone() else {
            return;
        };
        self.job(move |c| {
            let state = c.get(&format!(
                "/projects/{}/git{}",
                sc.project,
                sc.root_query(true)
            ))?;
            let log = c
                .get(&format!(
                    "/projects/{}/git/log?limit=20{}",
                    sc.project,
                    sc.root_query(false)
                ))
                .unwrap_or(Value::Null);
            Ok(Msg::Details(Loaded::Git {
                scope: sc,
                state,
                log,
            }))
        });
    }

    /// Everything again (after a save or a git action).
    fn reload_details(&mut self) {
        let dirs: Vec<String> = std::iter::once(String::new())
            .chain(self.details.expanded.iter().cloned())
            .collect();
        for d in dirs {
            self.load_dir(&d);
        }
        self.load_git();
    }

    pub(super) fn on_loaded(&mut self, loaded: Loaded) {
        let current = self.details.scope.clone();
        match loaded {
            Loaded::Dir { scope, dir, list } if Some(&scope) == current.as_ref() => {
                self.details
                    .tree
                    .insert(dir, list.as_array().cloned().unwrap_or_default());
            }
            Loaded::Found { scope, query, list } if Some(&scope) == current.as_ref() => {
                if self.details.find.trim() == query {
                    self.details.found = list.as_array().cloned().unwrap_or_default();
                }
            }
            Loaded::Git { scope, state, log } if Some(&scope) == current.as_ref() => {
                self.details.git = state;
                self.details.log = log.as_array().cloned().unwrap_or_default();
            }
            Loaded::Conversations { path, list } => {
                if self.details.conversations_for.as_deref() == Some(path.as_str()) {
                    self.details.conversations = list.as_array().cloned().unwrap_or_default();
                }
            }
            Loaded::File { scope, path, file } => {
                if let Some(e) = self
                    .editors
                    .iter_mut()
                    .find(|e| e.scope == scope && e.path == path)
                {
                    let reopened = !e.loading;
                    e.loading = false;
                    e.modified = file["modified"].as_u64().unwrap_or(0);
                    match file["text"].as_str() {
                        Some(text) => {
                            e.note = None;
                            // Reading again keeps what was typed only when nothing was.
                            if !reopened || !e.dirty() {
                                e.text = text.to_string();
                            }
                            e.saved = text.to_string();
                        }
                        None => {
                            e.note = Some(if file["binary"] == true {
                                "A binary file: open it with another app.".into()
                            } else {
                                format!(
                                    "Too large to show here ({}).",
                                    mb(file["size"].as_u64().unwrap_or(0))
                                )
                            });
                        }
                    }
                }
            }
            Loaded::Diff { scope, path, diff } => {
                if let Some(e) = self
                    .editors
                    .iter_mut()
                    .find(|e| e.scope == scope && e.path == path)
                {
                    e.diff = Some(diff);
                }
            }
            Loaded::Done(note) => {
                self.details.busy = None;
                self.toast(note, false);
                self.reload_details();
            }
            Loaded::Failed(why) => {
                self.details.busy = None;
                self.toast(why, true);
                self.reload_details();
            }
            _ => {}
        }
    }

    /// Open a file in a tab (or show it when it is open already).
    pub(super) fn open_file(&mut self, scope: Scope, path: &str, diff: bool) {
        let i = match self
            .editors
            .iter()
            .position(|e| e.scope == scope && e.path == path)
        {
            Some(i) => i,
            None => {
                self.editors.push(Editor {
                    scope: scope.clone(),
                    path: path.to_string(),
                    text: String::new(),
                    saved: String::new(),
                    modified: 0,
                    note: None,
                    loading: true,
                    diff: None,
                    show_diff: false,
                });
                self.read_file(&scope, path);
                self.editors.len() - 1
            }
        };
        if diff {
            self.editors[i].show_diff = true;
            self.load_diff(&scope, path);
        }
        self.editor_active = Some(i);
        self.view = View::Terminals;
    }

    fn read_file(&mut self, scope: &Scope, path: &str) {
        let (sc, path) = (scope.clone(), path.to_string());
        self.job(move |c| {
            let file = c.get(&format!(
                "/projects/{}/file?path={}{}",
                sc.project,
                query_escape(&path),
                sc.root_query(false)
            ))?;
            Ok(Msg::Details(Loaded::File {
                scope: sc,
                path,
                file,
            }))
        });
    }

    fn load_diff(&mut self, scope: &Scope, path: &str) {
        let staged = self.details.git["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| s(c, "path") == path && c["staged"] == true && c["unstaged"] == false);
        let (sc, path) = (scope.clone(), path.to_string());
        self.job(move |c| {
            let v: Value = c.get(&format!(
                "/projects/{}/git/diff?path={}{}{}",
                sc.project,
                query_escape(&path),
                if staged { "&staged=1" } else { "" },
                sc.root_query(false)
            ))?;
            Ok(Msg::Details(Loaded::Diff {
                scope: sc,
                path,
                diff: s(&v, "diff").to_string(),
            }))
        });
    }

    pub(super) fn save_editor(&mut self, i: usize) {
        let Some(e) = self.editors.get(i) else { return };
        if !e.dirty() {
            return;
        }
        let (sc, path, text, modified) =
            (e.scope.clone(), e.path.clone(), e.text.clone(), e.modified);
        self.job(move |c| {
            let body = json!({ "path": path, "text": text, "modified": modified, "root": sc.root });
            let file = c.put(&format!("/projects/{}/file", sc.project), body)?;
            Ok(Msg::Details(Loaded::File {
                scope: sc,
                path,
                file,
            }))
        });
        self.toast(format!("Saved {}", self.editors[i].name()), false);
        self.details.last_git = None;
    }

    /// A git action on the panel's folder, then everything is read again.
    fn git_action(&mut self, busy: &'static str, done: String, path: &'static str, body: Value) {
        let Some(sc) = self.details.scope.clone() else {
            return;
        };
        self.details.busy = Some(busy);
        let mut body = body;
        body["root"] = json!(sc.root);
        self.job(move |c| {
            let r = c.post(&format!("/projects/{}/git/{path}", sc.project), body);
            Ok(match r {
                Ok(v) if path == "pr" => {
                    let url = s(&v, "url").to_string();
                    if !url.is_empty() {
                        open_url(&url);
                    }
                    Msg::Details(Loaded::Done(format!("Pull request: {url}")))
                }
                Ok(_) => Msg::Details(Loaded::Done(done)),
                Err(e) => Msg::Details(Loaded::Failed(format!("{e}"))),
            })
        });
    }

    /// The panel (right side of the window).
    pub(super) fn details_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (tab, label) in [
                (Tab::Files, "Files"),
                (Tab::Changes, "Changes"),
                (Tab::Agents, "Agents"),
            ] {
                let selected = self.details.tab == tab;
                let count = match tab {
                    Tab::Changes => self.details.git["changes"]
                        .as_array()
                        .map_or(0, |c| c.len()),
                    _ => 0,
                };
                let text = if count > 0 {
                    format!("{label} {count}")
                } else {
                    label.to_string()
                };
                let button = egui::Button::new(RichText::new(text).size(12.5).color(if selected {
                    theme::background()
                } else {
                    theme::ink()
                }))
                .fill(if selected {
                    theme::ink()
                } else {
                    theme::surface()
                })
                .corner_radius(6);
                if ui.add(button).clicked() {
                    self.details.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("↻").color(theme::muted())).frame(false))
                    .on_hover_text("read again")
                    .clicked()
                {
                    self.reload_details();
                }
            });
        });
        ui.add_space(6.0);
        let Some(scope) = self.details.scope.clone() else {
            ui.label(
                RichText::new(
                    "Open a project on the left: its files, changes and agents show here.",
                )
                .color(theme::muted()),
            );
            return;
        };
        match self.details.tab {
            Tab::Files => self.files_ui(ui, &scope),
            Tab::Changes => self.changes_ui(ui, &scope),
            Tab::Agents => self.agents_ui(ui, &scope),
        }
    }

    fn letters(&self) -> HashMap<String, String> {
        self.details.git["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| (s(c, "path").to_string(), s(c, "letter").to_string()))
            .collect()
    }

    fn files_ui(&mut self, ui: &mut Ui, scope: &Scope) {
        ui.horizontal(|ui| {
            for (contents, label) in [(false, "Names"), (true, "Contents")] {
                if ui
                    .selectable_label(self.details.contents == contents, label)
                    .clicked()
                {
                    self.details.contents = contents;
                    self.details.typed_at = Some(Instant::now());
                }
            }
        });
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.details.find)
                .hint_text("Find files")
                .desired_width(f32::INFINITY),
        );
        if r.changed() {
            self.details.typed_at = Some(Instant::now());
            if self.details.find.trim().is_empty() {
                self.details.found.clear();
                self.details.asked.clear();
            }
        }
        ui.add_space(4.0);
        let letters = self.letters();
        let mut open: Option<(String, bool)> = None;
        let mut toggle: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if !self.details.find.trim().is_empty() {
                    if self.details.found.is_empty() {
                        ui.label(
                            RichText::new("Nothing found yet.")
                                .size(11.5)
                                .color(theme::muted()),
                        );
                    }
                    for f in &self.details.found {
                        let path = s(f, "path");
                        let text = match f["line"].as_u64() {
                            Some(line) => format!("{path}:{line}  {}", s(f, "text").trim()),
                            None => path.to_string(),
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(text).size(12.0).color(theme::ink()),
                                )
                                .frame(false)
                                .truncate(),
                            )
                            .clicked()
                        {
                            open = Some((path.to_string(), false));
                        }
                    }
                    return;
                }
                let mut rows: Vec<(Value, usize)> = vec![];
                collect(&self.details.tree, &self.details.expanded, "", 0, &mut rows);
                for (e, depth) in rows {
                    let path = s(&e, "path").to_string();
                    let dir = e["dir"] == true;
                    let ignored = e["ignored"] == true;
                    let letter = if dir {
                        letters
                            .keys()
                            .any(|p| p.starts_with(&format!("{path}/")))
                            .then(|| "•".to_string())
                    } else {
                        letters.get(&path).cloned()
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 14.0);
                        let icon = if dir {
                            if self.details.expanded.contains(&path) {
                                "▾"
                            } else {
                                "▸"
                            }
                        } else {
                            " "
                        };
                        let color = match &letter {
                            Some(l) => letter_color(l),
                            None if ignored => theme::muted(),
                            None => theme::ink(),
                        };
                        let name = RichText::new(format!("{icon} {}", s(&e, "name")))
                            .size(12.5)
                            .color(color);
                        let name = if ignored { name.italics() } else { name };
                        let r = ui.add(egui::Button::new(name).frame(false).truncate());
                        if r.clicked() {
                            if dir {
                                toggle = Some(path.clone());
                            } else {
                                open = Some((path.clone(), false));
                            }
                        }
                        if let Some(l) = &letter {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(RichText::new(l).size(11.0).color(letter_color(l)));
                                },
                            );
                        }
                    });
                }
                if self.details.tree.is_empty() {
                    ui.label(
                        RichText::new("Reading the folder…")
                            .size(11.5)
                            .color(theme::muted()),
                    );
                }
            });
        if let Some(dir) = toggle {
            if !self.details.expanded.remove(&dir) {
                self.details.expanded.insert(dir.clone());
                if !self.details.tree.contains_key(&dir) {
                    self.load_dir(&dir);
                }
            }
        }
        if let Some((path, diff)) = open {
            self.open_file(scope.clone(), &path, diff);
        }
    }

    fn changes_ui(&mut self, ui: &mut Ui, scope: &Scope) {
        let git = self.details.git.clone();
        if git.is_null() {
            ui.label(
                RichText::new("Reading git…")
                    .size(11.5)
                    .color(theme::muted()),
            );
            return;
        }
        if git["repo"] != true {
            ui.label(RichText::new("This folder is not a git repository.").color(theme::muted()));
            return;
        }
        let branch = git["branch"].as_str().unwrap_or("(no branch)");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(branch).strong().color(theme::ink()));
            if let Some(up) = git["upstream"].as_str() {
                ui.label(
                    RichText::new(format!("→ {up}"))
                        .size(11.5)
                        .color(theme::muted()),
                );
            }
            let (ahead, behind) = (
                git["ahead"].as_u64().unwrap_or(0),
                git["behind"].as_u64().unwrap_or(0),
            );
            if ahead + behind > 0 {
                ui.label(
                    RichText::new(format!("↑{ahead} ↓{behind}"))
                        .size(11.5)
                        .color(theme::text()),
                );
            }
        });
        let changes: Vec<Value> = git["changes"].as_array().cloned().unwrap_or_default();
        let staged: Vec<&Value> = changes.iter().filter(|c| c["staged"] == true).collect();
        let unstaged: Vec<&Value> = changes.iter().filter(|c| c["unstaged"] == true).collect();
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.details.message)
                .hint_text("Commit message")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        let busy = self.details.busy;
        let mut action: Option<(&'static str, String, &'static str, Value)> = None;
        ui.horizontal_wrapped(|ui| {
            if let Some(b) = busy {
                ui.spinner();
                ui.label(RichText::new(b).size(11.5).color(theme::muted()));
                return;
            }
            if !unstaged.is_empty() && ui.button("Stage all").clicked() {
                action = Some(("Staging…", "Staged".into(), "stage", json!({})));
            }
            let can_commit = !staged.is_empty() && !self.details.message.trim().is_empty();
            let commit = ui
                .add_enabled(can_commit, egui::Button::new("Commit"))
                .on_disabled_hover_text(if staged.is_empty() {
                    "stage the changes first"
                } else {
                    "write a message"
                });
            if commit.clicked() {
                let message = self.details.message.trim().to_string();
                action = Some((
                    "Committing…",
                    "Committed".into(),
                    "commit",
                    json!({ "message": message }),
                ));
                self.details.message.clear();
            }
            let ahead = git["ahead"].as_u64().unwrap_or(0);
            if (ahead > 0 || git["upstream"].is_null())
                && ui
                    .button("Push")
                    .on_hover_text("push the branch (git push)")
                    .clicked()
            {
                action = Some(("Pushing…", "Pushed".into(), "push", json!({})));
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.details.pr_title)
                    .hint_text("Pull request title")
                    .desired_width(ui.available_width() - 110.0),
            );
            let can = busy.is_none() && !self.details.pr_title.trim().is_empty();
            if ui
                .add_enabled(can, egui::Button::new("Create PR"))
                .on_hover_text("with GitHub's gh; push the branch first")
                .clicked()
            {
                let title = self.details.pr_title.trim().to_string();
                action = Some((
                    "Opening the pull request…",
                    String::new(),
                    "pr",
                    json!({ "title": title }),
                ));
            }
        });
        let mut open: Option<String> = None;
        let mut stage: Option<(bool, String)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (title, list, staged_list) in
                    [("STAGED", &staged, true), ("CHANGES", &unstaged, false)]
                {
                    if list.is_empty() {
                        continue;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("{title}  {}", list.len()))
                            .size(10.5)
                            .color(theme::muted()),
                    );
                    for c in list.iter() {
                        let path = s(c, "path").to_string();
                        let letter = s(c, "letter").to_string();
                        let (dir, name) = match path.rsplit_once('/') {
                            Some((d, n)) => (d.to_string(), n.to_string()),
                            None => (String::new(), path.clone()),
                        };
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(&name).size(12.5).color(theme::ink()),
                                    )
                                    .frame(false)
                                    .truncate(),
                                )
                                .on_hover_text(&path)
                                .clicked()
                            {
                                open = Some(path.clone());
                            }
                            ui.label(RichText::new(dir).size(10.5).color(theme::muted()));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(&letter)
                                            .size(11.0)
                                            .color(letter_color(&letter)),
                                    );
                                    let (sign, hint) = if staged_list {
                                        ("−", "unstage")
                                    } else {
                                        ("+", "stage")
                                    };
                                    if busy.is_none()
                                        && ui
                                            .add(egui::Button::new(sign).frame(false))
                                            .on_hover_text(hint)
                                            .clicked()
                                    {
                                        stage = Some((!staged_list, path.clone()));
                                    }
                                },
                            );
                        });
                    }
                }
                if changes.is_empty() {
                    ui.label(
                        RichText::new("No changes.")
                            .size(11.5)
                            .color(theme::muted()),
                    );
                }
                if !self.details.log.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("COMMITS").size(10.5).color(theme::muted()));
                    for c in &self.details.log {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(s(c, "hash"))
                                    .size(10.5)
                                    .monospace()
                                    .color(theme::muted()),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(s(c, "subject"))
                                        .size(12.0)
                                        .color(theme::text()),
                                )
                                .truncate(),
                            );
                        });
                    }
                }
            });
        if let Some(path) = open {
            self.open_file(scope.clone(), &path, true);
        }
        if let Some((add, path)) = stage {
            let (busy, route) = if add {
                ("Staging…", "stage")
            } else {
                ("Unstaging…", "unstage")
            };
            action = Some((busy, String::new(), route, json!({ "paths": [path] })));
        }
        if let Some((busy, done, route, body)) = action {
            let done = if done.is_empty() {
                "Done".to_string()
            } else {
                done
            };
            self.git_action(busy, done, route, body);
        }
    }

    fn agents_ui(&mut self, ui: &mut Ui, scope: &Scope) {
        let folder = scope.root.clone().or_else(|| {
            self.projects
                .iter()
                .find(|p| s(p, "id") == scope.project)
                .map(|p| s(p, "path").to_string())
        });
        let Some(folder) = folder else { return };
        if self.details.conversations_for.as_deref() != Some(folder.as_str()) {
            self.details.conversations_for = Some(folder.clone());
            self.details.conversations.clear();
            let path = folder.clone();
            self.job(move |c| {
                let list = c.get(&format!("/conversations?path={}", query_escape(&path)))?;
                Ok(Msg::Details(Loaded::Conversations { path, list }))
            });
        }
        let project = Some(scope.project.clone());
        let mine: Vec<Value> = self
            .sessions()
            .into_iter()
            .filter(|x| self.project_of(x) == project)
            .collect();
        let mut open: Option<String> = None;
        let mut resume: Option<Value> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("AGENTS  {}", mine.len()))
                        .size(10.5)
                        .color(theme::muted()),
                );
                for x in &mine {
                    ui.horizontal(|ui| {
                        dot(ui, theme::status_color(s(x, "status")));
                        ui.label(
                            RichText::new(kind_label(s(x, "kind")))
                                .size(10.0)
                                .color(theme::muted()),
                        );
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(s(x, "name")).size(12.5).color(theme::ink()),
                                )
                                .frame(false)
                                .truncate(),
                            )
                            .clicked()
                        {
                            open = Some(s(x, "id").to_string());
                        }
                        activity_tag(ui, x);
                    });
                }
                if mine.is_empty() {
                    ui.label(
                        RichText::new("No agents in this project yet.")
                            .size(11.5)
                            .color(theme::muted()),
                    );
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "EARLIER CONVERSATIONS  {}",
                        self.details.conversations.len()
                    ))
                    .size(10.5)
                    .color(theme::muted()),
                );
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                for c in &self.details.conversations {
                    let title = s(c, "title");
                    let r = ui.add(
                        egui::Button::new(
                            RichText::new(if title.is_empty() {
                                "(untitled)"
                            } else {
                                title
                            })
                            .size(12.5)
                            .color(theme::ink()),
                        )
                        .frame(false)
                        .truncate(),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} · {} · {} asks{}",
                            s(c, "agent"),
                            ago(now_ms, c["updated_at"].as_u64().unwrap_or(0)),
                            c["messages"].as_u64().unwrap_or(0),
                            if c["session"].is_null() {
                                ""
                            } else {
                                " · open"
                            }
                        ))
                        .size(10.5)
                        .color(theme::muted()),
                    );
                    if r.on_hover_text("continue this conversation").clicked() {
                        resume = Some(c.clone());
                    }
                }
            });
        if let Some(id) = open {
            self.open_session(&id);
        }
        if let Some(c) = resume {
            if let Some(id) = c["session"].as_str() {
                self.open_session(id);
            } else {
                let body = json!({ "agent": c["agent"], "cwd": folder, "id": c["id"], "name": short_title(s(&c, "title")), "start": true });
                self.job(move |cl| Ok(Msg::Open(cl.post("/conversations/continue", body)?)));
            }
        }
    }

    /// An open file: its text to edit (⌘S saves), or its changes.
    pub(super) fn editor_view(&mut self, ui: &mut Ui, i: usize) {
        let save_key = ui.input_mut(|inp| inp.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        let mut save = save_key;
        let mut reload = false;
        let mut diff = false;
        {
            let e = &mut self.editors[i];
            ui.horizontal(|ui| {
                ui.label(RichText::new(&e.path).size(12.0).color(theme::muted()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let shortcut = if cfg!(target_os = "macos") {
                        "⌘S"
                    } else {
                        "Ctrl+S"
                    };
                    if ui
                        .add_enabled(e.dirty(), egui::Button::new("Save"))
                        .on_hover_text(shortcut)
                        .clicked()
                    {
                        save = true;
                    }
                    if ui
                        .selectable_label(e.show_diff, "Changes")
                        .on_hover_text("what changed since the last commit")
                        .clicked()
                    {
                        e.show_diff = !e.show_diff;
                        diff = e.show_diff;
                    }
                    if ui
                        .button("Reload")
                        .on_hover_text("read it again from disk")
                        .clicked()
                    {
                        reload = true;
                    }
                });
            });
        }
        if save {
            self.save_editor(i);
        }
        if reload {
            let (sc, path) = (self.editors[i].scope.clone(), self.editors[i].path.clone());
            self.editors[i].saved = self.editors[i].text.clone();
            self.read_file(&sc, &path);
            self.editors[i].text = String::new();
            self.editors[i].loading = true;
        }
        if diff {
            let (sc, path) = (self.editors[i].scope.clone(), self.editors[i].path.clone());
            self.load_diff(&sc, &path);
        }
        let font = self
            .settings
            .as_ref()
            .map_or(13.0, |st| st.terminal.font_size);
        let e = &mut self.editors[i];
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, theme::line()))
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                if e.loading {
                    ui.spinner();
                    return;
                }
                if let Some(note) = &e.note {
                    ui.label(RichText::new(note).color(theme::muted()));
                    return;
                }
                if e.show_diff {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match &e.diff {
                            None => {
                                ui.spinner();
                            }
                            Some(d) if d.trim().is_empty() => {
                                ui.label(
                                    RichText::new("No changes since the last commit.")
                                        .color(theme::muted()),
                                );
                            }
                            Some(d) => {
                                for line in d.lines() {
                                    let color = if line.starts_with('+') && !line.starts_with("+++")
                                    {
                                        theme::good()
                                    } else if line.starts_with('-') && !line.starts_with("---") {
                                        theme::bad()
                                    } else if line.starts_with("@@") {
                                        theme::teal()
                                    } else {
                                        theme::text()
                                    };
                                    ui.label(
                                        RichText::new(line).monospace().size(font).color(color),
                                    );
                                }
                            }
                        });
                    return;
                }
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            // Line numbers in the same font, so rows line up.
                            let lines =
                                e.text.lines().count().max(1) + usize::from(e.text.ends_with('\n'));
                            let numbers: String = (1..=lines).map(|n| format!("{n}\n")).collect();
                            ui.label(
                                RichText::new(numbers.trim_end())
                                    .monospace()
                                    .size(font)
                                    .color(theme::muted()),
                            );
                            ui.add(
                                egui::TextEdit::multiline(&mut e.text)
                                    .font(egui::FontId::monospace(font))
                                    .code_editor()
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(lines.max(30)),
                            );
                        });
                    });
            });
    }
}

/// A folder's entries and those of its expanded folders, in display order.
fn collect(
    tree: &HashMap<String, Vec<Value>>,
    expanded: &HashSet<String>,
    dir: &str,
    depth: usize,
    out: &mut Vec<(Value, usize)>,
) {
    let Some(list) = tree.get(dir) else { return };
    for e in list {
        out.push((e.clone(), depth));
        let path = s(e, "path");
        if e["dir"] == true && expanded.contains(path) {
            collect(tree, expanded, path, depth + 1, out);
        }
    }
}
