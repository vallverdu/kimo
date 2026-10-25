//! Projects: folders agents work in. Created empty (optionally `git init`), added from an
//! existing folder or cloned (the clone runs in a visible terminal). Files are never deleted.

use crate::core::git::parse_head;
use crate::core::models::{Project, SessionKind, SessionStatus};
use crate::services::sessions::{slug, Manager, NewSession, SessionInfo};
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
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedProject {
    pub project: Project,
    /// The terminal running `git clone`, when cloning.
    pub session: Option<SessionInfo>,
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

fn branch(path: &Path) -> Option<String> {
    parse_head(&std::fs::read_to_string(path.join(".git/HEAD")).ok()?)
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
        if self.lock().state.projects.iter().any(|p| p.path == path) {
            bail!("{path} is already a project");
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
        Ok(CreatedProject { project, session })
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
    fn names() {
        assert_eq!(folder_name("Everpano Invoices v2"), "everpano-invoices-v2");
        assert_eq!(folder_name("  /// "), "");
        assert_eq!(repo_name("git@github.com:vallverdu/kimo.git"), "kimo");
        assert_eq!(repo_name("https://github.com/vallverdu/kimo/"), "kimo");
    }
}
