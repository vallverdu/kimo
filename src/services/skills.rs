//! Agent skills on disk: list them (global and per project), install from GitHub, a git URL or
//! a folder, and remove them by moving them to a backup. Never deletes what it did not find in a
//! skills folder, and leaves synced (claude.ai) and built-in skills alone.

use crate::core::skills::{github_shorthand, parse_front_matter, valid_name};
use crate::services::sessions::Manager;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which agent reads a skills folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillAgent {
    Claude,
    Codex,
    /// `.agents/skills`, read by several agents.
    Shared,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub agent: SkillAgent,
    /// `global` or the project id.
    pub scope: String,
    pub path: String,
    /// Synced from claude.ai or built into the agent: read-only here.
    pub managed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallSkill {
    /// `owner/repo[/dir]`, a GitHub URL, a git URL or a local folder.
    pub source: String,
    /// Only the skill with this name (default: every skill found).
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default = "default_agents")]
    pub agents: Vec<SkillAgent>,
    /// Project id for a project skill; none = global.
    #[serde(default)]
    pub project: Option<String>,
    /// Replace existing skills with the same name (the old one is backed up).
    #[serde(default)]
    pub replace: bool,
}

fn default_agents() -> Vec<SkillAgent> {
    vec![SkillAgent::Claude]
}

fn roots(base: &Path) -> [(SkillAgent, PathBuf); 3] {
    [
        (SkillAgent::Claude, base.join(".claude/skills")),
        (SkillAgent::Codex, base.join(".codex/skills")),
        (SkillAgent::Shared, base.join(".agents/skills")),
    ]
}

/// Every `SKILL.md` up to three folders deep (synced skills are nested).
fn scan(root: &Path, agent: SkillAgent, scope: &str, out: &mut Vec<Skill>) {
    fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        if dir.join("SKILL.md").is_file() {
            found.push(dir.to_path_buf());
            return;
        }
        if depth == 0 {
            return;
        }
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let hidden_trash = name == ".trash" || name == ".git";
            if !hidden_trash && entry.path().is_dir() {
                walk(&entry.path(), depth - 1, found);
            }
        }
    }
    let mut dirs = vec![];
    walk(root, 3, &mut dirs);
    for dir in dirs {
        let text = fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
        let (name, description) = parse_front_matter(&text);
        let rel = dir
            .strip_prefix(root)
            .unwrap_or(&dir)
            .to_string_lossy()
            .into_owned();
        out.push(Skill {
            name: name.unwrap_or_else(|| {
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
            description: description.unwrap_or_default(),
            agent,
            scope: scope.to_string(),
            path: dir.display().to_string(),
            managed: rel.starts_with("synced/")
                || rel.starts_with(".system/")
                || rel.starts_with('.'),
        });
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_name() == ".git" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

impl Manager {
    fn backups_dir(&self) -> PathBuf {
        self.cfg.data_dir.join("skill-backups")
    }

    fn skill_base(&self, project: Option<&str>) -> Result<(PathBuf, String)> {
        match project {
            None => Ok((self.cfg.user_home.clone(), "global".into())),
            Some(id) => {
                let p = self
                    .lock()
                    .state
                    .projects
                    .iter()
                    .find(|p| p.id == id)
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown project: {id}"))?;
                Ok((PathBuf::from(p.path), p.id))
            }
        }
    }

    /// Global skills plus those of every project (or only `project`).
    pub fn skills(&self, project: Option<&str>) -> Result<Vec<Skill>> {
        let mut out = vec![];
        let projects: Vec<(PathBuf, String)> = match project {
            Some(id) => vec![self.skill_base(Some(id))?],
            None => {
                let mut all = vec![self.skill_base(None)?];
                all.extend(
                    self.lock()
                        .state
                        .projects
                        .iter()
                        .map(|p| (PathBuf::from(&p.path), p.id.clone())),
                );
                all
            }
        };
        for (base, scope) in projects {
            for (agent, root) in roots(&base) {
                scan(&root, agent, &scope, &mut out);
            }
        }
        out.sort_by(|a, b| {
            (a.scope != "global", &a.scope, a.name.to_lowercase()).cmp(&(
                b.scope != "global",
                &b.scope,
                b.name.to_lowercase(),
            ))
        });
        Ok(out)
    }

    /// Install skills from a source into the chosen agents' folders. Returns what was installed.
    pub fn install_skill(&self, req: InstallSkill) -> Result<Vec<Skill>> {
        if req.agents.is_empty() {
            bail!("choose at least one agent");
        }
        let (base, scope) = self.skill_base(req.project.as_deref())?;
        let source = req.source.trim();
        let tmp = self
            .cfg
            .data_dir
            .join("tmp")
            .join(uuid::Uuid::new_v4().simple().to_string());
        let local = PathBuf::from(self.expand_home(source));
        let source_dir = if local.is_dir() {
            local
        } else {
            let (url, sub) = match github_shorthand(source) {
                Some(found) => found,
                None if source.ends_with(".git")
                    || source.starts_with("git@")
                    || source.starts_with("https://") =>
                {
                    (source.to_string(), String::new())
                }
                None => bail!(
                    "source must be owner/repo[/folder], a git URL or a folder on this machine"
                ),
            };
            fs::create_dir_all(&tmp)?;
            let out = Command::new("git")
                .args(["clone", "--depth", "1", "--quiet", &url])
                .arg(tmp.join("repo"))
                .output()
                .context("running git clone")?;
            if !out.status.success() {
                let _ = fs::remove_dir_all(&tmp);
                bail!(
                    "git clone failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            tmp.join("repo").join(sub)
        };
        let result = (|| {
            let mut found = vec![];
            scan(&source_dir, SkillAgent::Shared, "source", &mut found);
            if let Some(only) = &req.skill {
                found.retain(|s| &s.name == only);
            }
            if found.is_empty() {
                bail!(
                    "no SKILL.md found in {source}{}",
                    req.skill
                        .as_ref()
                        .map(|s| format!(" named {s}"))
                        .unwrap_or_default()
                );
            }
            let mut installed = vec![];
            for skill in &found {
                let folder = Path::new(&skill.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let name = if valid_name(&skill.name) {
                    skill.name.clone()
                } else {
                    folder
                };
                if !valid_name(&name) {
                    bail!("skill name '{name}' cannot be a folder name");
                }
                for agent in &req.agents {
                    let root = roots(&base)
                        .into_iter()
                        .find(|(a, _)| a == agent)
                        .map(|(_, r)| r)
                        .unwrap_or_default();
                    let target = root.join(&name);
                    if fs::symlink_metadata(&target).is_ok() {
                        if !req.replace {
                            bail!(
                                "{} already exists: choose replace to update it",
                                target.display()
                            );
                        }
                        self.backup(&target)?;
                    }
                    copy_dir(Path::new(&skill.path), &target)
                        .with_context(|| format!("installing into {}", target.display()))?;
                    installed.push(Skill {
                        path: target.display().to_string(),
                        agent: *agent,
                        scope: scope.clone(),
                        managed: false,
                        name: name.clone(),
                        description: skill.description.clone(),
                    });
                }
            }
            Ok(installed)
        })();
        let _ = fs::remove_dir_all(&tmp);
        result
    }

    fn backup(&self, dir: &Path) -> Result<PathBuf> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Unique even for two backups of the same skill in the same second.
        let unique = &uuid::Uuid::new_v4().simple().to_string()[..6];
        let to = self.backups_dir().join(format!("{stamp}-{name}-{unique}"));
        fs::create_dir_all(self.backups_dir())?;
        fs::rename(dir, &to)
            .or_else(|_| copy_dir(dir, &to).and_then(|_| Ok(fs::remove_dir_all(dir)?)))?;
        Ok(to)
    }

    /// Remove a skill by moving it to `~/.kimo/skill-backups`. Returns where it went.
    pub fn remove_skill(&self, path: &str) -> Result<String> {
        let skill = self
            .skills(None)?
            .into_iter()
            .find(|s| s.path == path)
            .ok_or_else(|| anyhow!("not a skill kimo knows: {path}"))?;
        if skill.managed {
            bail!(
                "'{}' is synced or built in: manage it where it comes from",
                skill.name
            );
        }
        Ok(self.backup(Path::new(&skill.path))?.display().to_string())
    }
}

/// An agent CLI on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCli {
    pub kind: &'static str,
    pub installed: bool,
    pub version: Option<String>,
}

pub fn detect_agents() -> Vec<AgentCli> {
    ["claude", "codex"]
        .into_iter()
        .map(|kind| {
            let out = Command::new(kind)
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success());
            AgentCli {
                kind,
                installed: out.is_some(),
                version: out.map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                }),
            }
        })
        .collect()
}
