use std::path::Path;

use crate::error::Result;
use crate::git::{GitOpts, git, git_ok};
use crate::repo::{conflicted_files, rev_parse};
use crate::types::GateKind;

pub fn cut_gated_work(
    repo: &Path,
    kind: GateKind,
    id: &str,
    onto: &str,
    message: &str,
) -> Result<(String, String)> {
    let base = kind.base_branch(id);
    let work = kind.work_branch(id);
    git(repo, &["branch", "-f", &base, onto], GitOpts::default())?;

    let head = rev_parse(repo, "HEAD")?;
    let dirty = worktree_dirty(repo)?;
    if dirty || head == onto {
        git(repo, &["branch", "-f", &work, onto], GitOpts::default())?;
        git(
            repo,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{work}")],
            GitOpts::default(),
        )?;
        git(repo, &["add", "-A"], GitOpts::default())?;
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
    } else {
        git(repo, &["branch", "-f", &work, "HEAD"], GitOpts::default())?;
    }
    Ok((base, work))
}

pub fn assert_resolution_clean(repo: &Path) -> Result<()> {
    let unmerged = conflicted_files(repo)?;
    if !unmerged.is_empty() {
        return Err(crate::error::Error::msg(format!(
            "Conflict still has unmerged files: {}. Fix and git add them first.",
            unmerged.join(", ")
        )));
    }
    let markers = git(
        repo,
        &["grep", "-I", "-l", "^<<<<<<<", "--", ".", ":!.uplink"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if markers.code == 0 && !markers.stdout.trim().is_empty() {
        return Err(crate::error::Error::msg(format!(
            "Conflict markers still present in {}. Remove them before resolve.",
            markers.stdout.trim().replace('\n', ", ")
        )));
    }
    Ok(())
}

pub fn recover_onto(repo: &Path, kind: GateKind, id: &str, head: &str) -> Result<String> {
    let base = kind.base_branch(id);
    let work = kind.work_branch(id);
    if head == work {
        return rev_parse(repo, &base);
    }
    if head == base {
        let parent = git(
            repo,
            &["rev-parse", "--verify", "HEAD^"],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        )?;
        if parent.code == 0 {
            return Ok(parent.stdout.trim().to_string());
        }
        return rev_parse(repo, "HEAD");
    }
    Err(crate::error::Error::msg(format!(
        "Check out {base} or {work} before completing {id} (currently on {head})."
    )))
}

pub fn commit_resolution(repo: &Path, onto: &str, message: &str) -> Result<()> {
    git(repo, &["add", "-A"], GitOpts::default())?;
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
    git(repo, &["reset", "--soft", onto], GitOpts::default())?;
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

pub fn format_patch_at_head(repo: &Path) -> Result<String> {
    let formatted = git_ok(repo, &["format-patch", "--full-index", "-1", "--stdout"])?;
    if formatted.ends_with('\n') {
        Ok(formatted)
    } else {
        Ok(format!("{formatted}\n"))
    }
}

fn worktree_dirty(repo: &Path) -> Result<bool> {
    let staged = git(
        repo,
        &["diff", "--cached", "--quiet"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if staged.code != 0 {
        return Ok(true);
    }
    let unstaged = git(
        repo,
        &["diff", "--quiet"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if unstaged.code != 0 {
        return Ok(true);
    }
    let untracked = git_ok(repo, &["ls-files", "--others", "--exclude-standard"])?;
    Ok(!untracked.trim().is_empty())
}
