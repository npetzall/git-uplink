use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::prepare::export_commit_message;
use crate::queue::{now_iso, patch_path};
use crate::types::{PATCH_DIR, Patch, QueueState};

pub fn ensure_uplink_dirs(repo: &Path) -> Result<()> {
    fs::create_dir_all(repo.join(PATCH_DIR))?;
    Ok(())
}

pub fn commit_message(patch: &Patch) -> String {
    export_commit_message(patch)
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
    let message = commit_message(patch);
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

pub fn commit_queue(repo: &Path, message: &str) -> Result<()> {
    git(repo, &["add", "--", ".uplink"], GitOpts::default())?;
    let staged = git(
        repo,
        &["diff", "--cached", "--quiet"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if staged.code != 0 {
        git(repo, &["commit", "-m", message], GitOpts::default())?;
    }
    Ok(())
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
