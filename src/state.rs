//! Queue state read from and written to `uplink/state` objects. Nothing is
//! written to the work tree: changes are staged in memory and committed
//! through a temporary index.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_bytes, git_ok};
use crate::queue::{parse_queue, patch_path};
use crate::repo::{has_ref, random_hex, rev_parse};
use crate::types::{
    PREVIOUS_MAIN_PATH, PreviousMain, QUEUE_PATH, QueueState, STATE_BRANCH, STATE_REF,
};

/// A commit of `uplink/state` (or none yet) with changes staged on top.
#[derive(Debug, Clone)]
pub struct StateTree {
    repo: PathBuf,
    base: Option<String>,
    /// Path to new contents, or `None` for a removed file.
    changes: BTreeMap<String, Option<Vec<u8>>>,
}

impl StateTree {
    /// The state at `base`, any commit. `None` starts from an empty tree.
    pub fn at(repo: &Path, base: Option<&str>) -> Result<Self> {
        let base = match base {
            Some(base) => Some(rev_parse(repo, &format!("{base}^{{commit}}"))?),
            None => None,
        };
        Ok(Self {
            repo: repo.to_path_buf(),
            base,
            changes: BTreeMap::new(),
        })
    }

    /// Local `uplink/state`, created from origin's when it is missing. Empty
    /// when neither exists.
    pub fn head(repo: &Path) -> Result<Self> {
        crate::repo::ensure_state_branch(repo)?;
        if has_ref(repo, STATE_REF)? {
            Self::at(repo, Some(STATE_REF))
        } else {
            Self::at(repo, None)
        }
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// The commit the changes are staged on.
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// Whether anything is staged.
    pub fn is_dirty(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Paths staged since `base`.
    pub fn changed_paths(&self) -> Vec<String> {
        self.changes.keys().cloned().collect()
    }

    pub fn read_bytes(&self, path: &str) -> Result<Option<Vec<u8>>> {
        if let Some(change) = self.changes.get(path) {
            return Ok(change.clone());
        }
        let Some(base) = &self.base else {
            return Ok(None);
        };
        read_blob_at(&self.repo, base, path)
    }

    pub fn read(&self, path: &str) -> Result<Option<String>> {
        Ok(self
            .read_bytes(path)?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    pub fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.read_bytes(path)?.is_some())
    }

    /// Files under `prefix`, staged changes included, sorted.
    pub fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let dir = format!("{}/", prefix.trim_end_matches('/'));
        let mut paths: Vec<String> = match &self.base {
            Some(base) => git_ok(
                &self.repo,
                &[
                    "ls-tree",
                    "-r",
                    "--name-only",
                    "--full-tree",
                    base,
                    "--",
                    &dir,
                ],
            )?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
            None => Vec::new(),
        };
        for (path, change) in &self.changes {
            if !path.starts_with(&dir) {
                continue;
            }
            match change {
                Some(_) if !paths.contains(path) => paths.push(path.clone()),
                None => paths.retain(|p| p != path),
                Some(_) => {}
            }
        }
        paths.sort();
        Ok(paths)
    }

    pub fn write(&mut self, path: &str, contents: impl Into<Vec<u8>>) {
        self.changes.insert(path.to_string(), Some(contents.into()));
    }

    pub fn remove(&mut self, path: &str) {
        self.changes.insert(path.to_string(), None);
    }

    /// Removes every file under `prefix`.
    pub fn remove_dir(&mut self, prefix: &str) -> Result<()> {
        for path in self.list(prefix)? {
            self.remove(&path);
        }
        Ok(())
    }

    /// Stages `paths` as they are at `source`, removing those it lacks.
    pub fn copy_from(&mut self, source: &str, paths: &[String]) -> Result<()> {
        for path in paths {
            match read_blob_at(&self.repo, source, path)? {
                Some(bytes) => self.write(path, bytes),
                None => self.remove(path),
            }
        }
        Ok(())
    }

    /// The queue, with settings from `uplink/hooks`.
    pub fn queue(&self) -> Result<QueueState> {
        let Some(raw) = self.read(QUEUE_PATH)? else {
            return Err(Error::msg(format!(
                "{STATE_BRANCH} has no {QUEUE_PATH}; this clone is not initialized. \
Run `git uplink init --upstream <url> --contrib <url>` to create a queue."
            )));
        };
        let mut queue = parse_queue(&raw)?;
        queue.settings = crate::settings::load_settings(&self.repo, queue.config.forge);
        Ok(queue)
    }

    pub fn has_queue(&self) -> Result<bool> {
        self.exists(QUEUE_PATH)
    }

    pub fn write_queue(&mut self, queue: &QueueState) -> Result<()> {
        let body = format!("{}\n", serde_json::to_string_pretty(queue)?);
        self.write(QUEUE_PATH, body);
        Ok(())
    }

    pub fn write_previous_main(&mut self, previous: &PreviousMain) -> Result<()> {
        let body = format!("{}\n", serde_json::to_string_pretty(previous)?);
        self.write(PREVIOUS_MAIN_PATH, body);
        Ok(())
    }

    /// The stored patch file of `id`, or `None`.
    pub fn patch(&self, id: &str) -> Result<Option<String>> {
        self.read(&patch_key(id)?)
    }

    /// The stored patch file of `id`; an error when it is missing.
    pub fn require_patch(&self, id: &str) -> Result<String> {
        self.patch(id)?.ok_or_else(|| {
            Error::msg(format!(
                "patch {id} has no {} on {STATE_BRANCH}",
                patch_key(id).unwrap_or_default()
            ))
        })
    }

    pub fn write_patch(&mut self, id: &str, body: impl Into<Vec<u8>>) -> Result<()> {
        self.write(&patch_key(id)?, body);
        Ok(())
    }

    pub fn remove_patch(&mut self, id: &str) -> Result<()> {
        self.remove(&patch_key(id)?);
        Ok(())
    }

    /// The tree of `base` with the staged changes.
    pub fn tree(&self) -> Result<String> {
        if self.changes.is_empty()
            && let Some(base) = &self.base
        {
            return git_ok(&self.repo, &["rev-parse", &format!("{base}^{{tree}}")]);
        }
        let index = temp_index_path(&self.repo)?;
        let index_opts = GitOpts {
            extra_env: vec![(
                "GIT_INDEX_FILE".into(),
                index.to_string_lossy().into_owned(),
            )],
            ..GitOpts::default()
        };
        let outcome = (|| -> Result<String> {
            match &self.base {
                Some(base) => git(&self.repo, &["read-tree", base], index_opts.clone())?,
                None => git(&self.repo, &["read-tree", "--empty"], index_opts.clone())?,
            };
            for (path, change) in &self.changes {
                match change {
                    Some(contents) => {
                        let blob = git(
                            &self.repo,
                            &["hash-object", "-w", "--stdin"],
                            GitOpts {
                                input: Some(contents),
                                ..GitOpts::default()
                            },
                        )?
                        .stdout;
                        git(
                            &self.repo,
                            &[
                                "update-index",
                                "--add",
                                "--cacheinfo",
                                &format!("100644,{blob},{path}"),
                            ],
                            index_opts.clone(),
                        )?;
                    }
                    None => {
                        git(
                            &self.repo,
                            &["update-index", "--force-remove", "--", path],
                            index_opts.clone(),
                        )?;
                    }
                }
            }
            Ok(git(&self.repo, &["write-tree"], index_opts.clone())?.stdout)
        })();
        let _ = fs::remove_file(&index);
        outcome
    }

    /// Commits the staged changes on `base` without moving any ref. `None`
    /// when the tree did not change.
    pub fn commit(&self, message: &str) -> Result<Option<String>> {
        let tree = self.tree()?;
        if let Some(base) = &self.base {
            let old = git_ok(&self.repo, &["rev-parse", &format!("{base}^{{tree}}")])?;
            if old == tree {
                return Ok(None);
            }
        }
        let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
        if let Some(base) = &self.base {
            args.extend_from_slice(&["-p", base]);
        }
        Ok(Some(git(&self.repo, &args, GitOpts::default())?.stdout))
    }

    /// Commits the staged changes and moves local `uplink/state` to the new
    /// commit, only while it is still at `base`. Afterwards the tree is
    /// staged on that commit with nothing pending. Returns whether a commit
    /// was made. Refused while a queue run holds uplink/state, unless this
    /// is that run.
    pub fn commit_to_branch(&mut self, message: &str) -> Result<bool> {
        crate::lock::refuse_while_running(&self.repo)?;
        let Some(commit) = self.commit(message)? else {
            self.changes.clear();
            return Ok(false);
        };
        let old = self.base.clone().unwrap_or_default();
        git(
            &self.repo,
            &["update-ref", STATE_REF, &commit, &old],
            GitOpts::default(),
        )?;
        self.base = Some(commit);
        self.changes.clear();
        Ok(true)
    }
}

/// Writes `queue` on local `uplink/state` as one commit, when it changed.
pub fn commit_queue(repo: &Path, queue: &QueueState, message: &str) -> Result<()> {
    let mut state = StateTree::head(repo)?;
    state.write_queue(queue)?;
    state.commit_to_branch(message)?;
    Ok(())
}

/// `patches/<id>.patch` as a tree path.
fn patch_key(id: &str) -> Result<String> {
    Ok(patch_path(id)?.to_string_lossy().into_owned())
}

/// A temporary index file in the git dir, which also works in a linked
/// worktree, where `.git` is a file.
pub(crate) fn temp_index_path(repo: &Path) -> Result<PathBuf> {
    let dir = git_ok(repo, &["rev-parse", "--absolute-git-dir"])?;
    Ok(PathBuf::from(dir).join(format!("uplink-index-{}", random_hex(32))))
}

/// The contents of the regular file `path` at `commit`, or `None` when it
/// is missing there. Anything but a regular file is refused: anyone who
/// can push may have written `uplink/state`.
pub fn read_blob_at(repo: &Path, commit: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let entry = git_ok(repo, &["ls-tree", "-z", "--full-tree", commit, "--", path])?;
    let Some((meta, _)) = entry.split('\0').next().and_then(|e| e.split_once('\t')) else {
        return Ok(None);
    };
    let mut fields = meta.split(' ');
    let (mode, kind, blob) = (fields.next(), fields.next(), fields.next());
    match (mode, kind, blob) {
        (Some("100644" | "100755"), Some("blob"), Some(blob)) => {
            Ok(Some(git_bytes(repo, &["cat-file", "blob", blob])?))
        }
        _ => Err(Error::msg(format!(
            "{commit} has {path} as something other than a regular file. \
git uplink only reads regular files on {STATE_BRANCH}; replace it with a new commit."
        ))),
    }
}

#[cfg(test)]
#[path = "state_test.rs"]
mod tests;
