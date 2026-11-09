use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::assess::{company_commit_message, export_commit_message};
use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::queue::{now_iso, patch_path};
use crate::types::{ApplyOutcome, Patch, QUEUE_PATH, QueueConfig, QueueState, STATE_BRANCH};

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

/// The stable patch id of the patch file at `patch_file`, a state path such
/// as `.uplink/patches/<id>.patch`.
pub fn stable_patch_id(repo: &Path, patch_file: &str) -> Result<String> {
    let contents = crate::state::read_string(repo, patch_file)?
        .ok_or_else(|| Error::msg(format!("{patch_file} is not on {STATE_BRANCH}")))?;
    stable_patch_id_from_contents(repo, &contents)
}

/// A detached `git worktree add` checkout, removed when dropped. Work done in
/// it never touches the operator's own checkout.
pub(crate) struct TempWorktree<'a> {
    pub(crate) repo: &'a Path,
    pub(crate) dir: PathBuf,
}

impl<'a> TempWorktree<'a> {
    pub(crate) fn add(repo: &'a Path, prefix: &str, at: &str) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
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

/// [`format_product_patch`], set as patch `id` for the command's next
/// state commit. Returns its state path.
pub fn write_product_patch(
    repo: &Path,
    id: &str,
    from_ref: &str,
    message: &str,
    head_ref: &str,
) -> Result<PathBuf> {
    let file = patch_path(id)?;
    let body = format_product_patch(repo, id, from_ref, message, head_ref)?;
    crate::state::write(repo, &file, body)?;
    Ok(file)
}

/// The product change `from_ref..head_ref`, without `.uplink/`, as one
/// format-patch commit with `message`.
pub fn format_product_patch(
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

/// Applies patch `text` on the checkout and commits it. With
/// [`ApplyOutcome::Applied`] comes the patch as it applied here, when it
/// differs from `text` in more than its mbox line and date.
pub fn apply_patch_text(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    text: &str,
    export_message: bool,
) -> Result<(ApplyOutcome, Option<String>)> {
    if apply_succeeds(repo, &["--reverse", "--check"], text)? {
        return Ok((ApplyOutcome::Empty, None));
    }
    if !apply_succeeds(repo, &["--3way", "--index"], text)? {
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
    let refreshed = (patch_substance(text) != patch_substance(&body)).then_some(body);
    Ok((ApplyOutcome::Applied, refreshed))
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

/// Whether `git apply <flags>` succeeds in `dir` on patch `text`, given on stdin.
pub(crate) fn apply_succeeds(dir: &Path, flags: &[&str], text: &str) -> Result<bool> {
    let mut args = vec!["apply"];
    args.extend_from_slice(flags);
    let applied = git(
        dir,
        &args,
        GitOpts {
            allow_fail: true,
            input: Some(text.as_bytes()),
            ..GitOpts::default()
        },
    )?;
    Ok(applied.code == 0)
}

/// True when `git apply --reverse --check` succeeds against `git_ref`'s tree.
/// Uses a throwaway index so HEAD and the worktree stay put.
pub fn patch_already_applied_on(repo: &Path, git_ref: &str, text: &str) -> Result<bool> {
    let index = crate::state::temp_root(repo)?.join(format!("apply-check-{}", Uuid::new_v4()));
    let index_s = index.to_string_lossy().into_owned();
    let index_opts = GitOpts {
        extra_env: vec![("GIT_INDEX_FILE".into(), index_s)],
        allow_fail: true,
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<bool> {
        git(
            repo,
            &["read-tree", git_ref],
            GitOpts {
                extra_env: index_opts.extra_env.clone(),
                ..GitOpts::default()
            },
        )?;
        let reverse = git(
            repo,
            &["apply", "--cached", "--reverse", "--check"],
            GitOpts {
                input: Some(text.as_bytes()),
                ..index_opts
            },
        )?;
        Ok(reverse.code == 0)
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
        &["fetch", "--quiet", "--prune", remote, &spec],
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

fn state_ref_source(repo: &Path) -> Result<Option<String>> {
    let branch = STATE_BRANCH;
    if has_ref(repo, branch)? {
        return Ok(Some(branch.to_string()));
    }
    let origin = format!("{COMPANY_REMOTE}/{branch}");
    if has_ref(repo, &origin)? {
        return Ok(Some(origin));
    }
    Ok(None)
}

fn has_remote(repo: &Path, name: &str) -> bool {
    git_ok(repo, &["remote"])
        .unwrap_or_default()
        .split('\n')
        .any(|remote| remote == name)
}

pub fn ensure_remote(repo: &Path, name: &str, url: &str) -> Result<()> {
    if has_remote(repo, name) {
        git(repo, &["remote", "set-url", name, url], GitOpts::default())?;
    } else {
        git(repo, &["remote", "add", name, url], GitOpts::default())?;
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

/// Replace local `uplink/state` with origin's, dropping this command's edits.
pub fn replace_state_from_origin(repo: &Path) -> Result<()> {
    let sha = fetch_tracking_sha(repo, COMPANY_REMOTE, STATE_BRANCH)?;
    apply_state_sha(repo, STATE_BRANCH, &sha)?;
    if !path_exists_at(repo, STATE_BRANCH, QUEUE_PATH)? {
        return Err(not_initialized_error());
    }
    Ok(())
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
    apply_state_sha(repo, STATE_BRANCH, &sha)
}

pub fn state_exists(repo: &Path) -> Result<bool> {
    if has_ref(repo, STATE_BRANCH)? {
        return Ok(true);
    }
    if has_ref(repo, &format!("{COMPANY_REMOTE}/{STATE_BRANCH}"))? {
        return Ok(true);
    }
    crate::state::has_queue(repo)
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
    // C locale so the "couldn't find remote ref" message is not translated.
    let fetched = git(
        repo,
        &args,
        GitOpts {
            allow_fail: true,
            extra_env: vec![("LC_ALL".into(), "C".into())],
            ..GitOpts::default()
        },
    )?;
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
    let args = ["fetch", "--quiet", "--prune", remote, spec.as_str()];
    // C locale so the "couldn't find remote ref" message is not translated.
    let fetched = git(
        repo,
        &args,
        GitOpts {
            allow_fail: true,
            extra_env: vec![("LC_ALL".into(), "C".into())],
            ..GitOpts::default()
        },
    )?;
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

/// Creates local `uplink/state` from origin's when only that exists.
pub fn ensure_state_branch(repo: &Path) -> Result<()> {
    let Some(source) = state_ref_source(repo)? else {
        return Ok(());
    };
    let branch = STATE_BRANCH;
    if source != branch {
        git(repo, &["branch", "-f", branch, &source], GitOpts::default())?;
    }
    Ok(())
}

/// Commits this command's changes to uplink/state. Refused while a queue run
/// (adopt, restack) holds uplink/state, unless this is that run.
pub fn commit_queue(repo: &Path, message: &str) -> Result<()> {
    crate::state::commit(repo, message)
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
    if !git_succeeds(repo, &["fetch", "--quiet", "--prune", remote, &spec])? {
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

pub fn apply_state_sha(repo: &Path, branch: &str, sha: &str) -> Result<()> {
    set_state_branch(repo, branch, sha)
}

/// Fetch `uplink/state` into a remote-tracking ref without moving the local branch.
pub fn fetch_state_tracking(repo: &Path, remote: &str, branch: &str) -> Result<Option<String>> {
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    if !git_succeeds(repo, &["fetch", "--quiet", "--prune", remote, &spec])? {
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
    git(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), sha],
        GitOpts::default(),
    )?;
    if branch == STATE_BRANCH {
        crate::state::discard(repo)?;
    }
    Ok(())
}

/// Sets state `paths` as they are at `source`, removing the ones `source`
/// does not have, for the command's next state commit.
pub fn restore_paths_from(repo: &Path, source: &str, paths: &[String]) -> Result<()> {
    for path in paths {
        if path_exists_at(repo, source, path)? {
            crate::state::write(repo, path, git_raw_blob(repo, source, path)?)?;
        } else {
            crate::state::remove(repo, path)?;
        }
    }
    Ok(())
}

fn git_raw_blob(repo: &Path, source: &str, path: &str) -> Result<Vec<u8>> {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(["cat-file", "blob", &format!("{source}:{path}")])
        .output()?;
    if !out.status.success() {
        return Err(Error::msg(format!("could not read {source}:{path}")));
    }
    Ok(out.stdout)
}

pub fn path_exists_at(repo: &Path, sha: &str, path: &str) -> Result<bool> {
    has_object(repo, &format!("{sha}:{path}"))
}

pub fn push_state_branch(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let dest = format!("refs/heads/{branch}:refs/heads/{branch}");
    git(repo, &["push", remote, &dest], GitOpts::default())?;
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
    let branch = STATE_BRANCH;
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

/// Force-push a local branch (preview or company main).
/// The published commit is a queue replay, so this does not lease the remote tip
/// and does not fetch the branch first.
pub fn push_branch_force(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let dest = format!("refs/heads/{branch}:refs/heads/{branch}");
    git(
        repo,
        &["push", "--quiet", "--force", remote, &dest],
        GitOpts::default(),
    )?;
    Ok(())
}

pub fn new_patch_id() -> String {
    let raw = Uuid::new_v4().simple().to_string();
    format!("upl_{}", &raw[..10])
}

pub fn stamp() -> String {
    now_iso()
}

#[cfg(test)]
mod tests {
    use super::{
        UPSTREAM_REF, ensure_upstream_ref, file_history, has_ref, patch_substance, show_at,
    };
    use crate::git::git_ok;

    #[test]
    fn ensure_upstream_ref_tolerates_a_missing_branch_but_not_a_broken_origin() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("origin.git");
        let repo = dir.path().join("work");
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(
            dir.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        )
        .unwrap();
        git_ok(&repo, &["init", "-q"]).unwrap();
        git_ok(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]).unwrap();
        git_ok(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();

        ensure_upstream_ref(&repo).unwrap();
        assert!(!has_ref(&repo, UPSTREAM_REF).unwrap());

        let missing = dir.path().join("missing.git");
        git_ok(
            &repo,
            &["remote", "set-url", "origin", missing.to_str().unwrap()],
        )
        .unwrap();
        assert!(ensure_upstream_ref(&repo).is_err());
    }

    #[test]
    fn option_like_refs_are_not_parsed_as_git_options() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).unwrap();
        std::fs::create_dir_all(repo.join(".uplink")).unwrap();
        std::fs::write(repo.join(".uplink/queue.json"), "{}\n").unwrap();
        git_ok(repo, &["add", "."]).unwrap();
        git_ok(repo, &["commit", "-q", "-m", "init"]).unwrap();
        // Without --end-of-options, git show would write the output here.
        std::fs::create_dir_all(repo.join("out:.uplink")).unwrap();

        assert!(show_at(repo, "--output=out", ".uplink/queue.json").is_err());
        assert!(!repo.join("out:.uplink/queue.json").exists());
        assert!(
            file_history(repo, "--output=log.txt", ".uplink/queue.json")
                .unwrap()
                .is_empty()
        );
        assert!(!repo.join("log.txt").exists());
        assert_eq!(show_at(repo, "HEAD", ".uplink/queue.json").unwrap(), "{}");
    }

    const SHA_A: &str = "d075809c4498552bd4e080010af34999967d68f7";
    const SHA_B: &str = "5529448d17e1f2eb9c08b4396ed7b22b55ccb83d";

    fn sample(sha: &str, date: &str, subject: &str, body_date: &str, hunk: &str) -> String {
        format!(
            "From {sha} Mon Sep 17 00:00:00 2001\n\
From: Uplink Bot <uplink@company.example>\n\
Date: {date}\n\
Subject: [PATCH] {subject}\n\
\n\
{subject}\n\
{body_date}\n\
\n\
---\n\
 notes.md | 1 +\n\
 1 file changed, 1 insertion(+)\n\
\n\
diff --git a/notes.md b/notes.md\n\
--- a/notes.md\n\
+++ b/notes.md\n\
@@ -0,0 +1 @@\n\
+{hunk}\n"
        )
    }

    #[test]
    fn sha_and_date_header_do_not_change_patch_substance() {
        let stored = sample(
            SHA_A,
            "Tue, 22 Sep 2026 22:52:44 +0200",
            "Uplink tooling",
            "Date: kept in the message",
            "tooling",
        );
        let replayed = sample(
            SHA_B,
            "Tue, 22 Sep 2026 21:20:41 +0000",
            "Uplink tooling",
            "Date: kept in the message",
            "tooling",
        );
        assert_eq!(patch_substance(&stored), patch_substance(&replayed));
    }

    #[test]
    fn hunk_change_is_a_different_patch() {
        let stored = sample(SHA_A, "Tue, 22 Sep 2026 22:52:44 +0200", "Note", "", "one");
        let updated = sample(SHA_B, "Tue, 22 Sep 2026 21:20:41 +0000", "Note", "", "two");
        assert_ne!(patch_substance(&stored), patch_substance(&updated));
    }

    #[test]
    fn subject_change_is_a_different_patch() {
        let stored = sample(SHA_A, "Tue, 22 Sep 2026 22:52:44 +0200", "Note", "", "one");
        let updated = sample(SHA_B, "Tue, 22 Sep 2026 21:20:41 +0000", "Other", "", "one");
        assert_ne!(patch_substance(&stored), patch_substance(&updated));
    }

    #[test]
    fn date_line_in_the_message_body_is_a_change() {
        let stored = sample(
            SHA_A,
            "Tue, 22 Sep 2026 22:52:44 +0200",
            "Note",
            "Date: kept in the message",
            "one",
        );
        let updated = sample(
            SHA_A,
            "Tue, 22 Sep 2026 22:52:44 +0200",
            "Note",
            "Date: rewritten in the message",
            "one",
        );
        assert_ne!(patch_substance(&stored), patch_substance(&updated));
    }
}
