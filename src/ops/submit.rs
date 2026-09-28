use super::*;

#[derive(Debug)]
pub struct SubmitResult {
    pub queue: QueueState,
    pub branch: String,
    pub sha: String,
}

pub fn submit_patch(repo: &Path, id: &str) -> Result<SubmitResult> {
    with_queue_lock(repo, || {
        ensure_upstream_ref(repo)?;
        let queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?.clone();
        check_ready_to_submit(repo, &queue, &patch)?;

        let branch = format!("uplink/{id}");
        let start = submit_base(repo, &queue, &patch)?;
        let sha = export_onto(repo, &queue, &patch, &start, &branch)?;
        push_to_contrib(repo, &queue, &branch)?;

        Ok(SubmitResult {
            queue: read_queue_file(repo)?,
            branch,
            sha,
        })
    })
}

/// Upstream-bound, approved, dependencies submitted or merged, assess ok,
/// and export preflight passes.
fn check_ready_to_submit(repo: &Path, queue: &QueueState, patch: &Patch) -> Result<()> {
    let id = &patch.id;
    if !queue.is_upstream(id) {
        return Err(Error::msg(format!("{id} is internal-only")));
    }
    if patch.status != PatchStatus::Approved && patch.status != PatchStatus::Submitted {
        return Err(Error::msg(format!(
            "{id} must be approved before submit (currently {})",
            patch.status
        )));
    }
    for dep_id in &patch.depends_on {
        let dep = get_patch(queue, dep_id)?;
        if queue.is_upstream(dep_id)
            && dep.status != PatchStatus::Merged
            && dep.status != PatchStatus::Submitted
        {
            return Err(Error::msg(format!("Submit {dep_id} before {id}")));
        }
    }
    if patch.assess.as_ref().is_some_and(|p| !p.ok) {
        return Err(Error::msg(format!(
            "{id} is not ready for contribution. Fix assess-for-upstream findings first."
        )));
    }
    assert_export_preflight(repo, queue, patch, &repo.join(patch_path(id)?), None)
}

/// Applies the patch on `start`, points `branch` at the result, and returns
/// to the company branch. Returns the exported commit.
fn export_onto(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    start: &str,
    branch: &str,
) -> Result<String> {
    let id = &patch.id;
    let company_branch = &queue.config.internal_branch;
    let snapshot = snapshot_uplink(repo)?;
    let applied = (|| -> Result<&'static str> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", start],
            GitOpts::default(),
        )?;
        apply_patch_file(repo, patch, &snapshot.join(patch_path(id)?), true)
    })();
    let applied = match applied {
        Ok(v) => v,
        Err(err) => {
            let _ = fs::remove_dir_all(&snapshot);
            return Err(err);
        }
    };
    if applied == "conflict" {
        let files = conflicted_files(repo).unwrap_or_default();
        checkout_company(repo, company_branch)?;
        let _ = fs::remove_dir_all(&snapshot);
        return Err(Error::Conflict(ConflictError::new(
            format!("Cannot export {id} onto upstream"),
            id,
            files,
        )));
    }
    if applied == "empty" {
        checkout_company(repo, company_branch)?;
        let _ = fs::remove_dir_all(&snapshot);
        return Err(Error::msg(format!(
            "{id} applies empty onto upstream; mark it merged instead."
        )));
    }
    git(repo, &["branch", "-f", branch, "HEAD"], GitOpts::default())?;
    let _ = fs::remove_dir_all(&snapshot);

    let sha = rev_parse(repo, branch)?;
    checkout_company(repo, company_branch)?;
    Ok(sha)
}

fn checkout_company(repo: &Path, company_branch: &str) -> Result<()> {
    git(
        repo,
        &["checkout", "-f", "--quiet", company_branch],
        GitOpts::default(),
    )?;
    Ok(())
}

/// Force-pushes the export branch when a contrib remote is configured.
fn push_to_contrib(repo: &Path, queue: &QueueState, branch: &str) -> Result<()> {
    let remotes = git_ok(repo, &["remote"]).unwrap_or_default();
    if remotes
        .split('\n')
        .any(|r| r == queue.config.contrib_remote)
    {
        git(
            repo,
            &[
                "push",
                "--force",
                &queue.config.contrib_remote,
                &format!("{branch}:{branch}"),
            ],
            GitOpts::default(),
        )?;
    }
    Ok(())
}

pub(super) fn submit_base(repo: &Path, queue: &QueueState, patch: &Patch) -> Result<String> {
    let submitted_deps: Vec<&Patch> = patch
        .depends_on
        .iter()
        .filter_map(|id| queue.all_patches().find(|p| p.id == *id))
        .filter(|dep| queue.is_upstream(&dep.id) && dep.status == PatchStatus::Submitted)
        .collect();
    if let Some(last) = submitted_deps.last()
        && let Some(branch) = last.upstream.as_ref().map(|u| u.contrib_branch.as_str())
        && has_ref(repo, branch)?
    {
        return Ok(branch.to_string());
    }
    ensure_upstream_ref(repo)?;
    Ok("uplink/upstream".into())
}
