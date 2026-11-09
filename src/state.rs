//! The queue state on `uplink/state`, read and written as git objects.
//!
//! `uplink/state` is the only copy. Nothing of it is checked out: a read
//! is a blob of the branch, and a command's changes are held in memory, per
//! repository, until [`commit`] writes them as a new `uplink/state` commit
//! with a compare-and-swap on the ref. Changes a command never commits go
//! when it ends.
//!
//! With no changes held, reads follow `uplink/state` as it moves, so a
//! long-lived process never sees an old queue. With changes held, a moved
//! `uplink/state` is refused instead of overwriting either side.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::queue::patch_path;
use crate::types::{QUEUE_PATH, STATE_BRANCH};

/// Directory every state path starts with on the branch.
pub const STATE_DIR: &str = ".uplink";

/// What a command changed, on top of the `uplink/state` commit it read.
struct Overlay {
    base: Option<String>,
    /// Path to new contents; `None` removes the path.
    edits: BTreeMap<String, Option<Vec<u8>>>,
}

/// One repository's state in this process, behind its own lock, so work on
/// one repository never waits on git running for another.
#[derive(Default)]
struct Repo {
    overlay: Option<Overlay>,
    /// Blobs by commit and path; `None` when the commit has no such path.
    blobs: HashMap<(String, String), Option<Vec<u8>>>,
}

#[derive(Default)]
struct Registry {
    /// Repository path to its git common dir.
    common: HashMap<PathBuf, PathBuf>,
    /// By git common dir.
    repos: HashMap<PathBuf, Arc<Mutex<Repo>>>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn registry() -> MutexGuard<'static, Option<Registry>> {
    REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The state of the repository `repo` is in. The registry is locked only to
/// look it up, never while git runs.
fn repo_state(repo: &Path) -> Result<Arc<Mutex<Repo>>> {
    let known = registry()
        .get_or_insert_with(Registry::default)
        .common
        .get(repo)
        .cloned();
    let key = match known {
        Some(key) => key,
        None => common_dir(repo)?,
    };
    let mut guard = registry();
    let reg = guard.get_or_insert_with(Registry::default);
    reg.common.insert(repo.to_path_buf(), key.clone());
    Ok(reg.repos.entry(key).or_default().clone())
}

fn with_repo<T>(repo: &Path, f: impl FnOnce(&mut Repo) -> Result<T>) -> Result<T> {
    let state = repo_state(repo)?;
    let mut guard = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut guard)
}

fn git_dir_abs(repo: &Path, which: &str) -> Result<PathBuf> {
    let raw = git_ok(repo, &["rev-parse", "--path-format=absolute", which])?;
    Ok(PathBuf::from(raw))
}

fn common_dir(repo: &Path) -> Result<PathBuf> {
    git_dir_abs(repo, "--git-common-dir")
}

/// Private temp space under the git directory, created `0700`.
pub fn temp_root(repo: &Path) -> Result<PathBuf> {
    let dir = common_dir(repo)?.join("uplink/tmp");
    create_private_dir(&dir)?;
    Ok(dir)
}

/// Where a command leaves files for whoever runs it next, such as the body
/// of a pull request: `$RUNNER_TEMP/uplink/<name>` on a GitHub runner,
/// `<git-common-dir>/uplink/out/<name>` elsewhere. Absolute, created `0700`,
/// and never committed.
pub fn out_dir(repo: &Path, name: &str) -> Result<PathBuf> {
    crate::queue::require_path_component(name)?;
    let root = match std::env::var_os("RUNNER_TEMP").filter(|v| !v.is_empty()) {
        Some(tmp) => PathBuf::from(tmp).join("uplink"),
        None => common_dir(repo)?.join("uplink/out"),
    };
    let dir = root.join(name);
    create_private_dir(&dir)?;
    Ok(dir)
}

/// Whether `rel` names a state path (under `.uplink/`).
pub fn is_state_path(rel: &Path) -> bool {
    rel.is_relative() && rel.starts_with(STATE_DIR)
}

fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn branch_sha(repo: &Path) -> Result<Option<String>> {
    let result = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{STATE_BRANCH}^{{commit}}"),
        ],
        GitOpts::allow_fail(),
    )?;
    Ok((result.code == 0 && !result.stdout.is_empty()).then_some(result.stdout))
}

/// `git` with its output as bytes, untouched.
fn git_bytes(repo: &Path, args: &[&str]) -> Result<Option<Vec<u8>>> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()?;
    Ok(output.status.success().then_some(output.stdout))
}

fn blob_at(st: &mut Repo, repo: &Path, sha: &str, rel: &str) -> Result<Option<Vec<u8>>> {
    let cache = (sha.to_string(), rel.to_string());
    if let Some(found) = st.blobs.get(&cache) {
        return Ok(found.clone());
    }
    let found = git_bytes(repo, &["cat-file", "blob", &format!("{sha}:{rel}")])?;
    st.blobs.insert(cache, found.clone());
    Ok(found)
}

fn moved_error() -> Error {
    Error::msg(format!(
        "{STATE_BRANCH} moved while this command had changes to it that are not committed; \
run the command again"
    ))
}

/// The commit reads go to: the one changes were made on, or the live tip.
fn read_base(st: &Repo, repo: &Path) -> Result<Option<String>> {
    let live = branch_sha(repo)?;
    match &st.overlay {
        Some(overlay) if overlay.base != live => Err(moved_error()),
        Some(overlay) => Ok(overlay.base.clone()),
        None => Ok(live),
    }
}

fn rel_of(rel: impl AsRef<Path>) -> String {
    rel.as_ref().to_string_lossy().replace('\\', "/")
}

/// The contents of state path `rel` (such as `.uplink/queue.json`), with
/// this command's changes; `None` when there is no such file.
pub fn read(repo: &Path, rel: impl AsRef<Path>) -> Result<Option<Vec<u8>>> {
    let rel = rel_of(rel);
    with_repo(repo, |st| {
        if let Some(edit) = st.overlay.as_ref().and_then(|o| o.edits.get(&rel)) {
            let edit = edit.clone();
            // Still refuse a moved branch: the rest of the read is stale.
            read_base(st, repo)?;
            return Ok(edit);
        }
        match read_base(st, repo)? {
            Some(base) => blob_at(st, repo, &base, &rel),
            None => Ok(None),
        }
    })
}

/// [`read`] as text.
pub fn read_string(repo: &Path, rel: impl AsRef<Path>) -> Result<Option<String>> {
    Ok(read(repo, rel)?.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

/// The patch file of `id`.
pub fn patch_text(repo: &Path, id: &str) -> Result<String> {
    let rel = patch_path(id)?;
    read_string(repo, &rel)?.ok_or_else(|| {
        Error::msg(format!(
            "{} is not on {STATE_BRANCH}",
            rel.to_string_lossy()
        ))
    })
}

pub fn exists(repo: &Path, rel: impl AsRef<Path>) -> Result<bool> {
    Ok(read(repo, rel)?.is_some())
}

/// The files under state directory `dir`, with this command's changes.
pub fn list(repo: &Path, dir: impl AsRef<Path>) -> Result<Vec<String>> {
    let dir = rel_of(dir);
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    with_repo(repo, |st| {
        let mut files: BTreeSet<String> = BTreeSet::new();
        if let Some(base) = read_base(st, repo)? {
            let listed = git(
                repo,
                &["ls-tree", "-r", "--name-only", &base, "--", &prefix],
                GitOpts::allow_fail(),
            )?;
            files.extend(
                listed
                    .stdout
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string),
            );
        }
        if let Some(overlay) = &st.overlay {
            for (path, edit) in &overlay.edits {
                if path.starts_with(&prefix) {
                    match edit {
                        Some(_) => files.insert(path.clone()),
                        None => files.remove(path),
                    };
                }
            }
        }
        Ok(files.into_iter().collect())
    })
}

fn edit(repo: &Path, rel: String, contents: Option<Vec<u8>>) -> Result<()> {
    with_repo(repo, |st| {
        let live = branch_sha(repo)?;
        let overlay = st.overlay.get_or_insert_with(|| Overlay {
            base: live.clone(),
            edits: BTreeMap::new(),
        });
        if overlay.base != live {
            return Err(moved_error());
        }
        overlay.edits.insert(rel, contents);
        Ok(())
    })
}

/// Sets state path `rel` to `contents`, for the command's next [`commit`].
pub fn write(repo: &Path, rel: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
    edit(repo, rel_of(rel), Some(contents.as_ref().to_vec()))
}

/// Removes state path `rel`, for the command's next [`commit`].
pub fn remove(repo: &Path, rel: impl AsRef<Path>) -> Result<()> {
    edit(repo, rel_of(rel), None)
}

/// Drops this command's change to state path `rel`, if any: the path reads
/// as it is on `uplink/state` again.
pub fn forget(repo: &Path, rel: impl AsRef<Path>) -> Result<()> {
    let rel = rel_of(rel);
    with_repo(repo, |st| {
        if let Some(overlay) = st.overlay.as_mut() {
            overlay.edits.remove(&rel);
            if overlay.edits.is_empty() {
                st.overlay = None;
            }
        }
        Ok(())
    })
}

/// Removes every file under state directory `dir`.
pub fn remove_dir(repo: &Path, dir: impl AsRef<Path>) -> Result<()> {
    for file in list(repo, dir)? {
        remove(repo, file)?;
    }
    Ok(())
}

/// Whether `uplink/state` holds a queue, or this command has written one.
pub fn has_queue(repo: &Path) -> Result<bool> {
    if branch_sha(repo)?.is_some() {
        return Ok(true);
    }
    with_repo(repo, |st| {
        Ok(st
            .overlay
            .as_ref()
            .is_some_and(|o| matches!(o.edits.get(QUEUE_PATH), Some(Some(_)))))
    })
}

/// Drops this command's changes.
pub fn discard(repo: &Path) -> Result<()> {
    with_repo(repo, |st| {
        st.overlay = None;
        Ok(())
    })
}

/// Moves `uplink/state` to `sha`, dropping this command's changes.
pub fn reset_to(repo: &Path, sha: &str) -> Result<()> {
    git(
        repo,
        &["update-ref", &format!("refs/heads/{STATE_BRANCH}"), sha],
        GitOpts::default(),
    )?;
    discard(repo)
}

/// State paths this command changed and has not committed.
pub fn uncommitted_paths(repo: &Path) -> Result<Vec<String>> {
    with_repo(repo, |st| {
        Ok(st
            .overlay
            .as_ref()
            .map(|o| o.edits.keys().cloned().collect())
            .unwrap_or_default())
    })
}

/// Every change still held, by repository (its git common dir). A command
/// that ends with any lost them; `main` reports it.
pub fn pending() -> Vec<(PathBuf, Vec<String>)> {
    let repos: Vec<(PathBuf, Arc<Mutex<Repo>>)> = match registry().as_ref() {
        Some(reg) => reg
            .repos
            .iter()
            .map(|(key, state)| (key.clone(), state.clone()))
            .collect(),
        None => return Vec::new(),
    };
    let mut held = Vec::new();
    for (key, state) in repos {
        let st = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(overlay) = st.overlay.as_ref().filter(|o| !o.edits.is_empty()) {
            held.push((key, overlay.edits.keys().cloned().collect()));
        }
    }
    held.sort();
    held
}

fn temp_index(repo: &Path) -> Result<PathBuf> {
    Ok(temp_root(repo)?.join(format!("index-{}", Uuid::new_v4())))
}

fn tree_at(repo: &Path, sha: &str) -> Result<String> {
    git_ok(repo, &["rev-parse", &format!("{sha}^{{tree}}")])
}

/// The tree of `base` with `edits` applied, built in a throwaway index.
fn tree_with(
    repo: &Path,
    base: Option<&str>,
    edits: &BTreeMap<String, Option<Vec<u8>>>,
) -> Result<String> {
    let index = temp_index(repo)?;
    let opts = GitOpts {
        extra_env: vec![(
            "GIT_INDEX_FILE".into(),
            index.to_string_lossy().into_owned(),
        )],
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<String> {
        match base {
            Some(base) => git(repo, &["read-tree", base], opts.clone())?,
            None => git(repo, &["read-tree", "--empty"], opts.clone())?,
        };
        let mut info = String::new();
        for (path, contents) in edits {
            match contents {
                Some(bytes) => {
                    let sha = git(
                        repo,
                        &["hash-object", "-w", "--stdin"],
                        GitOpts {
                            input: Some(bytes),
                            ..GitOpts::default()
                        },
                    )?
                    .stdout;
                    info.push_str(&format!("100644 {sha}\t{path}\n"));
                }
                // Mode 0 removes the path from the index.
                None => info.push_str(&format!("0 {}\t{path}\n", "0".repeat(40))),
            }
        }
        if !info.is_empty() {
            git(
                repo,
                &["update-index", "--index-info"],
                GitOpts {
                    input: Some(info.as_bytes()),
                    ..opts.clone()
                },
            )?;
        }
        Ok(git(repo, &["write-tree"], opts.clone())?.stdout)
    })();
    let _ = fs::remove_file(&index);
    outcome
}

/// Commits this command's changes to `uplink/state` as `message`. Does
/// nothing when there are none. Refused while a queue run holds
/// `uplink/state`, unless this is that run, and when `uplink/state` moved
/// since the changes were made.
pub fn commit(repo: &Path, message: &str) -> Result<()> {
    crate::lock::refuse_while_running(repo)?;
    with_repo(repo, |st| {
        let Some(overlay) = &st.overlay else {
            return Ok(());
        };
        let live = branch_sha(repo)?;
        if overlay.base != live {
            return Err(moved_error());
        }
        let base = overlay.base.clone();
        let tree = tree_with(repo, base.as_deref(), &overlay.edits)?;
        if let Some(base) = base.as_deref()
            && tree == tree_at(repo, base)?
        {
            st.overlay = None;
            return Ok(());
        }
        let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
        if let Some(parent) = base.as_deref() {
            args.extend_from_slice(&["-p", parent]);
        }
        let commit = git(repo, &args, GitOpts::default())?.stdout;
        let refname = format!("refs/heads/{STATE_BRANCH}");
        let line = match base.as_deref() {
            Some(old) => format!("update {refname} {commit} {old}\n"),
            None => format!("create {refname} {commit}\n"),
        };
        let swapped = git(
            repo,
            &["update-ref", "--stdin"],
            GitOpts {
                allow_fail: true,
                input: Some(line.as_bytes()),
                ..GitOpts::default()
            },
        )?;
        if swapped.code != 0 {
            return Err(Error::msg(format!(
                "{STATE_BRANCH} moved while this command was changing it; run the command again"
            )));
        }
        st.overlay = None;
        Ok(())
    })
}

/// Patch texts for a replay: read from `uplink/state` (with this command's
/// changes) on first use, and replaced in memory as the replay refreshes
/// them. [`PatchSet::save`] writes the replaced ones back.
#[derive(Default)]
pub struct PatchSet {
    texts: HashMap<String, String>,
    changed: BTreeSet<String>,
}

impl PatchSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// The text of patch `id`.
    pub fn get(&mut self, repo: &Path, id: &str) -> Result<String> {
        if let Some(text) = self.texts.get(id) {
            return Ok(text.clone());
        }
        let text = patch_text(repo, id)?;
        self.texts.insert(id.to_string(), text.clone());
        Ok(text)
    }

    /// Replaces the text of patch `id`.
    pub fn set(&mut self, id: &str, text: String) {
        self.texts.insert(id.to_string(), text);
        self.changed.insert(id.to_string());
    }

    /// Writes the replaced patches as changes of this command.
    pub fn save(&self, repo: &Path) -> Result<()> {
        for id in &self.changed {
            write(repo, patch_path(id)?, &self.texts[id])?;
        }
        Ok(())
    }
}

/// The tree a work tree's `.uplink/` would commit, leaving out `*.tmp`.
fn mirror_tree(repo: &Path, top: &Path) -> Result<String> {
    let index = temp_index(repo)?;
    let opts = GitOpts {
        extra_env: vec![
            (
                "GIT_DIR".into(),
                git_dir_abs(repo, "--git-dir")?
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("GIT_WORK_TREE".into(), top.to_string_lossy().into_owned()),
            (
                "GIT_INDEX_FILE".into(),
                index.to_string_lossy().into_owned(),
            ),
        ],
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<String> {
        git(top, &["read-tree", "--empty"], opts.clone())?;
        git(
            top,
            &[
                "add",
                "--force",
                "--all",
                "--",
                STATE_DIR,
                ":(exclude).uplink/*.tmp",
            ],
            opts.clone(),
        )?;
        Ok(git(top, &["write-tree"], opts.clone())?.stdout)
    })();
    let _ = fs::remove_file(&index);
    outcome
}

/// Checks a `.uplink/` directory left in the work tree by an earlier
/// binary. One that matches `uplink/state`, apart from forge output files
/// a command writes again, is removed with its line in `info/exclude`; one
/// with other files that never reached `uplink/state` is refused, naming
/// them.
pub fn retire_worktree_mirror(repo: &Path) -> Result<()> {
    let Ok(top) = git_ok(repo, &["rev-parse", "--show-toplevel"]) else {
        return Ok(());
    };
    let top = PathBuf::from(top);
    let mirror = top.join(STATE_DIR);
    if !mirror.is_dir() {
        return Ok(());
    }
    // A product repo may track a `.uplink/` of its own; leave that alone.
    let tracked = git_ok(&top, &["ls-files", "--", STATE_DIR]).unwrap_or_default();
    if !tracked.is_empty() {
        return Ok(());
    }
    let differing = match branch_sha(repo)? {
        Some(sha) => {
            let tree = mirror_tree(repo, &top)?;
            let base = tree_at(repo, &sha)?;
            if tree == base {
                Vec::new()
            } else {
                // Files only on the branch are not edits: an older binary
                // restored the mirror lazily.
                git_ok(
                    repo,
                    &["diff", "--name-only", "--diff-filter=AM", &base, &tree],
                )?
                .lines()
                .filter(|line| !line.is_empty() && !is_old_output(line))
                .map(str::to_string)
                .collect()
            }
        }
        None => vec![STATE_DIR.to_string()],
    };
    if !differing.is_empty() {
        return Err(Error::msg(format!(
            "{} in the work tree has files that are not on {STATE_BRANCH}:\n  {}\n\
git uplink keeps the queue on {STATE_BRANCH} only and no longer reads `.uplink/`. \
Copy out anything you want to keep, then delete `.uplink/` to continue; \
nothing on {STATE_BRANCH} is lost.",
            mirror.display(),
            differing.join("\n  ")
        )));
    }
    fs::remove_dir_all(&mirror)?;
    drop_exclude_line(repo)?;
    Ok(())
}

/// Files an older binary wrote into `.uplink/reports/<id>/` for the forge
/// to read and did not always commit: pull request bodies, their comments
/// and the contrib commit message. Each command writes them again.
fn is_old_output(path: &str) -> bool {
    const OUTPUTS: &[&str] = &[
        "pr.md",
        "commit-message.txt",
        "conflict.md",
        "conflict-comment.md",
        "transfer.md",
        "transfer-comment.md",
        "amend.md",
        "amend-comment.md",
    ];
    let Some(rest) = path.strip_prefix(".uplink/reports/") else {
        return false;
    };
    matches!(rest.split('/').collect::<Vec<_>>()[..], [_, name] if OUTPUTS.contains(&name))
}

fn drop_exclude_line(repo: &Path) -> Result<()> {
    let path = PathBuf::from(git_ok(
        repo,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    )?);
    let Ok(current) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let kept: Vec<&str> = current
        .lines()
        .filter(|line| !matches!(line.trim(), ".uplink/" | ".uplink"))
        .collect();
    if kept.len() == current.lines().count() {
        return Ok(());
    }
    let mut body = kept.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    fs::write(path, body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git_ok(dir, &["init", "--quiet", "-b", "main"]).unwrap();
        git_ok(dir, &["config", "user.email", "t@example.com"]).unwrap();
        git_ok(dir, &["config", "user.name", "T"]).unwrap();
        git_ok(dir, &["commit", "--quiet", "--allow-empty", "-m", "root"]).unwrap();
        tmp
    }

    fn on_branch(repo: &Path, path: &str) -> Option<String> {
        crate::repo::show_at(repo, STATE_BRANCH, path).ok()
    }

    fn text(repo: &Path, rel: &str) -> Option<String> {
        read_string(repo, rel).unwrap()
    }

    #[test]
    fn commit_creates_the_branch_and_touches_no_files() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{}\n").unwrap();
        assert_eq!(text(r, QUEUE_PATH).as_deref(), Some("{}\n"));
        commit(r, "init").unwrap();
        assert_eq!(on_branch(r, QUEUE_PATH).as_deref(), Some("{}"));
        assert!(!r.join(STATE_DIR).exists());
        assert!(!r.join(".git/uplink/stage").exists());
        assert!(uncommitted_paths(r).unwrap().is_empty());
    }

    #[test]
    fn reads_keep_the_bytes_exactly() {
        let tmp = repo();
        let r = tmp.path();
        write(r, ".uplink/patches/x.patch", "a\n\n").unwrap();
        commit(r, "init").unwrap();
        assert_eq!(text(r, ".uplink/patches/x.patch").as_deref(), Some("a\n\n"));
    }

    #[test]
    fn unchanged_contents_commit_nothing() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{}\n").unwrap();
        commit(r, "init").unwrap();
        let before = branch_sha(r).unwrap();
        write(r, QUEUE_PATH, "{}\n").unwrap();
        commit(r, "again").unwrap();
        assert_eq!(branch_sha(r).unwrap(), before);
    }

    #[test]
    fn remove_and_list() {
        let tmp = repo();
        let r = tmp.path();
        write(r, ".uplink/reports/a/one.md", "1").unwrap();
        write(r, ".uplink/reports/a/two.md", "2").unwrap();
        commit(r, "init").unwrap();
        remove(r, ".uplink/reports/a/one.md").unwrap();
        write(r, ".uplink/reports/a/three.md", "3").unwrap();
        assert_eq!(
            list(r, ".uplink/reports/a").unwrap(),
            [".uplink/reports/a/three.md", ".uplink/reports/a/two.md"]
        );
        commit(r, "edit").unwrap();
        assert!(on_branch(r, ".uplink/reports/a/one.md").is_none());
        assert_eq!(
            on_branch(r, ".uplink/reports/a/three.md").as_deref(),
            Some("3")
        );
    }

    #[test]
    fn reads_follow_the_branch_when_nothing_is_changed() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{\"a\":1}\n").unwrap();
        commit(r, "one").unwrap();
        let one = branch_sha(r).unwrap().unwrap();
        write(r, QUEUE_PATH, "{\"a\":2}\n").unwrap();
        commit(r, "two").unwrap();
        git_ok(r, &["update-ref", "refs/heads/uplink/state", &one]).unwrap();
        assert_eq!(text(r, QUEUE_PATH).as_deref(), Some("{\"a\":1}\n"));
    }

    #[test]
    fn changes_refuse_a_moved_branch() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{\"a\":1}\n").unwrap();
        commit(r, "one").unwrap();
        let one = branch_sha(r).unwrap().unwrap();
        write(r, QUEUE_PATH, "{\"a\":2}\n").unwrap();
        commit(r, "two").unwrap();
        write(r, QUEUE_PATH, "{\"a\":3}\n").unwrap();
        git_ok(r, &["update-ref", "refs/heads/uplink/state", &one]).unwrap();
        assert!(read(r, QUEUE_PATH).is_err());
        assert!(commit(r, "three").is_err());
        assert_eq!(branch_sha(r).unwrap().as_deref(), Some(one.as_str()));
    }

    #[test]
    fn discard_drops_changes() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{}\n").unwrap();
        commit(r, "init").unwrap();
        write(r, ".uplink/patches/x.patch", "x\n").unwrap();
        assert_eq!(uncommitted_paths(r).unwrap(), [".uplink/patches/x.patch"]);
        discard(r).unwrap();
        assert!(uncommitted_paths(r).unwrap().is_empty());
        assert!(!exists(r, ".uplink/patches/x.patch").unwrap());
    }

    #[test]
    fn a_patch_set_saves_only_what_it_replaced() {
        let tmp = repo();
        let r = tmp.path();
        write(r, ".uplink/patches/upl_a.patch", "a\n").unwrap();
        write(r, ".uplink/patches/upl_b.patch", "b\n").unwrap();
        commit(r, "init").unwrap();
        let mut set = PatchSet::new();
        assert_eq!(set.get(r, "upl_a").unwrap(), "a\n");
        set.set("upl_b", "b2\n".into());
        assert_eq!(set.get(r, "upl_b").unwrap(), "b2\n");
        // Nothing is written until it is saved.
        assert_eq!(
            text(r, ".uplink/patches/upl_b.patch").as_deref(),
            Some("b\n")
        );
        set.save(r).unwrap();
        assert_eq!(
            uncommitted_paths(r).unwrap(),
            [".uplink/patches/upl_b.patch"]
        );
    }

    #[test]
    fn matching_mirror_is_retired_and_a_differing_one_refused() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{}\n").unwrap();
        commit(r, "init").unwrap();
        let exclude = r.join(".git/info/exclude");
        fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        fs::write(&exclude, "*.log\n.uplink/\n").unwrap();
        fs::create_dir_all(r.join(".uplink")).unwrap();
        fs::write(r.join(QUEUE_PATH), "{}\n").unwrap();
        retire_worktree_mirror(r).unwrap();
        assert!(!r.join(".uplink").exists());
        assert_eq!(fs::read_to_string(&exclude).unwrap(), "*.log\n");

        fs::create_dir_all(r.join(".uplink")).unwrap();
        fs::write(r.join(QUEUE_PATH), "{\"edited\":true}\n").unwrap();
        let err = retire_worktree_mirror(r).unwrap_err().to_string();
        assert!(err.contains(".uplink/queue.json"), "{err}");
        assert!(r.join(QUEUE_PATH).exists());
    }

    #[test]
    fn a_mirror_with_only_old_outputs_is_retired() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{}\n").unwrap();
        commit(r, "init").unwrap();
        fs::create_dir_all(r.join(".uplink/reports/upl_x")).unwrap();
        fs::write(r.join(QUEUE_PATH), "{}\n").unwrap();
        fs::write(r.join(".uplink/reports/upl_x/pr.md"), "body\n").unwrap();
        fs::write(r.join(".uplink/reports/upl_x/conflict-comment.md"), "c\n").unwrap();
        retire_worktree_mirror(r).unwrap();
        assert!(!r.join(".uplink").exists());
    }

    #[test]
    fn old_outputs_are_only_the_forge_files() {
        assert!(is_old_output(".uplink/reports/upl_x/pr.md"));
        assert!(is_old_output(".uplink/reports/upl_x/amend-comment.md"));
        assert!(!is_old_output(".uplink/reports/upl_x/assessment.md"));
        assert!(!is_old_output(".uplink/reports/upl_x/extras/pr.md"));
        assert!(!is_old_output(".uplink/queue.json"));
    }

    #[test]
    fn pending_lists_changes_until_they_are_committed() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{}\n").unwrap();
        let key = common_dir(r).unwrap();
        assert!(
            pending()
                .iter()
                .any(|(k, files)| *k == key && files == &[QUEUE_PATH])
        );
        commit(r, "init").unwrap();
        assert!(!pending().iter().any(|(k, _)| *k == key));
    }

    #[test]
    fn linked_worktrees_share_the_branch() {
        let tmp = repo();
        let r = tmp.path();
        write(r, QUEUE_PATH, "{\"a\":1}\n").unwrap();
        commit(r, "one").unwrap();
        let wt = tmp.path().join("wt");
        git_ok(
            r,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                wt.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(text(&wt, QUEUE_PATH).as_deref(), Some("{\"a\":1}\n"));
        assert!(!wt.join(STATE_DIR).exists());
    }
}
