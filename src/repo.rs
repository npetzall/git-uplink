use std::fs;
use std::path::{Path, PathBuf};

use crate::assess::{company_commit_message, export_commit_message};
use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::queue::{now_iso, patch_path};
use crate::types::{
    ApplyOutcome, ORIGIN_STATE_REF, Patch, QUEUE_PATH, QueueConfig, QueueState, STATE_BRANCH,
    STATE_REF,
};

pub(crate) const UPSTREAM_REF: &str = "uplink/upstream";
pub(crate) const COMPANY_REMOTE: &str = "origin";

pub fn rev_parse(repo: &Path, git_ref: &str) -> Result<String> {
    git_ok(repo, &["rev-parse", git_ref])
}

pub fn has_ref(repo: &Path, git_ref: &str) -> Result<bool> {
    let result = git(
        repo,
        &["rev-parse", "--verify", "--quiet", git_ref],
        GitOpts::allow_fail(),
    )?;
    Ok(result.code == 0 && !result.stdout.is_empty())
}

fn has_object(repo: &Path, git_ref: &str) -> Result<bool> {
    git_succeeds(repo, &["cat-file", "-e", git_ref])
}

/// Resolve git refs to SHAs, fetching any missing objects from origin.
pub fn ensure_revs(repo: &Path, refs: &[&str]) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for git_ref in refs {
        if !has_object(repo, git_ref)? {
            missing.push(*git_ref);
        }
    }
    let fetch_err = if missing.is_empty() {
        None
    } else if !has_remote(repo, COMPANY_REMOTE) {
        return Err(Error::msg(format!(
            "revision '{}' is not in this clone and origin is not configured to fetch it",
            missing.join("', '")
        )));
    } else {
        let mut args = vec!["fetch", "--quiet", COMPANY_REMOTE];
        args.extend(missing.iter().copied());
        let fetched = git(repo, &args, GitOpts::allow_fail())?;
        if fetched.code != 0 {
            Some(if fetched.stderr.is_empty() {
                fetched.stdout
            } else {
                fetched.stderr
            })
        } else {
            None
        }
    };

    let mut shas = Vec::with_capacity(refs.len());
    for git_ref in refs {
        if !has_object(repo, git_ref)? {
            let mut msg = format!("revision '{git_ref}' is not in this clone");
            if let Some(detail) = &fetch_err {
                msg.push_str("; git fetch origin failed: ");
                msg.push_str(detail);
            } else if !missing.is_empty() {
                msg.push_str("; git fetch origin did not materialize it");
            }
            return Err(Error::msg(msg));
        }
        shas.push(rev_parse(repo, git_ref)?);
    }
    Ok(shas)
}

pub fn stable_patch_id_from_contents(repo: &Path, contents: &str) -> Result<String> {
    let result = git(
        repo,
        &["patch-id", "--stable"],
        GitOpts {
            input: Some(contents.as_bytes()),
            ..GitOpts::default()
        },
    )?;
    Ok(result
        .stdout
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string())
}

/// A detached `git worktree add` checkout, removed when dropped. Work done in
/// it never touches the operator's own checkout.
pub(crate) struct TempWorktree<'a> {
    pub(crate) repo: &'a Path,
    pub(crate) dir: PathBuf,
}

impl<'a> TempWorktree<'a> {
    pub(crate) fn add(repo: &'a Path, prefix: &str, at: &str) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", random_hex(32)));
        git(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                "--quiet",
                dir.to_str().unwrap_or(""),
                at,
            ],
            GitOpts::default(),
        )?;
        Ok(Self { repo, dir })
    }
}

impl Drop for TempWorktree<'_> {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.to_str() {
            let _ = git(
                self.repo,
                &["worktree", "remove", "--force", dir],
                GitOpts::allow_fail(),
            );
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The product change `from_ref..head_ref` as a patch file with `message`.
pub fn product_patch(
    repo: &Path,
    id: &str,
    from_ref: &str,
    message: &str,
    head_ref: &str,
) -> Result<String> {
    let diff = git_ok(
        repo,
        &[
            "diff",
            "--full-index",
            from_ref,
            head_ref,
            "--",
            ".",
            ":!.uplink",
        ],
    )?;
    if diff.trim().is_empty() {
        return Err(Error::msg("No product changes to import as a patch."));
    }

    let worktree = TempWorktree::add(repo, "uplink-isolate", from_ref)?;
    let payload = format!("{diff}\n");
    let apply = git(
        &worktree.dir,
        &["apply", "--3way", "--index"],
        GitOpts {
            allow_fail: true,
            input: Some(payload.as_bytes()),
            ..GitOpts::default()
        },
    )?;
    if apply.code != 0 {
        return Err(Error::msg(format!(
            "Could not isolate patch {id}: {}",
            if apply.stderr.is_empty() {
                apply.stdout
            } else {
                apply.stderr
            }
        )));
    }
    git(
        &worktree.dir,
        &["commit", "-m", message],
        GitOpts::default(),
    )?;
    let formatted = git_ok(
        &worktree.dir,
        &["format-patch", "--full-index", "-1", "--stdout"],
    )?;
    drop(worktree);
    Ok(if formatted.ends_with('\n') {
        formatted
    } else {
        format!("{formatted}\n")
    })
}

/// Whether `git apply <args>` with the patch text on stdin succeeds.
fn apply_succeeds(repo: &Path, args: &[&str], patch: &str, opts: GitOpts<'_>) -> Result<bool> {
    let mut cmd = vec!["apply"];
    cmd.extend_from_slice(args);
    let result = git(
        repo,
        &cmd,
        GitOpts {
            allow_fail: true,
            input: Some(patch.as_bytes()),
            ..opts
        },
    )?;
    Ok(result.code == 0)
}

/// Applies the stored patch file `stored` in `repo` and commits it. On
/// [`ApplyOutcome::Applied`] it also returns the patch as replayed when that
/// differs from `stored` in more than its `From` and `Date` lines, for the
/// caller to store.
pub fn apply_patch_file(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    stored: &str,
    export_message: bool,
) -> Result<(ApplyOutcome, Option<String>)> {
    if apply_succeeds(repo, &["--reverse", "--check"], stored, GitOpts::default())? {
        return Ok((ApplyOutcome::Empty, None));
    }

    if !apply_succeeds(repo, &["--3way", "--index"], stored, GitOpts::default())? {
        return Ok((ApplyOutcome::Conflict, None));
    }

    if git_succeeds(repo, &["diff", "--cached", "--quiet"])? {
        return Ok((ApplyOutcome::Empty, None));
    }

    let message = if export_message {
        export_commit_message(patch)
    } else {
        company_commit_message(queue, patch)
    };
    git(repo, &["commit", "-m", &message], GitOpts::default())?;
    let formatted = git_ok(repo, &["format-patch", "--full-index", "-1", "--stdout"])?;
    let body = if formatted.ends_with('\n') {
        formatted
    } else {
        format!("{formatted}\n")
    };
    let replayed = (patch_substance(stored) != patch_substance(&body)).then_some(body);
    Ok((ApplyOutcome::Applied, replayed))
}

/// Patch text with the mbox `From <sha>` line and the `Date:` header removed.
/// Those two fields change on every replay even when the patch itself does not.
pub(crate) fn patch_substance(text: &str) -> String {
    let mut lines = text.lines();
    let mut out = String::new();
    if let Some(first) = lines.next()
        && !is_mbox_from_line(first)
    {
        out.push_str(first);
        out.push('\n');
    }
    let mut in_headers = true;
    for line in lines {
        if in_headers {
            if line.is_empty() {
                in_headers = false;
            } else if line.starts_with("Date:") {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn is_mbox_from_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("From ") else {
        return false;
    };
    let Some(sha) = rest.split_whitespace().next() else {
        return false;
    };
    sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())
}

/// True when `git apply --reverse --check` succeeds against `git_ref`'s tree.
/// Uses a throwaway index so HEAD and the worktree stay put.
pub fn patch_already_applied_on(repo: &Path, git_ref: &str, patch: &str) -> Result<bool> {
    let index = crate::state::temp_index_path(repo)?;
    let index_opts = GitOpts {
        extra_env: vec![(
            "GIT_INDEX_FILE".into(),
            index.to_string_lossy().into_owned(),
        )],
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<bool> {
        git(repo, &["read-tree", git_ref], index_opts.clone())?;
        apply_succeeds(
            repo,
            &["--cached", "--reverse", "--check"],
            patch,
            index_opts,
        )
    })();
    let _ = fs::remove_file(&index);
    outcome
}

pub fn conflicted_files(repo: &Path) -> Result<Vec<String>> {
    let output = git_ok(repo, &["diff", "--name-only", "--diff-filter=U"])?;
    Ok(output
        .split('\n')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// Fetch public upstream into `refs/remotes/<remote>/<branch>` without moving
/// `uplink/upstream`. Sync classifies the range before promoting.
pub fn fetch_upstream_remote(repo: &Path, queue: &QueueState) -> Result<String> {
    let remote = &queue.config.upstream_remote;
    let branch = &queue.config.upstream_branch;
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    git(
        repo,
        &[
            "fetch",
            "--quiet",
            "--prune",
            "--end-of-options",
            remote,
            &spec,
        ],
        GitOpts::default(),
    )?;
    git_ok(repo, &["rev-parse", &format!("{remote}/{branch}")])
}

pub fn promote_upstream(repo: &Path, sha: &str) -> Result<()> {
    git(
        repo,
        &["branch", "-f", UPSTREAM_REF, sha],
        GitOpts::default(),
    )?;
    Ok(())
}

pub fn fetch_upstream(repo: &Path, queue: &QueueState) -> Result<String> {
    let sha = fetch_upstream_remote(repo, queue)?;
    promote_upstream(repo, &sha)?;
    Ok(sha)
}

fn has_remote(repo: &Path, name: &str) -> bool {
    git_ok(repo, &["remote"])
        .unwrap_or_default()
        .split('\n')
        .any(|remote| remote == name)
}

pub fn ensure_remote(repo: &Path, name: &str, url: &str) -> Result<()> {
    if has_remote(repo, name) {
        git(
            repo,
            &["remote", "set-url", "--end-of-options", name, url],
            GitOpts::default(),
        )?;
    } else {
        git(
            repo,
            &["remote", "add", "--end-of-options", name, url],
            GitOpts::default(),
        )?;
    }
    Ok(())
}

pub fn ensure_configured_remotes(repo: &Path, config: &QueueConfig) -> Result<()> {
    if let Some(url) = config.upstream_url.as_deref().filter(|u| !u.is_empty()) {
        ensure_remote(repo, &config.upstream_remote, url)?;
    }
    if let Some(url) = config.contrib_url.as_deref().filter(|u| !u.is_empty()) {
        ensure_remote(repo, &config.contrib_remote, url)?;
    }
    Ok(())
}

fn not_initialized_error() -> Error {
    Error::msg(
        "Could not fetch origin uplink/state; this clone is not initialized. \
Run `git uplink init --upstream <url> --contrib <url>` to create a queue.",
    )
}

/// Replace local `uplink/state` with origin's.
pub fn replace_state_from_origin(repo: &Path) -> Result<()> {
    let sha = fetch_tracking_sha(repo, COMPANY_REMOTE, STATE_BRANCH)?;
    if !path_exists_at(repo, &sha, QUEUE_PATH)? {
        return Err(not_initialized_error());
    }
    set_state_branch(repo, STATE_BRANCH, &sha)
}

/// Replace local `uplink/state` from origin when that remote exists. Missing
/// origin or a missing state branch is not an error (first-time create still
/// needs to run).
pub fn try_replace_state_from_origin(repo: &Path) -> Result<()> {
    if !has_remote(repo, COMPANY_REMOTE) {
        return Ok(());
    }
    let Ok(sha) = fetch_tracking_sha(repo, COMPANY_REMOTE, STATE_BRANCH) else {
        return Ok(());
    };
    set_state_branch(repo, STATE_BRANCH, &sha)
}

pub fn state_exists(repo: &Path) -> Result<bool> {
    Ok(has_ref(repo, STATE_REF)? || has_ref(repo, ORIGIN_STATE_REF)?)
}

/// Creates local `uplink/state` from origin's when it is missing. Nothing
/// happens when neither exists.
pub fn ensure_state_branch(repo: &Path) -> Result<()> {
    if has_ref(repo, STATE_REF)? {
        return Ok(());
    }
    if has_ref(repo, ORIGIN_STATE_REF)? {
        git(
            repo,
            &["branch", "-f", STATE_BRANCH, ORIGIN_STATE_REF],
            GitOpts::default(),
        )?;
    }
    Ok(())
}

/// Materialize local `uplink/upstream` from the company repo (`origin`), never from
/// the public `upstream` remote. Missing origin is not an error; callers still
/// check `has_ref`.
pub fn ensure_upstream_ref(repo: &Path) -> Result<()> {
    if has_ref(repo, UPSTREAM_REF)? {
        return Ok(());
    }
    let tracking = format!("{COMPANY_REMOTE}/{UPSTREAM_REF}");
    if has_ref(repo, &tracking)? {
        git(
            repo,
            &["branch", "-f", UPSTREAM_REF, &tracking],
            GitOpts::default(),
        )?;
        return Ok(());
    }
    if !has_remote(repo, COMPANY_REMOTE) {
        return Ok(());
    }
    let spec = format!("+refs/heads/{UPSTREAM_REF}:refs/heads/{UPSTREAM_REF}");
    let args = ["fetch", "--quiet", COMPANY_REMOTE, spec.as_str()];
    let fetched = git(repo, &args, GitOpts::allow_fail())?;
    // An origin without uplink/upstream is fine; any other failure is real.
    if fetched.code != 0 && !fetched.stderr.contains("couldn't find remote ref") {
        return Err(Error::Git(crate::git::GitError::new(&args, fetched)));
    }
    Ok(())
}

/// Fetch `uplink/upstream` from the company remote and update the local branch.
/// Does not talk to the public `upstream` remote (`fetch_upstream` is sync).
pub fn refresh_upstream_ref(repo: &Path, remote: &str) -> Result<()> {
    if !has_remote(repo, remote) {
        return Ok(());
    }
    let spec = format!("+refs/heads/{UPSTREAM_REF}:refs/remotes/{remote}/{UPSTREAM_REF}");
    let args = [
        "fetch",
        "--quiet",
        "--prune",
        "--end-of-options",
        remote,
        spec.as_str(),
    ];
    let fetched = git(repo, &args, GitOpts::allow_fail())?;
    if fetched.code != 0 {
        // A remote without the branch is fine. Any other failure is not:
        // init would go on to seed uplink/upstream from public upstream,
        // and preflight would build on a base nobody accepted.
        if fetched.stderr.contains("couldn't find remote ref") {
            return Ok(());
        }
        return Err(Error::Git(crate::git::GitError::new(&args, fetched)));
    }
    let sha = git_ok(repo, &["rev-parse", &format!("{remote}/{UPSTREAM_REF}")])?;
    git(
        repo,
        &["update-ref", &format!("refs/heads/{UPSTREAM_REF}"), &sha],
        GitOpts::default(),
    )?;
    Ok(())
}

/// Commits the tree of `base` with `files` (path, contents) written into it,
/// on top of `base`; without `base`, a root commit of only `files`. Uses a
/// temporary index: the working tree and every local branch
/// stay as they are.
pub fn commit_files_on(
    repo: &Path,
    base: Option<&str>,
    files: &[(&str, &str)],
    message: &str,
) -> Result<String> {
    let mut state = crate::state::StateTree::at(repo, base)?;
    for (path, contents) in files {
        state.write(path, *contents);
    }
    let tree = state.tree()?;
    let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
    if let Some(base) = state.base() {
        args.extend_from_slice(&["-p", base]);
    }
    Ok(git(repo, &args, GitOpts::default())?.stdout)
}

/// Pushes `commit` to `{branch}` on `remote` as a fast-forward. Never forced.
pub fn push_commit_to(repo: &Path, remote: &str, commit: &str, branch: &str) -> Result<()> {
    let dest = format!("{commit}:refs/heads/{branch}");
    git(
        repo,
        &["push", "--end-of-options", remote, &dest],
        GitOpts::default(),
    )?;
    Ok(())
}

pub fn ahead_behind(repo: &Path, local: &str, remote: &str) -> Result<(u32, u32)> {
    let ahead = git_ok(
        repo,
        &["rev-list", "--count", &format!("{remote}..{local}")],
    )?;
    let behind = git_ok(
        repo,
        &["rev-list", "--count", &format!("{local}..{remote}")],
    )?;
    Ok((parse_count(&ahead), parse_count(&behind)))
}

fn parse_count(raw: &str) -> u32 {
    raw.trim().parse().unwrap_or(0)
}

/// Fetch `branch` into a remote-tracking ref. Does not move a checked-out local branch.
pub fn fetch_tracking_sha(repo: &Path, remote: &str, branch: &str) -> Result<String> {
    if !has_remote(repo, remote) {
        return Err(Error::msg(format!(
            "{remote} remote is missing; cannot fetch {branch}. \
Run `git uplink init --upstream <url> --contrib <url>` to create a queue."
        )));
    }
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    if !git_succeeds(
        repo,
        &[
            "fetch",
            "--quiet",
            "--prune",
            "--end-of-options",
            remote,
            &spec,
        ],
    )? {
        if branch == STATE_BRANCH {
            return Err(not_initialized_error());
        }
        return Err(Error::msg(format!("Could not fetch {remote} {branch}.")));
    }
    git_ok(repo, &["rev-parse", &format!("{remote}/{branch}")])
}

pub fn point_branch_at(repo: &Path, branch: &str, sha: &str) -> Result<()> {
    git(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), sha],
        GitOpts::default(),
    )?;
    Ok(())
}

/// Fetch `uplink/state` into a remote-tracking ref without moving the local branch.
pub fn fetch_state_tracking(repo: &Path, remote: &str, branch: &str) -> Result<Option<String>> {
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    if !git_succeeds(
        repo,
        &[
            "fetch",
            "--quiet",
            "--prune",
            "--end-of-options",
            remote,
            &spec,
        ],
    )? {
        return Ok(None);
    }
    Ok(Some(git_ok(
        repo,
        &["rev-parse", &format!("{remote}/{branch}")],
    )?))
}

pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    git_succeeds(repo, &["merge-base", "--is-ancestor", ancestor, descendant])
}

pub fn merge_base(repo: &Path, a: &str, b: &str) -> Result<Option<String>> {
    let result = git(repo, &["merge-base", a, b], GitOpts::allow_fail())?;
    if result.code != 0 || result.stdout.is_empty() {
        return Ok(None);
    }
    Ok(Some(result.stdout))
}

/// Commits `git log <args>` lists, in its order, each with the values of its
/// `Uplink-Patch-Id` trailers (none for a commit no rebuild wrote).
pub fn commits_with_patch_ids(repo: &Path, args: &[&str]) -> Result<Vec<(String, Vec<String>)>> {
    let mut log = vec![
        "log",
        "--format=%H %(trailers:key=Uplink-Patch-Id,valueonly,separator=%x2C)",
    ];
    log.extend_from_slice(args);
    Ok(git_ok(repo, &log)?
        .lines()
        .filter_map(|line| {
            let (sha, ids) = line.split_once(' ').unwrap_or((line, ""));
            let ids = ids
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect();
            (!sha.is_empty()).then(|| (sha.to_string(), ids))
        })
        .collect())
}

/// Commits `git log <args>` lists, in its order, each with whether it carries
/// an `Uplink-Patch-Id` trailer. Every commit a rebuild writes has one.
pub fn commits_with_patch_id(repo: &Path, args: &[&str]) -> Result<Vec<(String, bool)>> {
    let mut log = vec![
        "log",
        "--format=%H %(trailers:key=Uplink-Patch-Id,valueonly,separator=%x2C)",
    ];
    log.extend_from_slice(args);
    Ok(git_ok(repo, &log)?
        .lines()
        .filter_map(|line| {
            let (sha, ids) = line.split_once(' ').unwrap_or((line, ""));
            (!sha.is_empty()).then(|| (sha.to_string(), !ids.trim().is_empty()))
        })
        .collect())
}

pub fn queue_at(repo: &Path, sha: &str) -> Result<QueueState> {
    let raw = show_at(repo, sha, QUEUE_PATH)?;
    crate::queue::parse_queue(&raw)
}

pub fn set_state_branch(repo: &Path, branch: &str, sha: &str) -> Result<()> {
    if branch == STATE_BRANCH {
        crate::lock::refuse_while_running(repo)?;
    }
    point_branch_at(repo, branch, sha)
}

pub fn path_exists_at(repo: &Path, sha: &str, path: &str) -> Result<bool> {
    has_object(repo, &format!("{sha}:{path}"))
}

pub fn push_state_branch(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let dest = format!("refs/heads/{branch}:refs/heads/{branch}");
    git(
        repo,
        &["push", "--end-of-options", remote, &dest],
        GitOpts::default(),
    )?;
    Ok(())
}

/// Pushes local `branch` with a lease: only while `remote` still has it at
/// `expected`, or does not have it when `expected` is `None`. A caller that
/// checked local descends from `expected` gets a fast-forward or nothing.
pub fn push_branch_leased(
    repo: &Path,
    remote: &str,
    branch: &str,
    expected: Option<&str>,
) -> Result<()> {
    let lease = format!(
        "--force-with-lease=refs/heads/{branch}:{}",
        expected.unwrap_or_default()
    );
    let dest = format!("refs/heads/{branch}:refs/heads/{branch}");
    git(
        repo,
        &["push", &lease, "--end-of-options", remote, &dest],
        GitOpts::default(),
    )?;
    Ok(())
}

pub fn show_at(repo: &Path, sha: &str, path: &str) -> Result<String> {
    git_ok(
        repo,
        &["show", "--end-of-options", &format!("{sha}:{path}")],
    )
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRevision {
    pub sha: String,
    pub at: String,
    pub subject: String,
}

/// Commits on `git_ref` that touched `path`, newest first.
pub fn file_history(repo: &Path, git_ref: &str, path: &str) -> Result<Vec<FileRevision>> {
    let result = git(
        repo,
        &[
            "log",
            "--format=%H%x09%cI%x09%s",
            "--end-of-options",
            git_ref,
            "--",
            path,
        ],
        GitOpts::allow_fail(),
    )?;
    if result.code != 0 {
        return Ok(Vec::new());
    }
    Ok(result
        .stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some(FileRevision {
                sha: parts.next()?.to_string(),
                at: parts.next()?.to_string(),
                subject: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect())
}

pub fn patch_state_commit(repo: &Path, id: &str) -> Result<String> {
    let branch = STATE_REF;
    let path = patch_path(id)?.to_string_lossy().into_owned();
    let result = git(
        repo,
        &["log", "-1", "--format=%H", branch, "--", &path],
        GitOpts::allow_fail(),
    )?;
    if result.code == 0 && !result.stdout.is_empty() {
        return Ok(result.stdout);
    }
    rev_parse(repo, branch)
}

pub fn refresh_company_branch(repo: &Path, remote: &str, branch: &str) -> Result<String> {
    let sha = fetch_tracking_sha(repo, remote, branch)?;
    git(
        repo,
        &["checkout", "--quiet", "-f", "-B", branch, &sha],
        GitOpts::default(),
    )?;
    Ok(sha)
}

/// Fetch `branch` from `remote` and point the local branch at that tip.
/// Does not check the branch out. A checked-out branch is left as-is so the
/// worktree stays put; a missing remote branch fails the fetch.
pub fn ensure_company_branch_ref(repo: &Path, remote: &str, branch: &str) -> Result<String> {
    let sha = fetch_tracking_sha(repo, remote, branch)?;
    let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if head == branch {
        return Ok(sha);
    }
    git(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), &sha],
        GitOpts::default(),
    )?;
    Ok(sha)
}

pub fn new_patch_id() -> String {
    format!("upl_{}", random_hex(10))
}

/// `len` random lowercase hex digits from the OS generator, for ids and
/// scratch file names that must not collide.
pub fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len.div_ceil(2)];
    getrandom::fill(&mut bytes).expect("OS random number generator failed");
    let mut hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    hex.truncate(len);
    hex
}

pub fn stamp() -> String {
    now_iso()
}

/// Where files for the forge to read go (PR descriptions and comments, a
/// commit message): `<git-common-dir>/uplink/out/<id>/`, outside the work
/// tree.
pub fn forge_out_dir(repo: &Path, id: &str) -> Result<PathBuf> {
    let id = crate::queue::require_path_component(id)?;
    let common = PathBuf::from(git_ok(repo, &["rev-parse", "--git-common-dir"])?);
    let common = if common.is_absolute() {
        common
    } else {
        repo.join(common)
    };
    Ok(common.join("uplink/out").join(id))
}

/// The `.uplink/` folder earlier binaries kept in the checkout. State now
/// lives only in `uplink/state` objects.
const LEGACY_DIR: &str = ".uplink";

/// Removes the `.uplink/` folder an earlier binary left in the checkout, and
/// its line in `.git/info/exclude`, when everything in it is on
/// `uplink/state`. Refuses when it holds anything else, so no edit is lost.
///
/// Does nothing outside a checkout, without the folder, when the product
/// tracks files under it, or when no earlier binary was here: neither the
/// exclude line it wrote nor an `uplink/state` exists. Such a folder is not
/// uplink's to remove.
pub fn retire_legacy_folder(repo: &Path) -> Result<()> {
    let dir = repo.join(LEGACY_DIR);
    let Ok(meta) = fs::symlink_metadata(&dir) else {
        return Ok(());
    };
    let toplevel = git(
        repo,
        &["rev-parse", "--show-toplevel"],
        GitOpts::allow_fail(),
    )?;
    if toplevel.code != 0 || fs::canonicalize(&toplevel.stdout).ok() != fs::canonicalize(repo).ok()
    {
        return Ok(());
    }
    if !git_ok(repo, &["ls-files", "-z", "--", LEGACY_DIR])?.is_empty() {
        return Ok(());
    }
    if !has_legacy_exclude(repo)? && !state_exists(repo)? {
        return Ok(());
    }
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error::msg(format!(
            "{LEGACY_DIR} in this checkout is not a folder. git uplink keeps its state only on \
{STATE_BRANCH} now; remove {LEGACY_DIR} by hand."
        )));
    }
    let uncommitted = legacy_uncommitted_paths(repo)?;
    if !uncommitted.is_empty() {
        return Err(Error::msg(format!(
            "{LEGACY_DIR}/ in this checkout differs from {STATE_BRANCH}:\n  {}\n\
git uplink keeps its state only on {STATE_BRANCH} now and no longer uses this folder.\n\
To keep these changes, commit them on {STATE_BRANCH} from a checkout of it \
(git worktree add <dir> {STATE_BRANCH}) and push it.\n\
To discard them: rm -rf {LEGACY_DIR}",
            uncommitted.join("\n  ")
        )));
    }
    fs::remove_dir_all(&dir)?;
    drop_legacy_exclude(repo)
}

/// Paths under `.uplink/` in the checkout that differ from `uplink/state`
/// (local, else origin's), temporary files left out.
fn legacy_uncommitted_paths(repo: &Path) -> Result<Vec<String>> {
    let base = if has_ref(repo, STATE_REF)? {
        Some(STATE_REF)
    } else if has_ref(repo, ORIGIN_STATE_REF)? {
        Some(ORIGIN_STATE_REF)
    } else {
        None
    };
    let index = crate::state::temp_index_path(repo)?;
    let index_opts = GitOpts {
        extra_env: vec![(
            "GIT_INDEX_FILE".into(),
            index.to_string_lossy().into_owned(),
        )],
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<String> {
        match base {
            Some(base) => git(repo, &["read-tree", base], index_opts.clone())?,
            None => git(repo, &["read-tree", "--empty"], index_opts.clone())?,
        };
        git(
            repo,
            &[
                "add",
                "-f",
                "-A",
                "--",
                LEGACY_DIR,
                ":(exclude).uplink/*.tmp",
            ],
            index_opts.clone(),
        )?;
        let tree = git(repo, &["write-tree"], index_opts.clone())?.stdout;
        match base {
            Some(base) => git_ok(
                repo,
                &["diff", "--name-only", &format!("{base}^{{tree}}"), &tree],
            ),
            None => git_ok(repo, &["ls-tree", "-r", "--name-only", &tree]),
        }
    })();
    let _ = fs::remove_file(&index);
    Ok(outcome?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn legacy_exclude_path(repo: &Path) -> Result<PathBuf> {
    let common = git_ok(repo, &["rev-parse", "--git-common-dir"])?;
    Ok(repo.join(common).join("info/exclude"))
}

fn is_legacy_exclude_line(line: &str) -> bool {
    matches!(line.trim(), ".uplink/" | ".uplink")
}

/// Whether `.git/info/exclude` has the line an earlier binary wrote.
fn has_legacy_exclude(repo: &Path) -> Result<bool> {
    Ok(fs::read_to_string(legacy_exclude_path(repo)?)
        .is_ok_and(|current| current.lines().any(is_legacy_exclude_line)))
}

fn drop_legacy_exclude(repo: &Path) -> Result<()> {
    let path = legacy_exclude_path(repo)?;
    let Ok(current) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let kept: Vec<&str> = current
        .lines()
        .filter(|line| !is_legacy_exclude_line(line))
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
#[path = "repo_test.rs"]
mod tests;
