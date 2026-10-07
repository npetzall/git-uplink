use super::*;

#[derive(Debug, Clone, Default)]
pub struct RebuildOpts {
    pub branch: Option<String>,
    pub push: bool,
    pub push_remote: Option<String>,
    /// Where the verdict of `preflight.sh` on the rebuilt tree comes from.
    pub preflight: ScriptVerdict,
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

/// [`rebuild`], taking the verdict of `preflight.sh` from `preflight`.
pub(super) fn rebuild_checked(repo: &Path, preflight: &ScriptVerdict) -> Result<QueueState> {
    let opts = RebuildOpts {
        preflight: preflight.clone(),
        ..RebuildOpts::default()
    };
    Ok(rebuild_with(repo, opts)?.queue)
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
            rebuild_once(repo, Some(&opts.preflight))?
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

/// How replaying the queue onto upstream ended.
enum Replayed {
    /// Every active patch applied: its id and the commit it made, in order.
    /// A patch that applied empty made none.
    Applied(Vec<(String, String)>),
    /// The queue already holds a patch in conflict.
    Blocked(Patch),
    Conflict(Patch, Vec<String>),
}

/// Applies the active patches in order on the checkout, which is detached
/// at `upstream_ref`, one commit each.
fn replay_queue(
    repo: &Path,
    queue: &mut QueueState,
    snapshot: &Path,
    upstream_ref: &str,
) -> Result<Replayed> {
    let mut applied = Vec::new();
    for patch in apply_order_active(queue)? {
        if patch.status == PatchStatus::Conflict {
            return Ok(Replayed::Blocked(patch));
        }
        let patch_file = snapshot.join(patch_path(&patch.id)?);
        let result = apply_patch_file(repo, &patch, &patch_file, false)?;
        if result == ApplyOutcome::Empty {
            if queue.is_upstream(&patch.id) {
                mark_merged_by_empty_rebase(repo, queue, &patch.id, upstream_ref)?;
            }
            continue;
        }
        if result == ApplyOutcome::Conflict {
            return Ok(Replayed::Conflict(patch, conflicted_files(repo)?));
        }
        refresh_patch_id(repo, queue, &patch.id, &patch_file)?;
        applied.push((patch.id, rev_parse(repo, "HEAD")?));
    }
    Ok(Replayed::Applied(applied))
}

/// The patch `preflight.sh` first fails on in a rebuild.
struct FailedPatch {
    id: String,
    sha: String,
    output: Option<String>,
}

fn tree_of(repo: &Path, rev: &str) -> Result<String> {
    rev_parse(repo, &format!("{rev}^{{tree}}"))
}

/// Gets the verdict of `preflight.sh` on a rebuild: `applied` on top of
/// `upstream_ref`. A tree equal to the company branch has nothing new to
/// test. When the script fails, `uplink/upstream` is the known good commit
/// and the rebuilt tree the known bad one, so `git bisect` finds the patch
/// to blame. An upstream that is not verified is tested first: when it
/// fails too, no patch is blamed.
///
/// `probe` is filled with every verdict the script gave here.
fn check_rebuilt(
    repo: &Path,
    queue: &mut QueueState,
    upstream_ref: &str,
    applied: &[(String, String)],
    verdict: &ScriptVerdict,
    mut probe: Option<&mut RebuildReport>,
) -> Result<Option<FailedPatch>> {
    let Some((_, head)) = applied.last() else {
        return Ok(None);
    };
    let company_branch = queue.config.internal_branch.clone();
    if has_ref(repo, &company_branch)? && tree_of(repo, head)? == tree_of(repo, &company_branch)? {
        return Ok(None);
    }
    let built = rev_preflight(
        repo,
        queue,
        head,
        &verdict.rebuild_part(|report| report.built.as_ref()),
    )
    .map(Some);
    if let Some(report) = probe.as_deref_mut() {
        report.built = Some(PreflightReport::of_ref(&built));
    }
    let failure = match built {
        Ok(_) => return Ok(None),
        Err(Error::Preflight(err)) if is_script_failure(&err) => err,
        Err(err) => return Err(err),
    };

    let upstream_token = rev_token(repo, queue, upstream_ref)?;
    let verified = queue
        .verified_upstream
        .as_ref()
        .is_some_and(|known| known.token == upstream_token);
    if !verified {
        let upstream = rev_preflight(
            repo,
            queue,
            upstream_ref,
            &verdict.rebuild_part(|report| report.upstream.as_ref()),
        )
        .map(Some);
        if let Some(report) = probe.as_deref_mut() {
            report.upstream = Some(PreflightReport::of_ref(&upstream));
        }
        match upstream {
            Ok(_) => {
                queue.verified_upstream = Some(VerifiedUpstream {
                    sha: rev_parse(repo, upstream_ref)?,
                    token: upstream_token,
                    at: stamp(),
                });
            }
            Err(Error::Preflight(err)) if is_script_failure(&err) => {
                return Err(upstream_failure(
                    "The rebuild fails preflight.sh, and so does uplink/upstream without any patch, so no patch is blamed. Fix preflight.sh on uplink/hooks, or sync to an upstream that passes.",
                    err,
                ));
            }
            Err(err) => return Err(err),
        }
    }

    let (id, sha) = match verdict.first_bad() {
        Some(blamed) => {
            let found = applied.iter().find(|(id, _)| *id == blamed.id);
            match found {
                Some(found) if rev_token(repo, queue, &found.1)? == blamed.token => found.clone(),
                _ => return Err(stale_first_bad(&blamed.token)),
            }
        }
        None if matches!(verdict, ScriptVerdict::Reported(_)) => {
            return Err(stale_first_bad(failure.token.as_deref().unwrap_or("")));
        }
        None if applied.len() == 1 => applied[0].clone(),
        None => {
            let sha = bisect_first_bad(repo, queue, upstream_ref, head)?;
            applied
                .iter()
                .find(|(_, commit)| *commit == sha)
                .cloned()
                .ok_or_else(|| {
                    Error::msg(format!("git bisect blamed {sha}, which is not a patch"))
                })?
        }
    };
    if let Some(report) = probe {
        report.first_bad = Some(FirstBad {
            id: id.clone(),
            token: rev_token(repo, queue, &sha)?,
        });
    }
    Ok(Some(FailedPatch {
        id,
        sha,
        output: failure.output,
    }))
}

/// What `preflight.sh` says about the rebuild of `queue` from the patch
/// files in `snapshot`. Leaves the checkout detached on what it applied.
pub(super) fn probe_rebuild_from(
    repo: &Path,
    queue: &QueueState,
    snapshot: &Path,
    upstream_ref: &str,
) -> Result<RebuildReport> {
    let mut queue = queue.clone();
    let mut report = RebuildReport::default();
    git(
        repo,
        &["checkout", "-f", "--quiet", "--detach", upstream_ref],
        GitOpts::default(),
    )?;
    // A rebuild that stops on a conflict never asks for a verdict.
    let Replayed::Applied(applied) = replay_queue(repo, &mut queue, snapshot, upstream_ref)? else {
        return Ok(report);
    };
    let checked = check_rebuilt(
        repo,
        &mut queue,
        upstream_ref,
        &applied,
        &ScriptVerdict::Run,
        Some(&mut report),
    );
    match checked {
        // In the report.
        Err(Error::Preflight(err)) if is_script_failure(&err) => Ok(report),
        other => other.map(|_| report),
    }
}

/// [`probe_rebuild_from`] with the patch files in `.uplink`, returning to
/// the commit that was checked out.
pub(super) fn probe_rebuild(
    repo: &Path,
    queue: &QueueState,
    upstream_ref: &str,
) -> Result<RebuildReport> {
    let (original, original_sha) = checkout_identity(repo)?;
    let snapshot = snapshot_uplink(repo)?;
    let outcome = probe_rebuild_from(repo, queue, &snapshot, upstream_ref);
    let _ = fs::remove_dir_all(&snapshot);
    restore_checkout(repo, &original, &original_sha)?;
    crate::repo::ensure_state_worktree(repo)?;
    outcome
}

/// A probe's report: `checked` on the tree the command tests, and, when
/// that passed, the rebuild it would end with.
pub(super) fn probe_report(
    checked: Result<Option<String>>,
    rebuild: impl FnOnce() -> Result<RebuildReport>,
) -> Result<PreflightReport> {
    let mut report = PreflightReport::of_ref(&checked);
    if checked.is_ok() {
        report.rebuild = Some(Box::new(rebuild()?));
    }
    Ok(report)
}

/// Replays the queue onto upstream and publishes the company branch.
/// `check` is where the verdict of `preflight.sh` on the result comes from;
/// `None` publishes without one.
pub(super) fn rebuild_once(repo: &Path, check: Option<&ScriptVerdict>) -> Result<QueueState> {
    let mut queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    ensure_upstream_ref(repo)?;
    let upstream_ref = if has_ref(repo, "uplink/upstream")? {
        "uplink/upstream"
    } else {
        company_branch.as_str()
    };
    ensure_clean_worktree(repo, "a rebuild")?;
    let previous = replaced_main(repo, &company_branch, upstream_ref)?;
    let snapshot = snapshot_uplink(repo)?;
    let outcome = (|| -> Result<QueueState> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", upstream_ref],
            GitOpts::default(),
        )?;
        let applied = match replay_queue(repo, &mut queue, &snapshot, upstream_ref)? {
            Replayed::Applied(applied) => applied,
            Replayed::Blocked(patch) => {
                restore_company_branch(repo, &company_branch)?;
                return Err(Error::Conflict(ConflictError::new(
                    format!("Queue is blocked on conflict in {}", patch.id),
                    patch.id,
                    patch.conflict.map(|c| c.files).unwrap_or_default(),
                )));
            }
            Replayed::Conflict(patch, files) => {
                return Err(Error::Conflict(persist_conflict(
                    repo,
                    &mut queue,
                    &snapshot,
                    &company_branch,
                    upstream_ref,
                    &patch,
                    Stopped::Apply { files },
                )?));
            }
        };

        if let Some(verdict) = check {
            let failed = check_rebuilt(repo, &mut queue, upstream_ref, &applied, verdict, None)
                .inspect_err(|_| {
                    let _ = restore_company_branch(repo, &company_branch);
                })?;
            if let Some(failed) = failed {
                let patch = get_patch(&queue, &failed.id)?.clone();
                return Err(Error::Conflict(persist_conflict(
                    repo,
                    &mut queue,
                    &snapshot,
                    &company_branch,
                    upstream_ref,
                    &patch,
                    Stopped::Preflight {
                        sha: failed.sha,
                        output: failed.output,
                    },
                )?));
            }
        }

        publish_rebuilt_company(
            repo,
            &mut queue,
            &snapshot,
            &company_branch,
            upstream_ref,
            previous.as_ref(),
        )?;
        Ok(queue)
    })();
    let _ = fs::remove_dir_all(&snapshot);
    outcome
}

/// The company main this rebuild is about to replace, for `git uplink rebase`:
/// its tip and the commits a branch may have started from that do not
/// identify themselves. Origin's main is included when it is known and
/// differs, since that is the one open pull requests were cut from.
fn replaced_main(
    repo: &Path,
    company_branch: &str,
    upstream_ref: &str,
) -> Result<Option<PreviousMain>> {
    if !has_ref(repo, company_branch)? {
        return Ok(None);
    }
    let tip = rev_parse(repo, company_branch)?;
    let mut tips = vec![tip.clone()];
    let tracking = format!("{COMPANY_REMOTE}/{company_branch}");
    if has_ref(repo, &tracking)? {
        let remote_tip = rev_parse(repo, &tracking)?;
        if remote_tip != tip {
            tips.push(remote_tip);
        }
    }
    let mut commits = Vec::new();
    for tip in &tips {
        let listed = commits_with_patch_id(
            repo,
            &["--first-parent", tip.as_str(), "--not", upstream_ref],
        )?;
        for (sha, has_patch_id) in listed {
            if !has_patch_id && !commits.contains(&sha) {
                commits.push(sha);
            }
        }
    }
    Ok(Some(PreviousMain {
        at: stamp(),
        tip,
        commits,
    }))
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
/// `.uplink` from the snapshot, and records the rebuild and the main it
/// replaced on uplink/state.
fn publish_rebuilt_company(
    repo: &Path,
    queue: &mut QueueState,
    snapshot: &Path,
    company_branch: &str,
    upstream_ref: &str,
    previous: Option<&PreviousMain>,
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
    if let Some(previous) = previous {
        write_previous_main(repo, previous)?;
    }
    commit_queue(repo, "uplink: record rebuild status")?;
    Ok(())
}

pub fn resolve_conflict(repo: &Path, id: &str) -> Result<QueueState> {
    resolve_conflict_with(repo, id, &ScriptVerdict::Run)
}

/// [`resolve_conflict`], taking the verdict of `preflight.sh` on the
/// rebuild from `preflight`.
pub fn resolve_conflict_with(
    repo: &Path,
    id: &str,
    preflight: &ScriptVerdict,
) -> Result<QueueState> {
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
        rebuild_checked(repo, preflight)
    })
}
