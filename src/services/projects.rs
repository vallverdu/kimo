//! Projects: folders agents work in. Created empty (optionally `git init`), added from an
//! existing folder or cloned (the clone runs in a visible terminal). Files are never deleted.

use crate::core::git::{branch_name, parse_head};
use crate::core::models::{Project, SessionKind, SessionStatus};
use crate::services::sessions::{canonical, slug, Manager, NewSession, SessionInfo};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct ProjectInfo {
    #[serde(flatten)]
    pub project: Project,
    pub exists: bool,
    pub branch: Option<String>,
    pub sessions: usize,
    pub running: usize,
    /// The project's folder first (`primary`), then its other git worktrees.
    pub worktrees: Vec<Worktree>,
}

/// A git worktree: a checkout of another branch of the same repository in its own folder.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    /// The project's own folder.
    pub primary: bool,
}

/// Changes to a project (`PATCH /projects/{id}`); missing fields stay as they are.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectChanges {
    #[serde(default)]
    pub name: Option<String>,
    /// `#rrggbb`, or an empty string for none.
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewWorktree {
    /// Branch name (made safe for git); an existing branch is checked out.
    pub name: String,
    /// Start an agent in it: `claude`, `codex` or `shell`.
    #[serde(default)]
    pub agent: Option<SessionKind>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedWorktree {
    pub worktree: Worktree,
    pub session: Option<SessionInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectMode {
    /// New folder inside `path` (or the projects folder).
    New,
    /// Register the folder at `path`.
    Existing,
    /// `git clone url` into a new folder inside `path` (or the projects folder).
    Clone,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewProject {
    #[serde(default)]
    pub name: String,
    pub mode: ProjectMode,
    #[serde(default)]
    pub path: Option<String>,
    /// `git init` a new folder.
    #[serde(default)]
    pub git: bool,
    #[serde(default)]
    pub url: Option<String>,
    /// Continue the folder's latest Claude and Codex conversations (existing folders).
    #[serde(default = "yes")]
    pub resume: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedProject {
    pub project: Project,
    /// The terminal running `git clone`, when cloning.
    pub session: Option<SessionInfo>,
    /// Earlier conversations in the folder, continued as sessions (the first one is started).
    pub continued: Vec<SessionInfo>,
    /// The folder already was a project: nothing was added, its conversations were opened.
    pub reopened: bool,
}

/// Folder name for a project name: lowercase words joined by dashes.
fn folder_name(name: &str) -> String {
    name.to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_'))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Repository name from a clone URL (`git@host:me/app.git` → `app`).
fn repo_name(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".git")
        .to_string()
}

/// The git directory of a checkout: `.git` is a folder in a normal checkout and a
/// `gitdir: …` file in a worktree.
fn git_dir(path: &Path) -> Option<PathBuf> {
    let git = path.join(".git");
    if git.is_dir() {
        return Some(git);
    }
    let pointer = std::fs::read_to_string(&git).ok()?;
    let target = PathBuf::from(pointer.trim().strip_prefix("gitdir:")?.trim());
    Some(if target.is_absolute() {
        target
    } else {
        path.join(target)
    })
}

/// Current branch of a checkout.
fn branch(path: &Path) -> Option<String> {
    parse_head(&std::fs::read_to_string(git_dir(path)?.join("HEAD")).ok()?)
}

/// The folder itself and the other worktrees of its repository, from the files git keeps in
/// `.git/worktrees` (no git process: this runs on every project listing).
fn worktrees(path: &Path) -> Vec<Worktree> {
    let Some(dir) = git_dir(path) else {
        return vec![];
    };
    // A linked worktree points to the shared directory through `commondir`.
    let common = match std::fs::read_to_string(dir.join("commondir")) {
        Ok(c) => {
            let c = PathBuf::from(c.trim());
            if c.is_absolute() {
                c
            } else {
                dir.join(c)
            }
        }
        Err(_) => dir,
    };
    let mut list = vec![Worktree {
        path: path.display().to_string(),
        branch: branch(path),
        primary: true,
    }];
    let mut others: Vec<Worktree> = std::fs::read_dir(common.join("worktrees"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let gitdir = std::fs::read_to_string(e.path().join("gitdir")).ok()?;
            let folder = Path::new(gitdir.trim()).parent()?.to_path_buf();
            (folder.is_dir() && folder != path).then(|| Worktree {
                path: folder.display().to_string(),
                branch: std::fs::read_to_string(e.path().join("HEAD"))
                    .ok()
                    .and_then(|h| parse_head(&h)),
                primary: false,
            })
        })
        .collect();
    // The repository's main checkout, when the project is itself a linked worktree.
    if let Some(main) = common
        .file_name()
        .filter(|n| *n == ".git")
        .and(common.parent())
    {
        if main != path && main.is_dir() {
            others.push(Worktree {
                path: main.display().to_string(),
                branch: branch(main),
                primary: false,
            });
        }
    }
    others.sort_by(|a, b| a.path.cmp(&b.path));
    list.extend(others);
    list
}

/// Run git in `dir`; its error message when it fails.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = crate::services::network::run_for(
        Command::new("git").args(args).current_dir(dir),
        std::time::Duration::from_secs(60),
    )
    .context("running git")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .map(|l| {
                l.trim_start_matches("fatal: ")
                    .trim_start_matches("error: ")
            })
            .find(|l| !l.trim().is_empty())
            .unwrap_or("git failed");
        bail!("{line}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Manager {
    pub fn projects(&self) -> Vec<ProjectInfo> {
        let sessions = self.sessions();
        let projects = self.lock().state.projects.clone();
        projects
            .into_iter()
            .map(|p| {
                let inside: Vec<_> = sessions
                    .iter()
                    .filter(|s| Path::new(&s.spec.cwd).starts_with(&p.path))
                    .collect();
                let path = PathBuf::from(&p.path);
                ProjectInfo {
                    exists: path.is_dir(),
                    branch: branch(&path),
                    worktrees: worktrees(&path),
                    sessions: inside.len(),
                    running: inside
                        .iter()
                        .filter(|s| s.status == SessionStatus::Running)
                        .count(),
                    project: p,
                }
            })
            .collect()
    }

    pub fn create_project(&self, req: NewProject) -> Result<CreatedProject> {
        let parent = match &req.path {
            Some(p) if !p.trim().is_empty() => PathBuf::from(self.expand_home(p.trim())),
            _ => self.cfg.projects_dir.clone(),
        };
        let url = req.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
        let (name, dir) = match req.mode {
            ProjectMode::Existing => {
                if !parent.is_dir() {
                    bail!("folder not found: {}", parent.display());
                }
                let base = parent
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let name = if req.name.trim().is_empty() {
                    base
                } else {
                    req.name.trim().to_string()
                };
                (name, parent)
            }
            ProjectMode::New | ProjectMode::Clone => {
                let name = match (req.name.trim(), req.mode, url) {
                    ("", ProjectMode::Clone, Some(u)) => repo_name(u),
                    (n, _, _) => n.to_string(),
                };
                let folder = folder_name(&name);
                if folder.is_empty() {
                    bail!("the project needs a name");
                }
                let dir = parent.join(folder);
                if dir.exists()
                    && std::fs::read_dir(&dir)
                        .map(|mut d| d.next().is_some())
                        .unwrap_or(true)
                {
                    bail!(
                        "{} already exists: add it as an existing folder instead",
                        dir.display()
                    );
                }
                (name, dir)
            }
        };
        if name.is_empty() {
            bail!("the project needs a name");
        }
        let path = dir.display().to_string();
        let known = self
            .lock()
            .state
            .projects
            .iter()
            .find(|p| p.path == path)
            .cloned();
        if let Some(project) = known {
            if req.mode != ProjectMode::Existing {
                bail!("{path} is already a project");
            }
            // Adding a project again opens it: its latest conversations continue.
            let continued = if req.resume {
                self.continue_latest(&project.name, &project.path)
            } else {
                vec![]
            };
            return Ok(CreatedProject {
                project,
                session: None,
                continued,
                reopened: true,
            });
        }
        if req.mode == ProjectMode::Clone && url.is_none() {
            bail!("paste the git URL to clone");
        }
        if req.mode != ProjectMode::Existing {
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {path}"))?;
        }
        if req.mode == ProjectMode::New && req.git {
            let out = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&dir)
                .output()
                .context("running git init")?;
            if !out.status.success() {
                bail!(
                    "git init failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
        }
        let project = Project {
            id: slug(&name),
            name: name.clone(),
            path: path.clone(),
            color: None,
            extra: Default::default(),
        };
        {
            let mut inner = self.lock();
            inner.state.projects.push(project.clone());
            self.save(&inner)?;
        }
        let session = match (req.mode, url) {
            (ProjectMode::Clone, Some(url)) => Some(self.create(
                NewSession {
                    name: format!("clone {name}"),
                    kind: SessionKind::Shell,
                    cwd: path,
                    account: None,
                    args: vec![],
                    command: Some(vec![
                        "git".into(),
                        "clone".into(),
                        "--progress".into(),
                        url.into(),
                        ".".into(),
                    ]),
                },
                true,
            )?),
            _ => None,
        };
        let continued = if req.mode == ProjectMode::Existing && req.resume {
            self.continue_latest(&project.name, &project.path)
        } else {
            vec![]
        };
        Ok(CreatedProject {
            project,
            session,
            continued,
            reopened: false,
        })
    }

    /// Forget a project. Its folder and files stay untouched.
    pub fn delete_project(&self, id: &str) -> Result<()> {
        let mut inner = self.lock();
        let before = inner.state.projects.len();
        inner.state.projects.retain(|p| p.id != id);
        if inner.state.projects.len() == before {
            return Err(anyhow!("unknown project: {id}"));
        }
        self.save(&inner)
    }

    /// Rename a project or change its colour (the folder stays where it is).
    pub fn update_project(&self, id: &str, changes: ProjectChanges) -> Result<Project> {
        let mut inner = self.lock();
        let project = inner
            .state
            .projects
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow!("unknown project: {id}"))?;
        if let Some(name) = changes.name {
            let name = name.trim();
            if name.is_empty() {
                bail!("the project needs a name");
            }
            project.name = name.to_string();
        }
        if let Some(color) = changes.color {
            let color = color.trim().to_lowercase();
            let valid = color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|b| b.is_ascii_hexdigit());
            if !color.is_empty() && !valid {
                bail!("colours look like #12939a");
            }
            project.color = (!color.is_empty()).then_some(color);
        }
        let project = project.clone();
        self.save(&inner)?;
        Ok(project)
    }

    fn project(&self, id: &str) -> Result<Project> {
        self.lock()
            .state
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown project: {id}"))
    }

    /// A new git worktree of the project (a branch in its own folder, under
    /// `~/.kimo/worktrees/<repository folder>/`), with an agent started in it when asked.
    pub fn add_worktree(&self, id: &str, req: NewWorktree) -> Result<CreatedWorktree> {
        let project = self.project(id)?;
        let repo = PathBuf::from(&project.path);
        if git_dir(&repo).is_none() {
            bail!("{} is not a git repository", project.name);
        }
        let branch = branch_name(&req.name);
        if branch.is_empty() {
            bail!("the worktree needs a name");
        }
        let repo_folder = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| project.id.clone());
        let folder = self
            .cfg
            .data_dir
            .join("worktrees")
            .join(repo_folder)
            .join(branch.replace('/', "-"));
        if folder.exists() {
            bail!("{} already exists", folder.display());
        }
        std::fs::create_dir_all(folder.parent().unwrap_or(&folder))?;
        let target = folder.display().to_string();
        let exists = git(
            &repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        if exists {
            git(&repo, &["worktree", "add", &target, &branch])?;
        } else {
            git(&repo, &["worktree", "add", "-b", &branch, &target])?;
        }
        // As git records it (macOS: /var is /private/var), so listings and sessions match.
        let target = canonical(&target);
        let worktree = Worktree {
            path: target.clone(),
            branch: Some(branch.clone()),
            primary: false,
        };
        let session = match req.agent {
            Some(kind) => Some(self.create(
                NewSession {
                    name: format!("{} {branch}", project.name),
                    kind,
                    cwd: target,
                    account: None,
                    args: vec![],
                    command: None,
                },
                true,
            )?),
            None => None,
        };
        Ok(CreatedWorktree { worktree, session })
    }

    /// Remove one of the project's worktrees with `git worktree remove`: git refuses when it
    /// has uncommitted changes, and so does kimo while an agent runs in it. The branch stays.
    pub fn remove_worktree(&self, id: &str, path: &str) -> Result<Vec<String>> {
        let project = self.project(id)?;
        let repo = PathBuf::from(&project.path);
        let known = worktrees(&repo)
            .into_iter()
            .any(|w| !w.primary && canonical(&w.path) == canonical(path));
        if !known {
            bail!("{path} is not a worktree of {}", project.name);
        }
        let path = &canonical(path);
        let inside: Vec<SessionInfo> = self
            .sessions()
            .into_iter()
            .filter(|s| Path::new(&s.spec.cwd).starts_with(path))
            .collect();
        if inside
            .iter()
            .any(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Paused))
        {
            bail!("stop the agents working in this worktree first");
        }
        git(&repo, &["worktree", "remove", path])?;
        Ok(inside.into_iter().map(|s| s.spec.id).collect())
    }

    /// Create a folder `name` inside `parent` (for folder pickers: start a project from
    /// nothing). Returns the listing of the new folder.
    pub fn make_folder(&self, parent: &str, name: &str) -> Result<Listing> {
        let name = name.trim();
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            bail!("folder names cannot be empty or contain /");
        }
        let parent = PathBuf::from(self.expand_home(parent.trim()));
        if !parent.is_dir() {
            bail!("folder not found: {}", parent.display());
        }
        let dir = parent.join(name);
        if dir.exists() {
            bail!("{} already exists", dir.display());
        }
        std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.browse(Some(&dir.display().to_string()))
    }

    /// Subfolders of `path` (default: the projects folder) for the folder picker.
    pub fn browse(&self, path: Option<&str>) -> Result<Listing> {
        let dir = match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => PathBuf::from(self.expand_home(p)),
            None if self.cfg.projects_dir.is_dir() => self.cfg.projects_dir.clone(),
            None => self.cfg.user_home.clone(),
        };
        let mut dirs: Vec<Entry> = std::fs::read_dir(&dir)
            .with_context(|| format!("cannot open {}", dir.display()))?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.')).then(|| Entry {
                    git: e.path().join(".git").exists(),
                    path: e.path().display().to_string(),
                    name,
                })
            })
            .collect();
        dirs.sort_by_key(|d| d.name.to_lowercase());
        Ok(Listing {
            path: dir.display().to_string(),
            parent: dir.parent().map(|p| p.display().to_string()),
            git: dir.join(".git").exists(),
            dirs,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub git: bool,
    pub dirs: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub git: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_of_checkouts_and_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(branch(&repo).as_deref(), Some("main"));
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
        std::fs::write(
            repo.join(".git/worktrees/wt/HEAD"),
            "ref: refs/heads/feature\n",
        )
        .unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        assert_eq!(branch(&wt).as_deref(), Some("feature"));
        assert_eq!(branch(tmp.path()), None);
    }

    #[test]
    fn names() {
        assert_eq!(folder_name("Acme Invoices v2"), "acme-invoices-v2");
        assert_eq!(folder_name("  /// "), "");
        assert_eq!(repo_name("git@github.com:someone/kimo.git"), "kimo");
        assert_eq!(repo_name("https://github.com/someone/kimo/"), "kimo");
    }
}
