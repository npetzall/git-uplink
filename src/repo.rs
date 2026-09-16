use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::prepare::{company_commit_message, export_commit_message};
use crate::queue::{now_iso, patch_path};
use crate::types::{PATCH_DIR, Patch, QUEUE_PATH, QueueState, STATE_BRANCH};

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
    let file = patch_path(id);
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
    if export_identity {
        if let Some(prepare) = &patch.prepare {
            opts.extra_env = vec![
                ("GIT_AUTHOR_NAME".into(), prepare.author_name.clone()),
                ("GIT_AUTHOR_EMAIL".into(), prepare.author_email.clone()),
                ("GIT_COMMITTER_NAME".into(), prepare.author_name.clone()),
                ("GIT_COMMITTER_EMAIL".into(), prepare.author_email.clone()),
            ];
        }
    }
    let message = if export_identity {
        export_commit_message(patch)
    } else {
        company_commit_message(patch)
    };
    git(repo, &["commit", "-m", &message], opts)?;
    let formatted = git_ok(repo, &["format-patch", "--full-index", "-1", "--stdout"])?;
    if let Some(parent) = patch_file_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = if formatted.ends_with('\n') {
        formatted
    } else {
        format!("{formatted}\n")
    };
    fs::write(patch_file_abs, body)?;
    Ok("applied")
}

pub fn conflicted_files(repo: &Path) -> Result<Vec<String>> {
    let output = git_ok(repo, &["diff", "--name-only", "--diff-filter=U"])?;
    Ok(output
        .split('\n')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn fetch_upstream(repo: &Path, queue: &QueueState) -> Result<String> {
    let remote = &queue.config.upstream_remote;
    let branch = &queue.config.upstream_branch;
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    git(
        repo,
        &["fetch", "--quiet", "--prune", remote, &spec],
        GitOpts::default(),
    )?;
    let sha = git_ok(repo, &["rev-parse", &format!("{remote}/{branch}")])?;
    git(
        repo,
        &["branch", "-f", "uplink/upstream", &sha],
        GitOpts::default(),
    )?;
    Ok(sha)
}

pub fn state_branch(repo: &Path) -> String {
    let path = repo.join(QUEUE_PATH);
    if path.is_file() {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(queue) = serde_json::from_str::<QueueState>(&raw) {
                if !queue.config.state_branch.is_empty() {
                    return queue.config.state_branch;
                }
            }
        }
    }
    STATE_BRANCH.to_string()
}

fn state_ref_source(repo: &Path) -> Result<Option<String>> {
    let branch = state_branch(repo);
    if has_ref(repo, &branch)? {
        return Ok(Some(branch));
    }
    let origin = format!("origin/{branch}");
    if has_ref(repo, &origin)? {
        return Ok(Some(origin));
    }
    Ok(None)
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

pub fn commit_queue(repo: &Path, message: &str) -> Result<()> {
    ensure_uplink_excluded(repo)?;
    ensure_uplink_dirs(repo)?;
    let branch = state_branch(repo);
    let index = repo.join(format!(".git/uplink-index-{}", Uuid::new_v4()));
    let index_s = index.to_string_lossy().into_owned();
    let index_opts = GitOpts {
        extra_env: vec![("GIT_INDEX_FILE".into(), index_s)],
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<()> {
        if has_ref(repo, &branch)? {
            git(repo, &["read-tree", &branch], index_opts.clone())?;
        } else {
            git(repo, &["read-tree", "--empty"], index_opts.clone())?;
        }
        git(repo, &["add", "-f", "--", ".uplink"], index_opts.clone())?;
        let tree = git(repo, &["write-tree"], index_opts.clone())?.stdout;
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
    })();
    let _ = fs::remove_file(&index);
    outcome
}

pub fn refresh_state_branch(repo: &Path, remote: &str, branch: &str) -> Result<()> {
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
        return Ok(());
    }
    let sha = git_ok(repo, &["rev-parse", &format!("{remote}/{branch}")])?;
    git(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), &sha],
        GitOpts::default(),
    )?;
    ensure_uplink_excluded(repo)?;
    restore_state_worktree(repo, branch)
}

pub fn push_state_branch(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let dest = format!("refs/heads/{branch}:refs/heads/{branch}");
    git(repo, &["push", remote, &dest], GitOpts::default())?;
    Ok(())
}

pub fn show_at(repo: &Path, sha: &str, path: &str) -> Result<String> {
    git_ok(repo, &["show", &format!("{sha}:{path}")])
}

pub fn patch_state_commit(repo: &Path, id: &str) -> Result<String> {
    let branch = state_branch(repo);
    let path = format!(".uplink/patches/{id}.patch");
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
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    git(
        repo,
        &["fetch", "--quiet", "--prune", remote, &spec],
        GitOpts::default(),
    )?;
    let sha = git_ok(repo, &["rev-parse", &format!("{remote}/{branch}")])?;
    git(
        repo,
        &["checkout", "-f", "--quiet", branch],
        GitOpts::default(),
    )?;
    git(
        repo,
        &["reset", "--hard", "--quiet", &sha],
        GitOpts::default(),
    )?;
    Ok(sha)
}

pub fn push_company_branch(
    repo: &Path,
    remote: &str,
    branch: &str,
    expected_sha: &str,
) -> Result<()> {
    let lease = format!("--force-with-lease=refs/heads/{branch}:{expected_sha}");
    let dest = format!("HEAD:refs/heads/{branch}");
    git(repo, &["push", &lease, remote, &dest], GitOpts::default())?;
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
