use super::*;

#[derive(Debug, Clone, Default)]
pub struct RebuildOpts {
    pub branch: Option<String>,
    pub push: bool,
    pub push_remote: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RebuildResult {
    pub queue: QueueState,
    pub branch: String,
    pub preview: bool,
}

pub fn rebuild(repo: &Path) -> Result<QueueState> {
    Ok(rebuild_with(repo, RebuildOpts::default())?.queue)
}

pub fn rebuild_with(repo: &Path, opts: RebuildOpts) -> Result<RebuildResult> {
    with_queue_lock(repo, || {
        let queued = read_queue_file(repo)?;
        let company_branch = queued.config.internal_branch.clone();
        let target = opts
            .branch
            .as_deref()
            .unwrap_or(company_branch.as_str())
            .to_string();
        let preview = target != company_branch;
        if preview {
            check_preview_branch(&target, opts.push)?;
        }
        let queue = if preview {
            rebuild_preview(repo, &target)?
        } else {
            rebuild_once(repo)?
        };
        if opts.push {
            let remote = opts.push_remote.as_deref().unwrap_or("origin");
            // State first: a rejected state push leaves origin main alone.
            push_state_branch(repo, remote, STATE_BRANCH)?;
            push_branch_force(repo, remote, &target)?;
        }
        Ok(RebuildResult {
            queue,
            branch: target,
            preview,
        })
    })
}

const PREVIEW_BRANCH_PREFIX: &str = "uplink/preview/";

/// A preview may only overwrite its own `uplink/preview/<name>` branch, and is
/// never force-pushed, so a typo cannot clobber a real branch here or on origin.
fn check_preview_branch(target: &str, push: bool) -> Result<()> {
    if target
        .strip_prefix(PREVIEW_BRANCH_PREFIX)
        .is_none_or(str::is_empty)
    {
        return Err(Error::msg(format!(
            "cannot rebuild onto {target}: preview branches must be named {PREVIEW_BRANCH_PREFIX}<name>"
        )));
    }
    if push {
        return Err(Error::msg(format!(
            "--push publishes company main only; not pushing preview {target}"
        )));
    }
    Ok(())
}

pub(super) fn checkout_identity(repo: &Path) -> Result<(String, String)> {
    Ok((
        git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?,
        git_ok(repo, &["rev-parse", "HEAD"])?,
    ))
}

pub(super) fn restore_checkout(repo: &Path, name: &str, sha: &str) -> Result<()> {
    if name != "HEAD" {
        git(
            repo,
            &["checkout", "-f", "--quiet", name],
            GitOpts::default(),
        )?;
    } else {
        git(
            repo,
            &["checkout", "-f", "--quiet", sha],
            GitOpts::default(),
        )?;
    }
    Ok(())
}

pub(super) fn rebuild_preview(repo: &Path, branch: &str) -> Result<QueueState> {
    let queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    ensure_upstream_ref(repo)?;
    let upstream_ref = if has_ref(repo, "uplink/upstream")? {
        "uplink/upstream"
    } else {
        company_branch.as_str()
    };
    ensure_clean_worktree(repo, "a preview rebuild")?;
    let (original, original_sha) = checkout_identity(repo)?;
    let snapshot = snapshot_uplink(repo)?;
    let outcome = (|| -> Result<QueueState> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", upstream_ref],
            GitOpts::default(),
        )?;
        let mut last_good = git_ok(repo, &["rev-parse", "HEAD"])?;
        git(
            repo,
            &["branch", "-f", branch, &last_good],
            GitOpts::default(),
        )?;
        for patch in apply_order_active(&queue)? {
            if patch.status == PatchStatus::Conflict {
                return Err(Error::msg(format!(
                    "Queue is blocked on conflict in {}",
                    patch.id
                )));
            }
            let patch_file = snapshot.join(patch_path(&patch.id)?);
            let result = apply_patch_file(repo, &patch, &patch_file, false)?;
            if result == ApplyOutcome::Empty {
                continue;
            }
            if result == ApplyOutcome::Conflict {
                let files = conflicted_files(repo)?;
                git(
                    repo,
                    &["reset", "--hard", "--quiet", &last_good],
                    GitOpts::default(),
                )?;
                git(
                    repo,
                    &["branch", "-f", branch, &last_good],
                    GitOpts::default(),
                )?;
                let list = if files.is_empty() {
                    "untracked conflict".into()
                } else {
                    files.join(", ")
                };
                return Err(Error::msg(format!(
                    "Preview rebuild stopped on {} (\"{}\"): {list}",
                    patch.id, patch.title
                )));
            }
            last_good = git_ok(repo, &["rev-parse", "HEAD"])?;
            git(
                repo,
                &["branch", "-f", branch, &last_good],
                GitOpts::default(),
            )?;
        }
        Ok(queue)
    })();
    let _ = fs::remove_dir_all(&snapshot);
    restore_checkout(repo, &original, &original_sha)?;
    crate::repo::ensure_state_worktree(repo)?;
    outcome
}

pub(super) fn rebuild_once(repo: &Path) -> Result<QueueState> {
    let mut queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    ensure_upstream_ref(repo)?;
    let upstream_ref = if has_ref(repo, "uplink/upstream")? {
        "uplink/upstream"
    } else {
        company_branch.as_str()
    };
    ensure_clean_worktree(repo, "a rebuild")?;
    let snapshot = snapshot_uplink(repo)?;
    let outcome = (|| -> Result<QueueState> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", upstream_ref],
            GitOpts::default(),
        )?;
        for patch in apply_order_active(&queue)? {
            if patch.status == PatchStatus::Conflict {
                restore_company_branch(repo, &company_branch)?;
                return Err(Error::Conflict(ConflictError::new(
                    format!("Queue is blocked on conflict in {}", patch.id),
                    patch.id,
                    patch.conflict.map(|c| c.files).unwrap_or_default(),
                )));
            }
            let patch_file = snapshot.join(patch_path(&patch.id)?);
            let result = apply_patch_file(repo, &patch, &patch_file, false)?;
            if result == ApplyOutcome::Empty {
                if queue.is_upstream(&patch.id) {
                    mark_merged_by_empty_rebase(repo, &mut queue, &patch.id, upstream_ref)?;
                }
                continue;
            }
            if result == ApplyOutcome::Conflict {
                let files = conflicted_files(repo)?;
                return Err(Error::Conflict(persist_apply_conflict(
                    repo,
                    &mut queue,
                    &snapshot,
                    &company_branch,
                    upstream_ref,
                    &patch,
                    files,
                )?));
            }

            refresh_patch_id(repo, &mut queue, &patch.id, &patch_file)?;
        }

        publish_rebuilt_company(repo, &mut queue, &snapshot, &company_branch, upstream_ref)?;
        Ok(queue)
    })();
    let _ = fs::remove_dir_all(&snapshot);
    outcome
}

/// An upstream patch that applies empty is already in upstream.
fn mark_merged_by_empty_rebase(
    repo: &Path,
    queue: &mut QueueState,
    id: &str,
    upstream_ref: &str,
) -> Result<()> {
    let current = get_patch_mut(queue, id)?;
    current.status = PatchStatus::Merged;
    current.merged = Some(PatchMerged {
        via: MergeVia::EmptyRebase,
        at: stamp(),
        upstream_sha: Some(rev_parse(repo, upstream_ref)?),
    });
    add_event(
        current,
        "merged",
        "Became empty on rebuild; treating as already present upstream",
    );
    Ok(())
}

/// Refreshes the stable patch id of a patch that applied. A patch in
/// conflict never gets here: the rebuild stops on it before applying.
fn refresh_patch_id(
    repo: &Path,
    queue: &mut QueueState,
    id: &str,
    patch_file: &Path,
) -> Result<()> {
    let contents = fs::read_to_string(patch_file)?;
    let current = get_patch_mut(queue, id)?;
    current.patch_id_stable = Some(stable_patch_id_from_contents(repo, &contents)?);
    Ok(())
}

/// Commits the rebuilt tree, points the company branch at it, restores
/// `.uplink` from the snapshot, and records the rebuild on uplink/state.
fn publish_rebuilt_company(
    repo: &Path,
    queue: &mut QueueState,
    snapshot: &Path,
    company_branch: &str,
    upstream_ref: &str,
) -> Result<()> {
    // Tracked changes only: an untracked file in the operator's checkout
    // survives the detached checkout and must not land on the company branch.
    git(repo, &["add", "-u"], GitOpts::default())?;
    if !git_succeeds(repo, &["diff", "--cached", "--quiet"])? {
        git(
            repo,
            &[
                "commit",
                "-m",
                &format!("uplink: rebuild company {company_branch} onto upstream"),
            ],
            GitOpts::default(),
        )?;
    }
    git(
        repo,
        &["branch", "-f", company_branch, "HEAD"],
        GitOpts::default(),
    )?;
    git(
        repo,
        &["checkout", "-f", "--quiet", company_branch],
        GitOpts::default(),
    )?;
    fs::create_dir_all(repo.join(".uplink/patches"))?;
    copy_dir(&snapshot.join(".uplink"), &repo.join(".uplink"))?;
    queue.last_sync = Some(LastSync {
        at: stamp(),
        upstream_sha: rev_parse(repo, upstream_ref)?,
        result: "ok".into(),
        message: Some("Rebuild completed".into()),
    });
    write_queue_file(repo, queue)?;
    commit_queue(repo, "uplink: record rebuild status")?;
    Ok(())
}

pub fn resolve_conflict(repo: &Path, id: &str) -> Result<QueueState> {
    with_queue_lock(repo, || {
        let base = GateKind::Conflict.base_branch(id);
        let work = GateKind::Conflict.work_branch(id);
        let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        if head != base && head != work {
            return Err(Error::msg(format!(
                "Check out {base} or {work} before resolving {id} (currently on {head})."
            )));
        }
        restore_uplink_from_state(repo)?;
        let mut queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?.clone();
        if patch.status != PatchStatus::Conflict {
            return Err(Error::msg(format!("{id} is not in conflict")));
        }
        assert_resolution_clean(repo)?;
        let onto = patch
            .conflict
            .as_ref()
            .and_then(|c| c.onto.clone())
            .unwrap_or(recover_onto(repo, GateKind::Conflict, id, &head)?);
        let message = company_commit_message(&patch);
        let before = rev_parse(repo, "HEAD")?;
        commit_resolution(repo, &onto, &message)?;
        // The resolution is new code, so the stored assessment no longer
        // describes it. An upstream-bound resolution must pass; otherwise the
        // branch is put back as it was so the resolution can be fixed.
        let intent = PatchIntent::from_internal_only(!queue.is_upstream(id));
        let report = assess_from_message(
            repo,
            &queue,
            &onto,
            "HEAD",
            &stored_commit_message(&patch),
            Some(&patch.title),
            intent,
        )?;
        if !intent.is_internal_only()
            && let Err(err) = assert_assess_ok(&report, &patch.title)
        {
            git(repo, &["reset", "--soft", &before], GitOpts::default())?;
            return Err(err);
        }
        fs::create_dir_all(repo.join(".uplink/patches"))?;
        fs::write(repo.join(patch_path(id)?), format_patch_at_head(repo)?)?;
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.assess = Some(report);
            patch.conflict = None;
            let rel = patch_path(id)?.to_string_lossy().into_owned();
            patch.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
            patch.status = status_after_rewrite(repo, patch);
            let detail = rewrite_event_detail(patch.status, "Conflict resolved");
            add_event(patch, "amended", detail);
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: amend {id} after conflict"))?;
        rebuild(repo)
    })
}
