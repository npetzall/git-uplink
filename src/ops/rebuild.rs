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
        let state_branch = queued.config.state_branch.clone();
        let target = opts
            .branch
            .as_deref()
            .unwrap_or(company_branch.as_str())
            .to_string();
        if is_reserved_rebuild_branch(&target, &queued.config) {
            return Err(Error::msg(format!(
                "cannot rebuild onto reserved branch {target}"
            )));
        }
        let preview = target != company_branch;
        let queue = if preview {
            rebuild_preview(repo, &target)?
        } else {
            rebuild_once(repo)?
        };
        if opts.push {
            let remote = opts.push_remote.as_deref().unwrap_or("origin");
            push_branch_force(repo, remote, &target)?;
            push_state_branch(repo, remote, &state_branch)?;
        }
        Ok(RebuildResult {
            queue,
            branch: target,
            preview,
        })
    })
}

pub(super) fn is_reserved_rebuild_branch(name: &str, config: &crate::types::QueueConfig) -> bool {
    name == "uplink/state"
        || name == config.state_branch
        || name == "uplink/upstream"
        || name == adopt::ADOPT_FROM_REF
        || name.starts_with("uplink/conflict/")
        || name.starts_with("uplink/transfer-to-upstream/")
        || name.starts_with("uplink/transfer-to-internal/")
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
            if result == "empty" {
                continue;
            }
            if result == "conflict" {
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
            if result == "empty" {
                if queue.is_upstream(&patch.id) {
                    mark_merged_by_empty_rebase(repo, &mut queue, &patch.id, upstream_ref)?;
                }
                continue;
            }
            if result == "conflict" {
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

            record_clean_apply(repo, &mut queue, &patch.id, &patch_file)?;
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

/// Refreshes the stable patch id and clears any earlier conflict.
fn record_clean_apply(
    repo: &Path,
    queue: &mut QueueState,
    id: &str,
    patch_file: &Path,
) -> Result<()> {
    let contents = fs::read_to_string(patch_file)?;
    let current = get_patch_mut(queue, id)?;
    current.patch_id_stable = Some(stable_patch_id_from_contents(repo, &contents)?);
    if current.status == PatchStatus::Conflict {
        current.status = if current
            .upstream
            .as_ref()
            .and_then(|u| u.pr_number)
            .is_some()
        {
            PatchStatus::Submitted
        } else {
            PatchStatus::Queued
        };
    }
    current.conflict = None;
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
    git(repo, &["add", "-A"], GitOpts::default())?;
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
        commit_resolution(repo, &onto, &message)?;
        fs::create_dir_all(repo.join(".uplink/patches"))?;
        fs::write(repo.join(patch_path(id)?), format_patch_at_head(repo)?)?;
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = if patch.upstream.is_some() {
                PatchStatus::Amended
            } else {
                PatchStatus::Queued
            };
            patch.conflict = None;
            let rel = patch_path(id)?.to_string_lossy().into_owned();
            patch.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
            add_event(
                patch,
                "amended",
                "Conflict resolved; patch refreshed for rebuild and upstream PR",
            );
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: amend {id} after conflict"))?;
        rebuild(repo)
    })
}
