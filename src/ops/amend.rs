use super::transfer::{PreviewOutcome, apply_queue_preview, not_previewed};
use super::*;
use crate::gate::cut_amend_work;

/// A new title and raw message for the amended patch, as from the merged PR.
#[derive(Debug, Clone)]
pub struct AmendMessage {
    pub title: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AmendResult {
    pub queue: QueueState,
    pub id: String,
    /// False for the start, which only cuts the gated branches.
    pub completed: bool,
    /// Whether complete changed the patch. A merged PR with no code or
    /// message change leaves the queue as it was.
    pub changed: bool,
    pub base_branch: Option<String>,
    pub work_branch: Option<String>,
    pub onto: Option<String>,
}

/// Starts an amend (cuts `uplink/amend/<id>` and `-work`), or with
/// `complete` folds the merged work into the patch.
pub fn amend_patch(
    repo: &Path,
    id: &str,
    complete: bool,
    message: Option<AmendMessage>,
) -> Result<AmendResult> {
    amend_patch_with(repo, id, complete, message, &ScriptVerdict::Run)
}

/// [`amend_patch`], taking the verdict of `preflight.sh` from `preflight`.
pub fn amend_patch_with(
    repo: &Path,
    id: &str,
    complete: bool,
    message: Option<AmendMessage>,
    preflight: &ScriptVerdict,
) -> Result<AmendResult> {
    if !complete && message.is_some() {
        return Err(Error::msg(
            "A new title or message applies only with --complete",
        ));
    }
    with_queue_lock(repo, || {
        if !complete {
            return start_amend(repo, id);
        }
        complete_amend(repo, id, message.as_ref(), Checks::Record(preflight)).map(Checked::recorded)
    })
}

/// What `amend --complete` would test, and what `preflight.sh` says about
/// it. The queue is left as it was; the checkout is not, so run it in a
/// clone made for it.
pub fn amend_preflight(
    repo: &Path,
    id: &str,
    message: Option<AmendMessage>,
) -> Result<PreflightReport> {
    with_queue_lock(repo, || {
        Ok(Checked::report(complete_amend(
            repo,
            id,
            message.as_ref(),
            Checks::Probe,
        )))
    })
}

fn validate_amend(queue: &QueueState, id: &str) -> Result<Patch> {
    if queue.is_tooling(id) {
        return Err(Error::msg(format!(
            "{id} is tooling; change it with git uplink init --upgrade"
        )));
    }
    let patch = get_patch(queue, id)?.clone();
    match patch.status {
        PatchStatus::Conflict => Err(Error::msg(format!(
            "{id} is in conflict; resolve it before amending"
        ))),
        PatchStatus::Merged | PatchStatus::Dropped => Err(Error::msg(format!(
            "{id} cannot be amended while status is {}",
            patch.status
        ))),
        _ => Ok(patch),
    }
}

pub(super) fn upstream_ref_or_company(repo: &Path, queue: &QueueState) -> Result<String> {
    ensure_upstream_ref(repo)?;
    Ok(if has_ref(repo, UPSTREAM_REF)? {
        UPSTREAM_REF.to_string()
    } else {
        queue.config.internal_branch.clone()
    })
}

fn start_amend(repo: &Path, id: &str) -> Result<AmendResult> {
    let queue = read_queue_file(repo)?;
    validate_amend(&queue, id)?;
    let upstream_ref = upstream_ref_or_company(repo, &queue)?;
    ensure_clean_worktree(repo, "an amend")?;
    let (original, original_sha) = checkout_identity(repo)?;
    let snapshot = snapshot_uplink(repo)?;
    let outcome = (|| -> Result<(String, String, String)> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", &upstream_ref],
            GitOpts::default(),
        )?;
        let target = match apply_queue_preview(repo, &queue, &snapshot, id, true)? {
            PreviewOutcome::Conflict { .. } => {
                return Err(Error::msg(format!(
                    "{id} does not apply on {upstream_ref}; run sync first"
                )));
            }
            PreviewOutcome::Applied(target) => target,
        };
        let onto = target.onto.ok_or_else(|| not_previewed(id))?;
        let after = target.after.ok_or_else(|| not_previewed(id))?;
        if onto == after {
            return Err(Error::msg(format!(
                "{id} is already part of {upstream_ref}; nothing to amend"
            )));
        }
        let (base, work) = cut_amend_work(repo, id, &after)?;
        Ok((base, work, onto))
    })();
    let _ = fs::remove_dir_all(&snapshot);
    let restored = restore_checkout(repo, &original, &original_sha)
        .and_then(|_| crate::repo::ensure_state_worktree(repo));
    let (base, work, onto) = outcome?;
    restored?;
    Ok(AmendResult {
        queue,
        id: id.into(),
        completed: false,
        changed: false,
        base_branch: Some(base),
        work_branch: Some(work),
        onto: Some(onto),
    })
}

/// The commit that applied `id` on the amend base: the newest first-parent
/// commit above upstream carrying its `Uplink-Patch-Id` trailer. It survives
/// squash, merge, and rebase merges of the work PR.
fn find_patch_commit(repo: &Path, id: &str, upstream_ref: &str) -> Result<String> {
    let log = git_ok(
        repo,
        &[
            "log",
            "--first-parent",
            "--no-merges",
            "--format=%H %(trailers:key=Uplink-Patch-Id,valueonly,separator=%x2C)",
            "HEAD",
            "--not",
            upstream_ref,
        ],
    )?;
    log.lines()
        .find_map(|line| {
            let (sha, ids) = line.split_once(' ')?;
            ids.split(',')
                .any(|value| value.trim() == id)
                .then(|| sha.to_string())
        })
        .ok_or_else(|| {
            Error::msg(format!(
                "Could not find the commit for {id} on this branch; was it cut by git uplink amend?"
            ))
        })
}

fn tree_of(repo: &Path, rev: &str) -> Result<String> {
    rev_parse(repo, &format!("{rev}^{{tree}}"))
}

fn complete_amend(
    repo: &Path,
    id: &str,
    new_message: Option<&AmendMessage>,
    checks: Checks<'_>,
) -> Result<Checked<AmendResult>> {
    let base = GateKind::Amend.base_branch(id);
    let work = GateKind::Amend.work_branch(id);
    let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if head != base && head != work {
        return Err(Error::msg(format!(
            "Check out {base} or {work} before completing the amend of {id} (currently on {head})."
        )));
    }
    restore_uplink_from_state(repo)?;
    let mut queue = read_queue_file(repo)?;
    let patch = validate_amend(&queue, id)?;
    assert_resolution_clean(repo)?;
    let upstream_ref = upstream_ref_or_company(repo, &queue)?;
    let original = find_patch_commit(repo, id, &upstream_ref)?;
    let onto = rev_parse(repo, &format!("{original}^"))?;

    let mut updated = patch.clone();
    if let Some(change) = new_message {
        let title = change.title.trim();
        if title.is_empty() {
            return Err(Error::msg("The amended patch title cannot be empty"));
        }
        updated.title = title.to_string();
    }
    let raw_message = match new_message {
        Some(change) => change
            .message
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(updated.title.as_str())
            .to_string(),
        None => stored_commit_message(&patch),
    };

    let before = rev_parse(repo, "HEAD")?;
    commit_resolution(repo, &onto, &company_commit_message(&patch))?;
    if rev_parse(repo, "HEAD")? == onto {
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
        return Err(Error::msg(format!(
            "The amend leaves {id} empty; drop the patch instead"
        )));
    }

    let intent = PatchIntent::from_internal_only(!queue.is_upstream(id));
    let checked = (|| -> Result<AssessReport> {
        let report = assess_from_message(
            repo,
            &queue,
            &onto,
            "HEAD",
            &raw_message,
            Some(&updated.title),
            intent,
        )?;
        if !intent.is_internal_only() {
            assert_assess_ok(&report, &updated.title)?;
        }
        if new_message.is_some() {
            updated.commit_message = report.commit_message.clone();
        }
        let message = company_commit_message(&updated);
        git(
            repo,
            &["commit", "--amend", "--quiet", "-m", &message],
            GitOpts::default(),
        )?;
        Ok(report)
    })();
    let report = match checked {
        Ok(report) => report,
        Err(err) => {
            git(repo, &["reset", "--soft", &before], GitOpts::default())?;
            return Err(err);
        }
    };

    let unchanged = tree_of(repo, "HEAD")? == tree_of(repo, &original)?
        && updated.title == patch.title
        && stored_commit_message(&updated) == stored_commit_message(&patch);
    if unchanged {
        git(
            repo,
            &["reset", "--quiet", "--hard", &before],
            GitOpts::default(),
        )?;
        git(
            repo,
            &["checkout", "-f", "--quiet", &queue.config.internal_branch],
            GitOpts::default(),
        )?;
        crate::repo::ensure_state_worktree(repo)?;
        if checks.is_probe() {
            // Nothing changed, so complete has nothing to test.
            return Ok(Checked::Probed(PreflightReport::of(Ok(None))));
        }
        return Ok(Checked::Recorded(AmendResult {
            queue,
            id: id.into(),
            completed: true,
            changed: false,
            base_branch: None,
            work_branch: None,
            onto: None,
        }));
    }

    fs::create_dir_all(repo.join(".uplink/patches"))?;
    let patch_abs = repo.join(patch_path(id)?);
    fs::write(&patch_abs, format_patch_at_head(repo)?)?;
    {
        let current = get_patch_mut(&mut queue, id)?;
        current.title = updated.title.clone();
        current.commit_message = updated.commit_message.clone();
    }
    let verdict = checks.verdict();
    let layer_checks = if intent.is_internal_only() {
        command_preflight(&queue, repo, None, verdict)
    } else {
        export_preflight(
            repo,
            &queue,
            get_patch(&queue, id)?,
            &patch_abs,
            None,
            verdict,
        )
        .and_then(|token| assert_upstream_layer_applies(repo, &queue).map(|_| token))
    };
    if checks.is_probe() {
        let report = probe_report(layer_checks, || {
            probe_rebuild(repo, &queue, &upstream_ref, false)
        });
        restore_uplink_from_state(repo)?;
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
        return Ok(Checked::Probed(report?));
    }
    if layer_checks.is_err() {
        restore_uplink_from_state(repo)?;
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
    }
    layer_checks?;

    {
        let current = get_patch_mut(&mut queue, id)?;
        current.assess = Some(report);
        let rel = patch_path(id)?.to_string_lossy().into_owned();
        current.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
        current.status = status_after_rewrite(repo, current);
        let detail = rewrite_event_detail(current.status, "Amended through the gated amend PR");
        add_event(current, "amended", detail);
    }
    write_queue_file(repo, &queue)?;
    commit_queue(repo, &format!("uplink: amend {id}"))?;
    let queue = rebuild_checked(repo, verdict)?;
    Ok(Checked::Recorded(AmendResult {
        queue,
        id: id.into(),
        completed: true,
        changed: true,
        base_branch: None,
        work_branch: None,
        onto: None,
    }))
}
