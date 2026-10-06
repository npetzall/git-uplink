use super::*;

#[derive(Debug)]
pub struct SubmitResult {
    pub queue: QueueState,
    pub branch: String,
    /// The local export commit. The forge recreates it on contrib, so the
    /// public sha differs unless `pushed`.
    pub sha: String,
    /// The `uplink/upstream` commit the export is built on.
    pub base: String,
    /// Tree of the export commit; the forge's API commit must match it.
    pub tree: String,
    pub message: String,
    /// True when `--push` force-pushed the branch to contrib (unsigned).
    pub pushed: bool,
}

/// Builds `uplink/<id>` on `uplink/upstream`. With `push`, force-pushes it to
/// contrib as is. Without, nothing leaves the machine and the forge creates
/// the signed commit from the result.
pub fn submit_patch(repo: &Path, id: &str, push: bool) -> Result<SubmitResult> {
    submit_patch_with(repo, id, push, &ScriptVerdict::Run)
}

/// [`submit_patch`], taking the verdict of `preflight.sh` from `preflight`.
pub fn submit_patch_with(
    repo: &Path,
    id: &str,
    push: bool,
    preflight: &ScriptVerdict,
) -> Result<SubmitResult> {
    with_queue_lock(repo, || {
        ensure_upstream_ref(repo)?;
        let queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?.clone();
        check_ready_to_submit(repo, &queue, &patch, preflight)?;

        let branch = format!("uplink/{id}");
        let base = rev_parse(repo, "uplink/upstream")?;
        let sha = export_onto(repo, &queue, &patch, &base, &branch)?;
        let tree = rev_parse(repo, &format!("{sha}^{{tree}}"))?;
        let message = git_ok(repo, &["log", "-1", "--format=%B", &sha])?;
        if push {
            push_to_contrib(repo, &queue, &branch)?;
        }

        Ok(SubmitResult {
            queue: read_queue_file(repo)?,
            branch,
            sha,
            base,
            tree,
            message,
            pushed: push,
        })
    })
}

/// Upstream-bound, approved, upstream dependencies merged, assess ok,
/// and export preflight passes. Every export is built on `uplink/upstream`.
fn check_ready_to_submit(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    preflight: &ScriptVerdict,
) -> Result<()> {
    let id = &patch.id;
    if !queue.is_upstream(id) {
        return Err(Error::msg(format!("{id} is internal-only")));
    }
    ensure_upstream_deps_merged(queue, patch, "Submit")?;
    if patch.status != PatchStatus::Approved && patch.status != PatchStatus::Submitted {
        return Err(Error::msg(format!(
            "{id} must be approved before submit (currently {})",
            patch.status
        )));
    }
    ensure_assessed_ok(patch)?;
    // The approval is for specific content, not for the patch id.
    review_token(repo, patch)?;
    if covering_approval(repo, patch).is_none() {
        return Err(Error::msg(format!(
            "{id} changed since it was approved; approve the current content before submit (dispatch Uplink submit again)."
        )));
    }
    export_preflight(
        repo,
        queue,
        patch,
        &repo.join(patch_path(id)?),
        None,
        preflight,
    )
    .map(|_| ())
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
    ensure_clean_worktree(repo, "submit")?;
    let snapshot = snapshot_uplink(repo)?;
    let applied = (|| -> Result<ApplyOutcome> {
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
    if applied == ApplyOutcome::Conflict {
        let files = conflicted_files(repo).unwrap_or_default();
        checkout_company(repo, company_branch)?;
        let _ = fs::remove_dir_all(&snapshot);
        return Err(Error::Conflict(ConflictError::new(
            format!("Cannot export {id} onto upstream"),
            id,
            files,
        )));
    }
    if applied == ApplyOutcome::Empty {
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

/// Force-pushes the export branch to the contrib remote.
fn push_to_contrib(repo: &Path, queue: &QueueState, branch: &str) -> Result<()> {
    let contrib = &queue.config.contrib_remote;
    let remotes = git_ok(repo, &["remote"]).unwrap_or_default();
    if !remotes.split('\n').any(|r| r == contrib) {
        return Err(Error::msg(format!(
            "submit --push needs the `{contrib}` remote. Add it, or submit without --push and let the forge create the commit."
        )));
    }
    git(
        repo,
        &["push", "--force", contrib, &format!("{branch}:{branch}")],
        GitOpts::default(),
    )?;
    Ok(())
}

/// Refuses an upstream-bound patch whose stored assessment failed, or that
/// has none: without a report nothing was scanned.
pub(super) fn ensure_assessed_ok(patch: &Patch) -> Result<()> {
    let id = &patch.id;
    match &patch.assess {
        Some(report) if report.ok => Ok(()),
        Some(_) => Err(Error::msg(format!(
            "{id} is not ready for contribution. Fix the upstream assessment findings first."
        ))),
        None => Err(Error::msg(format!(
            "{id} has no upstream assessment on record, so nothing was scanned. \
Amend it (git uplink amend {id}) to assess it, then try again."
        ))),
    }
}

/// Refuses `action` (approve or submit) while an upstream-bound dependency is
/// not merged upstream. Internal-only dependencies never go upstream.
pub(super) fn ensure_upstream_deps_merged(
    queue: &QueueState,
    patch: &Patch,
    action: &str,
) -> Result<()> {
    let id = &patch.id;
    for dep_id in &patch.depends_on {
        let dep = get_patch(queue, dep_id)?;
        if blocks_submit(queue, dep) {
            return Err(Error::msg(format!(
                "{id} depends on {dep_id}, which is not merged upstream yet (currently {}). {action} {id} after {dep_id} is merged.",
                dep.status
            )));
        }
    }
    Ok(())
}

/// An upstream-bound dependency that is not merged upstream yet.
fn blocks_submit(queue: &QueueState, dep: &Patch) -> bool {
    queue.is_upstream(&dep.id) && dep.status != PatchStatus::Merged
}

/// Upstream-bound, queued, and no dependency blocks it. A conflict in
/// another patch does not: submit only needs the patch on `uplink/upstream`.
fn ready_to_submit(queue: &QueueState, patch: &Patch) -> bool {
    queue.is_upstream(&patch.id)
        && patch.status == PatchStatus::Queued
        && patch
            .depends_on
            .iter()
            .all(|dep_id| get_patch(queue, dep_id).is_ok_and(|dep| !blocks_submit(queue, dep)))
}

/// Patches a command made ready to submit: ready in `after`, and not in
/// `before`. A patch that was ready already is not listed again.
pub fn newly_ready_to_submit(before: &QueueState, after: &QueueState) -> Vec<String> {
    after
        .all_patches()
        .filter(|patch| ready_to_submit(after, patch))
        .filter(|patch| {
            !before
                .all_patches()
                .any(|was| was.id == patch.id && ready_to_submit(before, was))
        })
        .map(|patch| patch.id.clone())
        .collect()
}
