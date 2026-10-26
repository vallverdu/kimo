//! A project's files and git repository for the window's right panel and the phone: listing,
//! finding, reading and saving files, and git status, diffs, staging, commits, pushes and pull
//! requests. Everything stays inside the project's own folders (its worktrees included).

use crate::core::files::{hidden, looks_binary, relative};
use crate::core::git::{parse_status, Status};
use crate::services::network::run_for;
use crate::services::sessions::Manager;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Files larger than this are not shown as text.
const MAX_READ: u64 = 2 << 20;
/// Nor saved.
const MAX_WRITE: usize = 5 << 20;
/// Entries listed per folder, and results per search.
const MAX_LIST: usize = 5000;
const MAX_FOUND: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    /// Relative to the project folder.
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// Ignored by git (build output, dependencies).
    pub ignored: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileText {
    pub path: String,
    /// `None` for binary or too large files.
    pub text: Option<String>,
    pub binary: bool,
    pub size: u64,
    /// Unix ms: sent back when saving, so a newer copy on disk is not overwritten.
    pub modified: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub path: String,
    /// Content matches: the line and its number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub author: String,
    /// Unix seconds.
    pub at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitState {
    /// The folder is a git repository.
    pub repo: bool,
    #[serde(flatten)]
    pub status: Status,
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// `git` in a folder, with a time limit; its error output becomes the error.
fn git(dir: &Path, args: &[&str], secs: u64) -> Result<String> {
    let out = run_for(
        Command::new("git")
            .args(args)
            .current_dir(dir)
            // Never wait for a password prompt nobody sees.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0"),
        Duration::from_secs(secs),
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
    /// The folder of a project (or one of its worktrees, `root`), resolved.
    pub fn project_root(&self, id: &str, root: Option<&str>) -> Result<PathBuf> {
        let project = self
            .lock()
            .state
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown project: {id}"))?;
        let folders = crate::services::projects::folders_of(&project);
        let folder = match root.filter(|r| !r.trim().is_empty()) {
            None => project.path.clone(),
            Some(r) => folders
                .into_iter()
                .find(|f| f == r || std::fs::canonicalize(f).ok() == std::fs::canonicalize(r).ok())
                .ok_or_else(|| anyhow!("{r} is not a folder of {}", project.name))?,
        };
        std::fs::canonicalize(&folder).with_context(|| format!("folder not found: {folder}"))
    }

    /// A path inside a project folder; symlinks may not lead out of it.
    fn inside(root: &Path, rel: &str, must_exist: bool) -> Result<PathBuf> {
        let rel = relative(rel).ok_or_else(|| anyhow!("{rel:?} is outside the project"))?;
        let path = root.join(&rel);
        let check = if path.exists() {
            std::fs::canonicalize(&path)?
        } else if must_exist {
            bail!("not found: {}", rel.display());
        } else {
            // A new file: its folder must be inside.
            let parent = path.parent().unwrap_or(root);
            std::fs::canonicalize(parent)
                .with_context(|| format!("folder not found: {}", parent.display()))?
                .join(path.file_name().unwrap_or_default())
        };
        if !check.starts_with(root) {
            bail!("{} leads outside the project", rel.display());
        }
        Ok(path)
    }

    pub fn list_files(&self, id: &str, root: Option<&str>, dir: &str) -> Result<Vec<Entry>> {
        let root = self.project_root(id, root)?;
        let folder = Self::inside(&root, dir, true)?;
        let base = relative(dir).unwrap_or_default();
        let mut entries: Vec<Entry> = std::fs::read_dir(&folder)
            .with_context(|| format!("reading {}", folder.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| !hidden(&e.file_name().to_string_lossy()))
            .take(MAX_LIST)
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let meta = std::fs::metadata(e.path()).ok();
                Entry {
                    path: base.join(&name).display().to_string(),
                    dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                    size: meta.as_ref().map_or(0, |m| m.len()),
                    name,
                    ignored: false,
                }
            })
            .collect();
        // Which of them git ignores (shown dimmed).
        if !entries.is_empty() && root.join(".git").exists() {
            let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
            let mut args = vec!["check-ignore", "--"];
            args.extend(paths.iter().map(String::as_str));
            // check-ignore exits 1 when nothing is ignored: that is not an error here.
            let out = run_for(
                Command::new("git").args(&args).current_dir(&root),
                Duration::from_secs(10),
            );
            if let Ok(out) = out {
                let ignored: Vec<String> = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(String::from)
                    .collect();
                for e in &mut entries {
                    e.ignored = ignored.contains(&e.path);
                }
            }
        }
        entries.sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
        Ok(entries)
    }

    /// Files whose path contains `query`, or (`contents`) lines that contain it.
    pub fn find_files(
        &self,
        id: &str,
        root: Option<&str>,
        query: &str,
        contents: bool,
    ) -> Result<Vec<Found>> {
        let root = self.project_root(id, root)?;
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }
        let repo = root.join(".git").exists();
        if contents {
            if !repo {
                bail!("searching contents needs a git repository");
            }
            let out = run_for(
                Command::new("git")
                    .args([
                        "grep",
                        "-n",
                        "-I",
                        "-i",
                        "--untracked",
                        "--fixed-strings",
                        "-e",
                        query,
                    ])
                    .current_dir(&root),
                Duration::from_secs(20),
            )?;
            return Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .take(MAX_FOUND)
                .filter_map(|l| {
                    let (path, rest) = l.split_once(':')?;
                    let (line, text) = rest.split_once(':')?;
                    Some(Found {
                        path: path.to_string(),
                        line: line.parse().ok(),
                        text: Some(text.chars().take(240).collect()),
                    })
                })
                .collect());
        }
        let needle = query.to_lowercase();
        let paths: Vec<String> = if repo {
            git(
                &root,
                &["ls-files", "--cached", "--others", "--exclude-standard"],
                20,
            )?
            .lines()
            .map(String::from)
            .collect()
        } else {
            let mut found = vec![];
            let mut stack = vec![PathBuf::new()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(root.join(&dir)) else {
                    continue;
                };
                for e in rd.filter_map(|e| e.ok()) {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if hidden(&name) {
                        continue;
                    }
                    let rel = dir.join(&name);
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push(rel);
                    } else {
                        found.push(rel.display().to_string());
                    }
                    if found.len() > 20_000 {
                        break;
                    }
                }
            }
            found
        };
        Ok(paths
            .into_iter()
            .filter(|p| p.to_lowercase().contains(&needle))
            .take(MAX_FOUND)
            .map(|path| Found {
                path,
                line: None,
                text: None,
            })
            .collect())
    }

    pub fn read_file(&self, id: &str, root: Option<&str>, path: &str) -> Result<FileText> {
        let root = self.project_root(id, root)?;
        let full = Self::inside(&root, path, true)?;
        let meta = std::fs::metadata(&full)?;
        if meta.is_dir() {
            bail!("{path} is a folder");
        }
        let (text, binary) = if meta.len() > MAX_READ {
            (None, false)
        } else {
            let bytes = std::fs::read(&full)?;
            if looks_binary(&bytes) {
                (None, true)
            } else {
                (Some(String::from_utf8_lossy(&bytes).into_owned()), false)
            }
        };
        Ok(FileText {
            path: path.to_string(),
            text,
            binary,
            size: meta.len(),
            modified: modified_ms(&meta),
        })
    }

    /// Save a file. `modified`: the time the client read it; refused when the copy on disk is
    /// newer (an agent changed it meanwhile). Returns the file as saved.
    pub fn write_file(
        &self,
        id: &str,
        root: Option<&str>,
        path: &str,
        text: &str,
        modified: Option<u64>,
    ) -> Result<FileText> {
        if text.len() > MAX_WRITE {
            bail!("too large to save here ({} MB at most)", MAX_WRITE >> 20);
        }
        let folder = root;
        let root = self.project_root(id, folder)?;
        let full = Self::inside(&root, path, false)?;
        let old = std::fs::metadata(&full).ok();
        if let (Some(meta), Some(read_at)) = (&old, modified) {
            if modified_ms(meta) > read_at {
                bail!("{path} changed on disk since it was opened: reopen it first");
            }
        }
        if old.as_ref().is_some_and(|m| m.is_dir()) {
            bail!("{path} is a folder");
        }
        // Next to it, then renamed over it, keeping its permissions.
        let tmp = full.with_file_name(format!(
            ".{}.kimo-save",
            full.file_name().unwrap_or_default().to_string_lossy()
        ));
        std::fs::write(&tmp, text).with_context(|| format!("writing {path}"))?;
        if let Some(meta) = &old {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, &full).with_context(|| format!("writing {path}"))?;
        self.read_file(id, folder, path)
    }

    pub fn git_status(&self, id: &str, root: Option<&str>) -> Result<GitState> {
        let root = self.project_root(id, root)?;
        if !root.join(".git").exists() {
            return Ok(GitState {
                repo: false,
                status: Status::default(),
            });
        }
        let out = git(&root, &["status", "--porcelain=v2", "--branch", "-z"], 20)?;
        Ok(GitState {
            repo: true,
            status: parse_status(&out),
        })
    }

    /// The changes of a file (staged ones with `staged`); a new file shows whole.
    pub fn git_diff(
        &self,
        id: &str,
        root: Option<&str>,
        path: &str,
        staged: bool,
    ) -> Result<String> {
        let root = self.project_root(id, root)?;
        relative(path).ok_or_else(|| anyhow!("{path:?} is outside the project"))?;
        let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
        if staged {
            args.push("--cached");
        }
        args.extend(["--", path]);
        let mut diff = git(&root, &args, 20)?;
        if diff.is_empty() && !staged {
            // Untracked: compared with nothing (git exits 1 when they differ).
            let out = run_for(
                Command::new("git")
                    .args(["diff", "--no-color", "--no-index", "--", "/dev/null", path])
                    .current_dir(&root),
                Duration::from_secs(20),
            )?;
            diff = String::from_utf8_lossy(&out.stdout).into_owned();
        }
        const MAX: usize = 1 << 20;
        if diff.len() > MAX {
            let cut = (0..=MAX)
                .rev()
                .find(|&i| diff.is_char_boundary(i))
                .unwrap_or(0);
            diff.truncate(cut);
            diff.push_str("\n… (diff cut: too long to show)\n");
        }
        Ok(diff)
    }

    pub fn git_stage(
        &self,
        id: &str,
        root: Option<&str>,
        paths: &[String],
        stage: bool,
    ) -> Result<()> {
        let root = self.project_root(id, root)?;
        for p in paths {
            relative(p).ok_or_else(|| anyhow!("{p:?} is outside the project"))?;
        }
        let mut args: Vec<&str> = if stage {
            vec!["add", "-A", "--"]
        } else {
            vec!["restore", "--staged", "--"]
        };
        if paths.is_empty() {
            args.push(".");
        } else {
            args.extend(paths.iter().map(String::as_str));
        }
        git(&root, &args, 60).map(|_| ())
    }

    /// Commit what is staged; returns the new commit.
    pub fn git_commit(&self, id: &str, root: Option<&str>, message: &str) -> Result<Commit> {
        let root = self.project_root(id, root)?;
        if message.trim().is_empty() {
            bail!("write a commit message");
        }
        git(&root, &["commit", "-m", message], 60)?;
        self.git_log(id, Some(&root.to_string_lossy()), 1)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("the commit was not found"))
    }

    /// Push the branch (setting its upstream the first time).
    pub fn git_push(&self, id: &str, root: Option<&str>) -> Result<String> {
        let root = self.project_root(id, root)?;
        let state = parse_status(&git(
            &root,
            &["status", "--porcelain=v2", "--branch", "-z"],
            20,
        )?);
        let branch = state.branch.ok_or_else(|| anyhow!("not on a branch"))?;
        let out = if state.upstream.is_some() {
            git(&root, &["push"], 120)?
        } else {
            git(&root, &["push", "-u", "origin", &branch], 120)?
        };
        Ok(out)
    }

    /// Open a pull request for the branch with GitHub's `gh`; returns its address.
    pub fn git_pull_request(
        &self,
        id: &str,
        root: Option<&str>,
        title: &str,
        body: &str,
    ) -> Result<String> {
        let root = self.project_root(id, root)?;
        if title.trim().is_empty() {
            bail!("give the pull request a title");
        }
        let out = run_for(
            Command::new("gh")
                .args(["pr", "create", "--title", title, "--body", body])
                .current_dir(&root)
                .env("GH_PROMPT_DISABLED", "1"),
            Duration::from_secs(60),
        )
        .map_err(|_| anyhow!("GitHub's gh command is needed for pull requests (brew install gh, then gh auth login)"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!(
                "{}",
                err.lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("gh pr create failed")
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .rev()
            .find(|l| l.starts_with("http"))
            .unwrap_or_default()
            .to_string())
    }

    pub fn git_log(&self, id: &str, root: Option<&str>, limit: usize) -> Result<Vec<Commit>> {
        let root = self.project_root(id, root)?;
        if !root.join(".git").exists() {
            return Ok(vec![]);
        }
        let n = format!("-{}", limit.clamp(1, 200));
        // An empty repository has no commits yet: no list rather than an error.
        let Ok(out) = git(&root, &["log", &n, "--format=%h%x1f%s%x1f%an%x1f%ct"], 20) else {
            return Ok(vec![]);
        };
        Ok(out
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\x1f').collect();
                Some(Commit {
                    hash: f.first()?.to_string(),
                    subject: f.get(1)?.to_string(),
                    author: f.get(2)?.to_string(),
                    at: f.get(3)?.parse().ok()?,
                })
            })
            .collect())
    }
}
