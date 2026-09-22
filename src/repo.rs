use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::assess::{company_commit_message, export_commit_message};
use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::queue::{now_iso, patch_path};
use crate::types::{PATCH_DIR, Patch, QUEUE_PATH, QueueConfig, QueueState, STATE_BRANCH};

pub(crate) const UPSTREAM_REF: &str = "uplink/upstream";
pub(crate) const COMPANY_REMOTE: &str = "origin";

pub fn ensure_uplink_dirs(repo: &Path) -> Result<()> {
    fs::create_dir_all(repo.join(PATCH_DIR))?;
    Ok(())
}

pub fn rev_parse(repo: &Path, git_ref: &str) -> Result<String> {
    git_ok(repo, &["rev-parse", git_ref])
}

pub fn has_ref(repo: &Path, git_ref: &str) -> Result<bool> {
    let result = git(
        repo,
        &["rev-parse", "--verify", "--quiet", git_ref],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    Ok(result.code == 0 && !result.stdout.is_empty())
}

fn has_object(repo: &Path, git_ref: &str) -> Result<bool> {
    let result = git(
        repo,
        &["cat-file", "-e", git_ref],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    Ok(result.code == 0)
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
        let fetched = git(
            repo,
            &args,
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        )?;
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

pub fn stable_patch_id(repo: &Path, patch_file: &str) -> Result<String> {
    let contents = fs::read_to_string(repo.join(patch_file))?;
    stable_patch_id_from_contents(repo, &contents)
}

pub fn write_product_patch(
    repo: &Path,
    id: &str,
    from_ref: &str,
    message: &str,
    head_ref: &str,
) -> Result<PathBuf> {
    ensure_uplink_dirs(repo)?;
    let file = patch_path(id)?;
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

    let original = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let original_sha = git_ok(repo, &["rev-parse", "HEAD"])?;
    let formatted = (|| -> Result<String> {
        git(
            repo,
            &["checkout", "--quiet", "--detach", from_ref],
            GitOpts::default(),
        )?;
        let payload = format!("{diff}\n");
        let apply = git(
            repo,
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
        git(repo, &["commit", "-m", message], GitOpts::default())?;
        git_ok(repo, &["format-patch", "--full-index", "-1", "--stdout"])
    })();

    if original != "HEAD" {
        git(
            repo,
            &["checkout", "-f", "--quiet", &original],
            GitOpts::default(),
        )?;
    } else {
        git(
            repo,
            &["checkout", "-f", "--quiet", &original_sha],
            GitOpts::default(),
        )?;
    }
    let formatted = formatted?;
    fs::create_dir_all(repo.join(PATCH_DIR))?;
    let body = if formatted.ends_with('\n') {
        formatted
    } else {
        format!("{formatted}\n")
    };
    fs::write(repo.join(&file), body)?;
    Ok(file)
}

pub fn apply_patch_file(
    repo: &Path,
    patch: &Patch,
    patch_file_abs: &Path,
    export_identity: bool,
) -> Result<&'static str> {
    let reverse = git(
        repo,
        &[
            "apply",
            "--reverse",
            "--check",
            patch_file_abs.to_str().unwrap_or(""),
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if reverse.code == 0 {
        return Ok("empty");
    }

    let apply = git(
        repo,
        &[
            "apply",
            "--3way",
            "--index",
            patch_file_abs.to_str().unwrap_or(""),
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if apply.code != 0 {
        return Ok("conflict");
    }

    let staged = git(
        repo,
        &["diff", "--cached", "--quiet"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if staged.code == 0 {
        return Ok("empty");
    }

    let mut opts = GitOpts::default();
    if export_identity && let Some(assess) = &patch.assess {
        opts.extra_env = vec![
            ("GIT_AUTHOR_NAME".into(), assess.author_name.clone()),
            ("GIT_AUTHOR_EMAIL".into(), assess.author_email.clone()),
            ("GIT_COMMITTER_NAME".into(), assess.author_name.clone()),
            ("GIT_COMMITTER_EMAIL".into(), assess.author_email.clone()),
        ];
    }
    let message = if export_identity {
        export_commit_message(patch)
    } else {
        company_commit_message(patch)
    };
    git(repo, &["commit", "-m", &message], opts)?;
    let formatted = git_ok(repo, &["format-patch", "--full-index", "-1", "--stdout"])?;
    let body = if formatted.ends_with('\n') {
        formatted
    } else {
        format!("{formatted}\n")
    };
    let stored = fs::read_to_string(patch_file_abs).unwrap_or_default();
    if patch_substance(&stored) != patch_substance(&body) {
        if let Some(parent) = patch_file_abs.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(patch_file_abs, body)?;
    }
    Ok("applied")
}

/// Patch text with the mbox `From <sha>` line and the `Date:` header removed.
/// Those two fields change on every replay even when the patch itself does not.
fn patch_substance(text: &str) -> String {
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
pub fn patch_already_applied_on(repo: &Path, git_ref: &str, patch_file_abs: &Path) -> Result<bool> {
    let index = repo.join(format!(".git/uplink-apply-check-{}", Uuid::new_v4()));
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
            &[
                "apply",
                "--cached",
                "--reverse",
                "--check",
                patch_file_abs.to_str().unwrap_or(""),
            ],
            index_opts,
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

pub fn state_branch(repo: &Path) -> String {
    let path = repo.join(QUEUE_PATH);
    if path.is_file()
        && let Ok(raw) = fs::read_to_string(&path)
        && let Ok(queue) = serde_json::from_str::<QueueState>(&raw)
        && !queue.config.state_branch.is_empty()
    {
        return queue.config.state_branch;
    }
    STATE_BRANCH.to_string()
}

fn state_ref_source(repo: &Path) -> Result<Option<String>> {
    let branch = state_branch(repo);
    if has_ref(repo, &branch)? {
        return Ok(Some(branch));
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

/// Replace local `uplink/state` with origin and restore `.uplink/`.
pub fn replace_state_from_origin(repo: &Path) -> Result<()> {
    let sha = fetch_tracking_sha(repo, COMPANY_REMOTE, STATE_BRANCH)?;
    apply_state_sha(repo, STATE_BRANCH, &sha)?;
    if !repo.join(QUEUE_PATH).is_file() {
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
    Ok(repo.join(QUEUE_PATH).is_file())
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
    let fetched = git(
        repo,
        &["fetch", "--quiet", COMPANY_REMOTE, &spec],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if fetched.code != 0 {
        return Ok(());
    }
    Ok(())
}

/// Fetch `uplink/upstream` from the company remote and update the local branch.
/// Does not talk to the public `upstream` remote (`fetch_upstream` is sync).
pub fn refresh_upstream_ref(repo: &Path, remote: &str) -> Result<()> {
    let spec = format!("+refs/heads/{UPSTREAM_REF}:refs/remotes/{remote}/{UPSTREAM_REF}");
    let fetched = git(
        repo,
        &["fetch", "--quiet", "--prune", remote, &spec],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if fetched.code != 0 {
        return Ok(());
    }
    let sha = git_ok(repo, &["rev-parse", &format!("{remote}/{UPSTREAM_REF}")])?;
    git(
        repo,
        &["update-ref", &format!("refs/heads/{UPSTREAM_REF}"), &sha],
        GitOpts::default(),
    )?;
    Ok(())
}

fn ensure_uplink_excluded(repo: &Path) -> Result<()> {
    let info = repo.join(".git/info");
    fs::create_dir_all(&info)?;
    let path = info.join("exclude");
    let current = fs::read_to_string(&path).unwrap_or_default();
    if current
        .lines()
        .any(|line| matches!(line.trim(), ".uplink/" | ".uplink"))
    {
        return Ok(());
    }
    let mut body = current;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(".uplink/\n");
    fs::write(path, body)?;
    Ok(())
}

fn restore_state_worktree(repo: &Path, source: &str) -> Result<()> {
    git(
        repo,
        &["restore", "--source", source, "--worktree", "--", ".uplink"],
        GitOpts::default(),
    )?;
    Ok(())
}

pub fn ensure_state_worktree(repo: &Path) -> Result<()> {
    ensure_uplink_excluded(repo)?;
    if repo.join(QUEUE_PATH).is_file() {
        return Ok(());
    }
    let Some(source) = state_ref_source(repo)? else {
        return Ok(());
    };
    let branch = state_branch(repo);
    if source != branch {
        git(
            repo,
            &["branch", "-f", &branch, &source],
            GitOpts::default(),
        )?;
    }
    restore_state_worktree(repo, &branch)
}

fn uplink_worktree_tree(repo: &Path, branch: &str) -> Result<String> {
    ensure_uplink_excluded(repo)?;
    ensure_uplink_dirs(repo)?;
    let index = repo.join(format!(".git/uplink-index-{}", Uuid::new_v4()));
    let index_s = index.to_string_lossy().into_owned();
    let index_opts = GitOpts {
        extra_env: vec![("GIT_INDEX_FILE".into(), index_s)],
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<String> {
        if has_ref(repo, branch)? {
            git(repo, &["read-tree", branch], index_opts.clone())?;
        } else {
            git(repo, &["read-tree", "--empty"], index_opts.clone())?;
        }
        git(repo, &["add", "-f", "--", ".uplink"], index_opts.clone())?;
        Ok(git(repo, &["write-tree"], index_opts.clone())?.stdout)
    })();
    let _ = fs::remove_file(&index);
    outcome
}

pub fn commit_queue(repo: &Path, message: &str) -> Result<()> {
    let branch = state_branch(repo);
    let tree = uplink_worktree_tree(repo, &branch)?;
    if has_ref(repo, &branch)? {
        let old_tree = git_ok(repo, &["rev-parse", &format!("{branch}^{{tree}}")])?;
        if old_tree == tree {
            return Ok(());
        }
    }
    let parent = if has_ref(repo, &branch)? {
        Some(rev_parse(repo, &branch)?)
    } else {
        None
    };
    let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
    if let Some(parent) = parent.as_deref() {
        args.extend_from_slice(&["-p", parent]);
    }
    let commit = git(repo, &args, GitOpts::default())?.stdout;
    git(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), &commit],
        GitOpts::default(),
    )?;
    Ok(())
}

/// Paths under `.uplink/` that differ from the committed `{branch}` tree.
pub fn uplink_uncommitted_paths(repo: &Path, branch: &str) -> Result<Vec<String>> {
    let worktree = uplink_worktree_tree(repo, branch)?;
    let listing = if has_ref(repo, branch)? {
        let committed = git_ok(repo, &["rev-parse", &format!("{branch}^{{tree}}")])?;
        if committed == worktree {
            return Ok(Vec::new());
        }
        git_ok(repo, &["diff", "--name-only", &committed, &worktree])?
    } else {
        git_ok(repo, &["ls-tree", "-r", "--name-only", &worktree])?
    };
    Ok(listing
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
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
    let fetched = git(
        repo,
        &["fetch", "--quiet", "--prune", remote, &spec],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if fetched.code != 0 {
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
    point_branch_at(repo, branch, sha)?;
    let dir = repo.join(".uplink");
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    ensure_uplink_excluded(repo)?;
    restore_state_worktree(repo, branch)
}

/// Fetch `uplink/state` into a remote-tracking ref without moving the local branch.
pub fn fetch_state_tracking(repo: &Path, remote: &str, branch: &str) -> Result<Option<String>> {
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    let fetched = git(
        repo,
        &["fetch", "--quiet", "--prune", remote, &spec],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if fetched.code != 0 {
        return Ok(None);
    }
    Ok(Some(git_ok(
        repo,
        &["rev-parse", &format!("{remote}/{branch}")],
    )?))
}

pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let result = git(
        repo,
        &["merge-base", "--is-ancestor", ancestor, descendant],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    Ok(result.code == 0)
}

pub fn merge_base(repo: &Path, a: &str, b: &str) -> Result<Option<String>> {
    let result = git(
        repo,
        &["merge-base", a, b],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if result.code != 0 || result.stdout.is_empty() {
        return Ok(None);
    }
    Ok(Some(result.stdout))
}

pub fn queue_at(repo: &Path, sha: &str) -> Result<QueueState> {
    let raw = show_at(repo, sha, QUEUE_PATH)?;
    Ok(serde_json::from_str(&raw)?)
}

pub fn set_state_branch(repo: &Path, branch: &str, sha: &str) -> Result<()> {
    git(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), sha],
        GitOpts::default(),
    )?;
    ensure_uplink_excluded(repo)?;
    restore_state_worktree(repo, branch)
}

pub fn restore_paths_from(repo: &Path, source: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["restore", "--source", source, "--worktree", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(repo, &args, GitOpts::default())?;
    Ok(())
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
    git_ok(repo, &["show", &format!("{sha}:{path}")])
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
        &["log", "--format=%H%x09%cI%x09%s", git_ref, "--", path],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
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
    let branch = state_branch(repo);
    let path = patch_path(id)?.to_string_lossy().into_owned();
    let result = git(
        repo,
        &["log", "-1", "--format=%H", &branch, "--", &path],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if result.code == 0 && !result.stdout.is_empty() {
        return Ok(result.stdout);
    }
    rev_parse(repo, &branch)
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

/// Force-with-lease push of a local branch (preview or company main).
pub fn push_branch_force_lease(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let fetch_spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    let fetched = git(
        repo,
        &["fetch", "--quiet", remote, &fetch_spec],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    let dest = format!("refs/heads/{branch}:refs/heads/{branch}");
    if fetched.code == 0 {
        let expected = git_ok(
            repo,
            &["rev-parse", &format!("refs/remotes/{remote}/{branch}")],
        )?;
        let lease = format!("--force-with-lease=refs/heads/{branch}:{expected}");
        git(
            repo,
            &["push", "--quiet", &lease, remote, &dest],
            GitOpts::default(),
        )?;
    } else {
        git(
            repo,
            &["push", "--quiet", remote, &dest],
            GitOpts::default(),
        )?;
    }
    Ok(())
}

pub fn new_patch_id() -> String {
    let raw = Uuid::new_v4().simple().to_string();
    format!("upl_{}", &raw[..10])
}

pub fn stamp() -> String {
    now_iso()
}

pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::patch_substance;

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
