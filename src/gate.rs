use std::path::Path;

use crate::error::Result;
use crate::git::{GitOpts, git, git_ok, git_succeeds};
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
        if !git_succeeds(repo, &["diff", "--cached", "--quiet"])? {
            git(repo, &["commit", "-m", message], GitOpts::default())?;
        }
    } else {
        git(repo, &["branch", "-f", &work, "HEAD"], GitOpts::default())?;
    }
    Ok((base, work))
}

/// Cuts the amend base at `after` (the patch applied) and a `-work` branch
/// one empty commit ahead, so a PR can be opened before any change exists.
/// Leaves HEAD on the work branch.
pub fn cut_amend_work(repo: &Path, id: &str, after: &str) -> Result<(String, String)> {
    let base = GateKind::Amend.base_branch(id);
    let work = GateKind::Amend.work_branch(id);
    git(repo, &["branch", "-f", &base, after], GitOpts::default())?;
    git(
        repo,
        &["checkout", "-f", "--quiet", "-B", &work, after],
        GitOpts::default(),
    )?;
    git(
        repo,
        &[
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            &format!("uplink: amend {id}"),
        ],
        GitOpts::default(),
    )?;
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
        GitOpts::allow_fail(),
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
            GitOpts::allow_fail(),
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
    if !git_succeeds(repo, &["diff", "--cached", "--quiet"])? {
        git(repo, &["commit", "-m", message], GitOpts::default())?;
    }
    git(repo, &["reset", "--soft", onto], GitOpts::default())?;
    // Gated work that tracks a file under `.uplink/` must not put it in the
    // patch: the assessment does not scan that path, and the checkout holds
    // the real queue and patches there.
    git(
        repo,
        &["reset", "--quiet", onto, "--", ".uplink"],
        GitOpts::default(),
    )?;
    if !git_succeeds(repo, &["diff", "--cached", "--quiet"])? {
        git(repo, &["commit", "-m", message], GitOpts::default())?;
    }
    Ok(())
}

/// Leaves out `.uplink/`, as the assessment's diff does.
pub fn format_patch_at_head(repo: &Path) -> Result<String> {
    let formatted = git_ok(
        repo,
        &[
            "format-patch",
            "--full-index",
            "-1",
            "--stdout",
            "--",
            ".",
            ":!.uplink",
        ],
    )?;
    if formatted.ends_with('\n') {
        Ok(formatted)
    } else {
        Ok(format!("{formatted}\n"))
    }
}

/// Refuses to continue when tracked files have staged or unstaged changes.
/// Callers that `checkout -f` the operator's checkout would discard them.
/// Untracked files survive a checkout unless the target tree has the same path.
pub(crate) fn ensure_clean_worktree(repo: &Path, what: &str) -> Result<()> {
    if !git_succeeds(repo, &["diff", "--cached", "--quiet"])?
        || !git_succeeds(repo, &["diff", "--quiet"])?
    {
        return Err(crate::error::Error::msg(format!(
            "Uncommitted changes in the working tree; commit or stash them before {what}."
        )));
    }
    Ok(())
}

fn worktree_dirty(repo: &Path) -> Result<bool> {
    if !git_succeeds(repo, &["diff", "--cached", "--quiet"])? {
        return Ok(true);
    }
    if !git_succeeds(repo, &["diff", "--quiet"])? {
        return Ok(true);
    }
    let untracked = git_ok(repo, &["ls-files", "--others", "--exclude-standard"])?;
    Ok(!untracked.trim().is_empty())
}
