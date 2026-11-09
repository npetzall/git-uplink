use super::message::{commit_message_change, with_message};
use super::*;

#[derive(Debug, Clone)]
pub struct TransferResult {
    pub queue: QueueState,
    pub id: String,
    pub direction: TransferDirection,
    pub transferred: bool,
    pub gated: bool,
    pub base_branch: Option<String>,
    pub work_branch: Option<String>,
    pub onto: Option<String>,
    pub files: Vec<String>,
    pub message: Option<String>,
    pub pr_close_url: Option<String>,
    pub pr_close_number: Option<u64>,
    pub pr_close_branch: Option<String>,
    /// Patches this transfer made ready to submit.
    pub ready_to_submit: Vec<String>,
}

pub fn transfer_patch(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    complete: bool,
) -> Result<TransferResult> {
    transfer_patch_with(repo, id, direction, complete, None, &ScriptVerdict::Run)
}

/// [`transfer_patch`] with a new title and message from the merged PR
/// (`complete` only), taking the verdict of `preflight.sh` from `preflight`.
pub fn transfer_patch_with(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    complete: bool,
    message: Option<PatchMessage>,
    preflight: &ScriptVerdict,
) -> Result<TransferResult> {
    with_queue_lock(repo, || {
        let before = read_queue_file(repo)?;
        let mut result = transfer(
            repo,
            id,
            direction,
            complete,
            message.as_ref(),
            Checks::Record(preflight),
        )?
        .recorded();
        result.ready_to_submit = newly_ready_to_submit(&before, &result.queue);
        Ok(result)
    })
}

/// What the transfer would test, and what `preflight.sh` says about it.
/// Nothing is moved, gated or recorded. With `complete` the checkout does
/// not stay as it was, so run it in a clone made for it.
pub fn transfer_preflight(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    complete: bool,
    message: Option<PatchMessage>,
) -> Result<PreflightReport> {
    with_queue_lock(repo, || {
        Ok(Checked::report(transfer(
            repo,
            id,
            direction,
            complete,
            message.as_ref(),
            Checks::Probe,
        )))
    })
}

fn transfer(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    complete: bool,
    message: Option<&PatchMessage>,
    checks: Checks<'_>,
) -> Result<Checked<TransferResult>> {
    if complete {
        complete_transfer(repo, id, direction, message, checks)
    } else if message.is_some() {
        Err(Error::msg(
            "A new title or message applies only with --complete",
        ))
    } else {
        start_transfer(repo, id, direction, checks)
    }
}

pub(super) fn validate_transfer(
    queue: &QueueState,
    id: &str,
    direction: TransferDirection,
) -> Result<Patch> {
    if queue.is_tooling(id) {
        return Err(Error::msg(format!(
            "{id} is tooling and cannot be transferred"
        )));
    }
    let source_ok = match direction {
        TransferDirection::ToUpstream => queue.is_internal(id),
        TransferDirection::ToInternal => queue.is_upstream(id),
    };
    if !source_ok {
        let want = match direction {
            TransferDirection::ToUpstream => "internal",
            TransferDirection::ToInternal => "upstream",
        };
        return Err(Error::msg(format!(
            "{id} is not in the {want} queue; cannot transfer {}",
            direction.as_str()
        )));
    }
    let patch = get_patch(queue, id)?.clone();
    if matches!(
        patch.status,
        PatchStatus::Merged | PatchStatus::Dropped | PatchStatus::Conflict
    ) {
        return Err(Error::msg(format!(
            "{id} cannot be transferred while status is {}",
            patch.status
        )));
    }

    if direction.to_internal() {
        let dependents: Vec<&str> = queue
            .upstream
            .iter()
            .filter(|other| {
                other.id != id && is_active(other) && other.depends_on.iter().any(|dep| dep == id)
            })
            .map(|other| other.id.as_str())
            .collect();
        if !dependents.is_empty() {
            return Err(Error::msg(format!(
                "{id} still has dependents on the upstream queue ({}); transfer those --to-internal first",
                dependents.join(", ")
            )));
        }
    }

    let mut preview = queue.clone();
    move_patch(&mut preview, id, direction.to_internal())?;
    for dep in &patch.depends_on {
        if cannot_depend_on(&preview, direction.to_internal(), dep) {
            return Err(Error::msg(format!(
                "{id} cannot depend on {dep} after transfer {}",
                direction.as_str()
            )));
        }
    }
    for other in preview.all_patches() {
        if other.id == id || !other.depends_on.iter().any(|d| d == id) {
            continue;
        }
        if cannot_depend_on(&preview, preview.is_internal(&other.id), id) {
            return Err(Error::msg(format!(
                "{} depends on {id}; transferring {} would break layer rules",
                other.id,
                direction.as_str()
            )));
        }
    }
    Ok(patch)
}

pub(super) fn assess_transfer_to_upstream(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    from_ref: &str,
    head_ref: &str,
) -> Result<AssessReport> {
    let report = assess_from_message(
        repo,
        queue,
        from_ref,
        head_ref,
        &patch.commit_message.clone(),
        PatchIntent::Upstream,
    )?;
    assert_assess_ok(&report, patch.subject())?;
    Ok(report)
}

pub(super) fn apply_upstream_assess(patch: &mut Patch, report: AssessReport) {
    patch.assess = Some(report);
}

pub(super) fn start_transfer(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    checks: Checks<'_>,
) -> Result<Checked<TransferResult>> {
    let queue = read_queue_file(repo)?;
    let patch = validate_transfer(&queue, id, direction)?;
    let company_branch = queue.config.internal_branch.clone();
    let mut preview = queue.clone();
    move_patch(&mut preview, id, direction.to_internal())?;

    ensure_upstream_ref(repo)?;
    let upstream_ref = if has_ref(repo, "uplink/upstream")? {
        "uplink/upstream"
    } else {
        company_branch.as_str()
    };
    ensure_clean_worktree(repo, "a transfer")?;
    let (original, original_sha) = checkout_identity(repo)?;
    let mut patches = PatchSet::new();
    let outcome = (|| -> Result<Checked<TransferResult>> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", upstream_ref],
            GitOpts::default(),
        )?;
        let target = match apply_queue_preview(repo, &preview, &mut patches, id, false)? {
            // Gated without asking the script.
            PreviewOutcome::Conflict { .. } if checks.is_probe() => {
                return Ok(Checked::Probed(PreflightReport::of(Ok(None))));
            }
            PreviewOutcome::Conflict { onto, files } => {
                return gate_transfer(
                    repo,
                    &queue,
                    id,
                    direction,
                    onto,
                    files,
                    format!("Patch {id} does not apply in the destination layer."),
                )
                .map(Checked::Recorded);
            }
            PreviewOutcome::Applied(target) => target,
        };

        // The checks read the dependencies as the replay left them.
        patches.save(repo)?;
        let checked = transfer_checks(
            repo,
            &preview,
            &patch,
            &patches.get(repo, id)?,
            direction,
            target.onto.as_deref(),
            target.after.as_deref(),
            checks.verdict(),
        );
        let _ = crate::state::discard(repo);
        if checks.is_probe() {
            return probe_report(checked.map(|(_, token)| token), || {
                probe_rebuild_from(repo, &preview, &mut patches, upstream_ref, false)
            })
            .map(Checked::Probed);
        }
        let assess_report = match checked {
            Ok((report, _)) => report,
            // A result for another tree says nothing about this one: gating
            // on it would open a PR for a failure nobody saw.
            Err(Error::Preflight(err)) if err.stage == STAGE_STALE => {
                return Err(Error::Preflight(err));
            }
            Err(err) => {
                let onto = target.onto.ok_or_else(|| not_previewed(id))?;
                if let Some(after) = &target.after {
                    git(
                        repo,
                        &["checkout", "-f", "--quiet", after],
                        GitOpts::default(),
                    )?;
                }
                return gate_transfer(
                    repo,
                    &queue,
                    id,
                    direction,
                    onto,
                    Vec::new(),
                    err.to_string(),
                )
                .map(Checked::Recorded);
            }
        };

        restore_checkout(repo, &original, &original_sha)?;
        crate::repo::ensure_state_branch(repo)?;
        finish_successful_transfer(repo, id, direction, assess_report, checks.verdict())
            .map(Checked::Recorded)
    })();
    match outcome {
        // A finished transfer has already returned to the company branch.
        Ok(Checked::Recorded(result)) if !result.gated => Ok(Checked::Recorded(result)),
        Ok(other) => {
            restore_checkout(repo, &original, &original_sha)?;
            crate::repo::ensure_state_branch(repo)?;
            Ok(other)
        }
        Err(err) => {
            let _ = restore_checkout(repo, &original, &original_sha);
            let _ = crate::repo::ensure_state_branch(repo);
            Err(err)
        }
    }
}

/// Commits around the target patch in the preview apply.
#[derive(Default)]
pub(super) struct TransferTarget {
    /// HEAD before the target patch applied.
    pub(super) onto: Option<String>,
    /// HEAD after it applied.
    pub(super) after: Option<String>,
}

pub(super) enum PreviewOutcome {
    /// The target patch itself did not apply on `onto`.
    Conflict {
        onto: String,
        files: Vec<String>,
    },
    Applied(TransferTarget),
}

pub(super) fn not_previewed(id: &str) -> Error {
    Error::msg(format!("{id} was not applied during transfer preview"))
}

/// Applies `preview` in order from a detached upstream checkout. Only a
/// conflict in the target patch `id` can be gated. With `stop_after_target`,
/// HEAD is left on the target patch's commit.
pub(super) fn apply_queue_preview(
    repo: &Path,
    preview: &QueueState,
    patches: &mut PatchSet,
    id: &str,
    stop_after_target: bool,
) -> Result<PreviewOutcome> {
    let mut target = TransferTarget::default();
    for item in apply_order_active(preview)? {
        if item.status == PatchStatus::Conflict {
            return Err(Error::msg(format!(
                "Queue is blocked on conflict in {}",
                item.id
            )));
        }
        let onto_here = rev_parse(repo, "HEAD")?;
        let text = patches.get(repo, &item.id)?;
        let (result, refreshed) = apply_patch_text(repo, preview, &item, &text, false)?;
        if let Some(refreshed) = refreshed {
            patches.set(&item.id, refreshed);
        }
        if result == ApplyOutcome::Conflict {
            if item.id != id {
                let why = if stop_after_target {
                    "does not apply; run sync or resolve first"
                } else {
                    "would not apply after the move"
                };
                return Err(Error::msg(format!(
                    "Cannot {} {id}: {} (\"{}\") {why}.",
                    if stop_after_target {
                        "amend"
                    } else {
                        "transfer"
                    },
                    item.id,
                    item.subject()
                )));
            }
            return Ok(PreviewOutcome::Conflict {
                onto: onto_here,
                files: conflicted_files(repo)?,
            });
        }
        if item.id == id {
            target.onto = Some(onto_here);
            target.after = Some(rev_parse(repo, "HEAD")?);
            if stop_after_target {
                break;
            }
        }
    }
    Ok(PreviewOutcome::Applied(target))
}

/// To upstream: assess the change `onto..after`, export preflight of `patch_text`,
/// and upstream-layer apply. To internal: `preflight.sh`.
/// Returns the new assess report for an upstream move, and the token of the
/// tree `preflight.sh` was asked about.
#[allow(clippy::too_many_arguments)]
fn transfer_checks(
    repo: &Path,
    preview: &QueueState,
    patch: &Patch,
    patch_text: &str,
    direction: TransferDirection,
    onto: Option<&str>,
    after: Option<&str>,
    verdict: &ScriptVerdict,
) -> Result<(Option<AssessReport>, Option<String>)> {
    match direction {
        TransferDirection::ToUpstream => {
            let onto = onto.ok_or_else(|| not_previewed(&patch.id))?;
            let after = after.ok_or_else(|| not_previewed(&patch.id))?;
            let report = assess_transfer_to_upstream(repo, preview, patch, onto, after)?;
            let token = export_preflight(repo, preview, patch, patch_text, None, verdict)?;
            assert_upstream_layer_applies(repo, preview)?;
            Ok((Some(report), token))
        }
        TransferDirection::ToInternal => {
            command_preflight(preview, repo, None, verdict).map(|token| (None, token))
        }
    }
}

/// Cuts the gated base/work branches at `onto` and reports the transfer as gated.
fn gate_transfer(
    repo: &Path,
    queue: &QueueState,
    id: &str,
    direction: TransferDirection,
    onto: String,
    files: Vec<String>,
    message: String,
) -> Result<TransferResult> {
    let (base, work) = cut_gated_work(
        repo,
        direction.gate_kind(),
        id,
        &onto,
        &format!("uplink: transfer {id} {}", direction.as_str()),
    )?;
    Ok(gated_transfer_result(
        queue.clone(),
        id,
        direction,
        (base, work, onto),
        files,
        message,
    ))
}

pub(super) fn gated_transfer_result(
    queue: QueueState,
    id: &str,
    direction: TransferDirection,
    (base, work, onto): (String, String, String),
    files: Vec<String>,
    message: String,
) -> TransferResult {
    TransferResult {
        queue,
        id: id.into(),
        direction,
        transferred: false,
        gated: true,
        base_branch: Some(base),
        work_branch: Some(work),
        onto: Some(onto),
        files,
        message: Some(message),
        pr_close_url: None,
        pr_close_number: None,
        pr_close_branch: None,
        ready_to_submit: Vec::new(),
    }
}

/// The queue after the rebuild that ends a transfer. The transfer is
/// recorded by then, so a rebuild that stops on a patch `preflight.sh` fails
/// on does not undo it: the queue then holds that patch in conflict.
fn rebuilt_or_stopped(repo: &Path, rebuilt: Result<QueueState>) -> Result<QueueState> {
    match rebuilt {
        Err(Error::Conflict(_)) => read_queue_file(repo),
        other => other,
    }
}

pub(super) fn finish_successful_transfer(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    assess: Option<AssessReport>,
    preflight: &ScriptVerdict,
) -> Result<TransferResult> {
    let mut queue = read_queue_file(repo)?;
    validate_transfer(&queue, id, direction)?;
    let (pr_close_url, pr_close_number, pr_close_branch) =
        pr_close_from_patch(get_patch(&queue, id)?, direction);
    move_patch(&mut queue, id, direction.to_internal())?;
    {
        let patch = get_patch_mut(&mut queue, id)?;
        patch.status = PatchStatus::Queued;
        patch.conflict = None;
        if direction.to_internal() {
            patch.upstream = None;
            patch.approvals.clear();
        }
        if let Some(report) = assess {
            apply_upstream_assess(patch, report);
        }
        add_event(
            patch,
            "transferred",
            format!("Moved {}", direction.as_str()),
        );
    }
    write_queue_file(repo, &queue)?;
    commit_queue(
        repo,
        &format!("uplink: transfer {id} {}", direction.as_str()),
    )?;
    let queue = rebuilt_or_stopped(repo, rebuild_checked(repo, preflight))?;
    Ok(TransferResult {
        queue,
        id: id.into(),
        direction,
        transferred: true,
        gated: false,
        base_branch: None,
        work_branch: None,
        onto: None,
        files: Vec::new(),
        message: None,
        pr_close_url,
        pr_close_number,
        pr_close_branch,
        ready_to_submit: Vec::new(),
    })
}

pub(super) fn complete_transfer(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    change: Option<&PatchMessage>,
    checks: Checks<'_>,
) -> Result<Checked<TransferResult>> {
    let kind = direction.gate_kind();
    let base = kind.base_branch(id);
    let work = kind.work_branch(id);
    let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if head != base && head != work {
        return Err(Error::msg(format!(
            "Check out {base} or {work} before completing transfer of {id} (currently on {head})."
        )));
    }
    restore_uplink_from_state(repo)?;
    let queue = read_queue_file(repo)?;
    let mut patch = validate_transfer(&queue, id, direction)?;
    assert_resolution_clean(repo)?;
    let onto = recover_onto(repo, kind, id, &head)?;
    merge_authors(&mut patch.source.authors, resolution_authors(repo, &onto)?);
    let message = company_commit_message(&queue, &patch);
    let before = rev_parse(repo, "HEAD")?;
    commit_resolution(repo, &onto, &message)?;
    let (updated, message_report) =
        match change_message(repo, &queue, &patch, &onto, direction, change) {
            Ok(changed) => changed,
            Err(err) => {
                git(repo, &["reset", "--soft", &before], GitOpts::default())?;
                return Err(err);
            }
        };
    crate::state::write(repo, patch_path(id)?, format_patch_at_head(repo)?)?;

    let mut preview = queue.clone();
    move_patch(&mut preview, id, direction.to_internal())?;
    {
        let current = get_patch_mut(&mut preview, id)?;
        current.commit_message = updated.commit_message.clone();
        current.source.authors = updated.source.authors.clone();
    }
    let checked = transfer_checks(
        repo,
        &preview,
        get_patch(&preview, id)?,
        &crate::state::patch_text(repo, id)?,
        direction,
        Some(&onto),
        Some("HEAD"),
        checks.verdict(),
    );
    if checks.is_probe() {
        let report = probe_report(checked.map(|(_, token)| token), || {
            let upstream_ref = super::amend::upstream_ref_or_company(repo, &preview)?;
            probe_rebuild(repo, &preview, &upstream_ref, false)
        });
        restore_uplink_from_state(repo)?;
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
        return Ok(Checked::Probed(report?));
    }
    let (assess_report, _) = checked?;
    // An internal-only patch is not assessed for upstream; a new message
    // still needs its own assessment.
    let assess_report = assess_report.or(message_report);

    let rel = patch_path(id)?.to_string_lossy().into_owned();
    let stable = stable_patch_id(repo, &rel)?;
    let mut queue = read_queue_file(repo)?;
    let (pr_close_url, pr_close_number, pr_close_branch) =
        pr_close_from_patch(get_patch(&queue, id)?, direction);
    record_transfer(&mut queue, id, direction, stable, assess_report)?;
    {
        let current = get_patch_mut(&mut queue, id)?;
        current.commit_message = updated.commit_message;
        current.source.authors = updated.source.authors;
    }
    write_queue_file(repo, &queue)?;
    commit_queue(
        repo,
        &format!("uplink: transfer {id} {}", direction.as_str()),
    )?;
    let queue = rebuilt_or_stopped(repo, rebuild_checked(repo, checks.verdict()))?;
    Ok(Checked::Recorded(TransferResult {
        queue,
        id: id.into(),
        direction,
        transferred: true,
        gated: false,
        base_branch: None,
        work_branch: None,
        onto: None,
        files: Vec::new(),
        message: None,
        pr_close_url,
        pr_close_number,
        pr_close_branch,
        ready_to_submit: Vec::new(),
    }))
}

/// `patch` with the title and message of `change`, committed on HEAD, and
/// the assessment of that message in the patch's new layer.
fn change_message(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    onto: &str,
    direction: TransferDirection,
    change: Option<&PatchMessage>,
) -> Result<(Patch, Option<AssessReport>)> {
    let (updated, raw_message) = with_message(patch, change)?;
    if change.is_none() {
        return Ok((updated, None));
    }
    let report = assess_from_message(
        repo,
        queue,
        onto,
        "HEAD",
        &raw_message,
        PatchIntent::from_internal_only(direction.to_internal()),
    )?;
    commit_message_change(repo, queue, &updated, change)?;
    Ok((updated, Some(report)))
}

/// Moves the patch to its new layer as queued, with the gated work's patch id.
fn record_transfer(
    queue: &mut QueueState,
    id: &str,
    direction: TransferDirection,
    stable: String,
    assess_report: Option<AssessReport>,
) -> Result<()> {
    move_patch(queue, id, direction.to_internal())?;
    let current = get_patch_mut(queue, id)?;
    current.status = PatchStatus::Queued;
    current.conflict = None;
    current.patch_id_stable = Some(stable);
    if direction.to_internal() {
        current.upstream = None;
        current.approvals.clear();
    }
    if let Some(report) = assess_report {
        apply_upstream_assess(current, report);
    }
    add_event(
        current,
        "transferred",
        format!("Moved {} after gated work", direction.as_str()),
    );
    Ok(())
}

pub(super) fn pr_close_from_patch(
    patch: &Patch,
    direction: TransferDirection,
) -> (Option<String>, Option<u64>, Option<String>) {
    if !direction.to_internal() {
        return (None, None, None);
    }
    match &patch.upstream {
        Some(upstream) => (
            upstream.pr_url.clone(),
            upstream.pr_number,
            Some(upstream.contrib_branch.clone()),
        ),
        None => (None, None, None),
    }
}
