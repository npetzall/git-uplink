use serde::Serialize;

use super::*;
use crate::assess::TO_UPSTREAM_ENVIRONMENT;

pub fn approve_patch(repo: &Path, id: &str) -> Result<Patch> {
    approve_patch_at(repo, id, None, None)
}

pub fn approve_patch_at(
    repo: &Path,
    id: &str,
    sha: Option<&str>,
    run_url: Option<&str>,
) -> Result<Patch> {
    approve_patch_reviewed(repo, id, sha, run_url, None)
}

/// The patch is no longer what the reviewer was shown.
pub(super) fn changed_since_review(id: &str) -> Error {
    Error::msg(format!(
        "{id} changed since the packet was reviewed; write a new packet and approve that (dispatch Uplink submit again)."
    ))
}

/// True for an approved or submitted upstream patch whose newest approval
/// does not cover its current content, for example after a replay onto a
/// moved upstream. `submit` refuses it until it is approved again.
pub fn approval_stale(repo: &Path, queue: &QueueState, patch: &Patch) -> bool {
    needs_covering_approval(queue, patch) && covering_approval(repo, patch).is_none()
}

/// [`approval_stale`] with the patch file read from `git_ref` instead of
/// `uplink/state` with this command's changes.
pub fn approval_stale_at(repo: &Path, git_ref: &str, queue: &QueueState, patch: &Patch) -> bool {
    if !needs_covering_approval(queue, patch) {
        return false;
    }
    let text =
        patch_path(&patch.id).and_then(|path| show_at(repo, git_ref, &path.to_string_lossy()));
    text.ok()
        .and_then(|text| covering_approval_for(repo, patch, &text))
        .is_none()
}

fn needs_covering_approval(queue: &QueueState, patch: &Patch) -> bool {
    queue.is_upstream(&patch.id)
        && matches!(patch.status, PatchStatus::Approved | PatchStatus::Submitted)
}

/// The status of a patch after a conflict resolution or an amend rewrote it.
/// A rewrite that adds and removes the same lines (see `changed_lines`) is
/// still covered by the last approval and needs no other one. Any other
/// rewrite makes the patch `queued` again, whatever review it was in; one
/// with a public PR then gets a delta review ([`Patch::reviews_delta`]).
pub(super) fn status_after_rewrite(repo: &Path, patch: &Patch) -> PatchStatus {
    match (
        covering_approval(repo, patch).is_some(),
        patch.upstream.is_some(),
    ) {
        (true, true) => PatchStatus::Submitted,
        (true, false) => PatchStatus::Approved,
        (false, _) => PatchStatus::Queued,
    }
}

/// The event detail for a rewrite that ended in `status`.
pub(super) fn rewrite_event_detail(status: PatchStatus, how: &str) -> String {
    match status {
        PatchStatus::Submitted | PatchStatus::Approved => format!(
            "{how}; patch refreshed and re-assessed. It adds and removes the same lines, so the last approval still covers it"
        ),
        _ => format!("{how}; patch refreshed and re-assessed"),
    }
}

/// Ids of the patches [`approval_stale`] is true for, in queue order.
pub fn stale_approvals(repo: &Path, queue: &QueueState) -> Vec<String> {
    queue
        .all_patches()
        .filter(|patch| approval_stale(repo, queue, patch))
        .map(|patch| patch.id.clone())
        .collect()
}

/// [`stale_approvals`] with patch files read from `git_ref`.
pub fn stale_approvals_at(repo: &Path, git_ref: &str, queue: &QueueState) -> Vec<String> {
    queue
        .all_patches()
        .filter(|patch| approval_stale_at(repo, git_ref, queue, patch))
        .map(|patch| patch.id.clone())
        .collect()
}

/// Records the to-upstream approval for the patch as it is now. `reviewed` is
/// the review token of the packet the reviewer saw; when the patch no longer
/// has that token, nothing is approved.
pub fn approve_patch_reviewed(
    repo: &Path,
    id: &str,
    sha: Option<&str>,
    run_url: Option<&str>,
    reviewed: Option<&str>,
) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        {
            if queue.is_internal(id) || queue.is_tooling(id) {
                return Err(Error::msg(format!(
                    "{id} is internal-only and cannot be approved for upstream."
                )));
            }
            super::submit::ensure_upstream_deps_merged(&queue, get_patch(&queue, id)?, "Approve")?;
            let token = review_token(repo, get_patch(&queue, id)?)?;
            if reviewed.is_some_and(|reviewed| reviewed != token) {
                return Err(changed_since_review(id));
            }
            let patch = get_patch_mut(&mut queue, id)?;
            super::submit::ensure_assessed_ok(patch)?;
            let kind = match patch.status {
                PatchStatus::Queued | PatchStatus::Waiting | PatchStatus::Rejected => {
                    if patch.reviews_delta() {
                        "delta"
                    } else {
                        "initial"
                    }
                }
                PatchStatus::Approved | PatchStatus::Submitted => {
                    if patch
                        .last_approval()
                        .is_some_and(|approval| approval_covers(repo, approval, patch, &token))
                    {
                        return Ok(patch.clone());
                    }
                    // Replayed onto a moved upstream since the last approval.
                    "refresh"
                }
                _ => {
                    return Err(Error::msg(format!(
                        "{id} is {} and cannot be approved for contribution.",
                        patch.status
                    )));
                }
            };
            if kind != "refresh" {
                patch.status = PatchStatus::Approved;
            }
            let sha = match sha {
                Some(value) if !value.is_empty() => value.to_string(),
                _ => rev_parse(repo, STATE_BRANCH)?,
            };
            let run_url = run_url
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(github_run_url_opt);
            let version = patch.approvals.len() as u32 + 1;
            let at = stamp();
            patch.approvals.push(PatchApproval {
                at: at.clone(),
                version,
                kind: kind.into(),
                sha,
                patch_id_stable: patch.patch_id_stable.clone(),
                run_url,
                reviewed: Some(token),
            });
            add_event(
                patch,
                "approved",
                match kind {
                    "delta" => "IP approved the delta since the previous contribution approval",
                    "refresh" => "IP approved the patch as replayed since the previous approval",
                    _ => "IP and contribution review passed; patch may leave the enterprise",
                },
            );
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: approve {id}"))?;
        Ok(get_patch(&queue, id)?.clone())
    })
}

pub(super) fn github_run_url_opt() -> Option<String> {
    let server = std::env::var("GITHUB_SERVER_URL").ok()?;
    let repository = std::env::var("GITHUB_REPOSITORY").ok()?;
    let run_id = std::env::var("GITHUB_RUN_ID").ok()?;
    Some(format!("{server}/{repository}/actions/runs/{run_id}"))
}

pub(super) fn mark_dropped(queue: &mut QueueState, id: &str, reason: &str) -> Result<()> {
    let patch = get_patch_mut(queue, id)?;
    patch.status = PatchStatus::Dropped;
    patch.conflict = None;
    add_event(patch, "dropped", reason);
    Ok(())
}

pub fn drop_patch(repo: &Path, id: &str, reason: &str) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        if queue.is_tooling(id) {
            return Err(Error::msg(format!(
                "{id} is the tooling patch; it holds the uplink workflows. Refresh it with `git uplink init --upgrade` instead of dropping it."
            )));
        }
        let status = get_patch(&queue, id)?.status;
        if !status.is_active() {
            return Err(Error::msg(format!("{id} is already {status}")));
        }
        mark_dropped(&mut queue, id, reason)?;
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: drop {id}"))?;
        rebuild(repo)?;
        Ok(get_patch(&read_queue_file(repo)?, id)?.clone())
    })
}

/// Ids `trim-deps` takes out of one patch's `depends_on`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimmedDeps {
    pub id: String,
    /// `depends_on` after the trim.
    pub depends_on: Vec<String>,
    pub removed: Vec<TrimmedDep>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimmedDep {
    pub id: String,
    /// The dependency kept that brings this one in.
    pub implied_by: String,
}

/// Takes out of every `depends_on` the ids another of its dependencies
/// brings in already. What applies before each patch stays the same, so
/// nothing is rebuilt. With `dry_run`, or when nothing changes, writes
/// nothing.
pub fn trim_deps(repo: &Path, dry_run: bool) -> Result<Vec<TrimmedDeps>> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let snapshot = queue.clone();
        let mut trimmed = Vec::new();
        for patch in queue.all_patches_mut() {
            let (kept, removed) = transitive_reduction(&snapshot, &patch.depends_on)?;
            if kept == patch.depends_on {
                continue;
            }
            let removed: Vec<TrimmedDep> = removed
                .into_iter()
                .map(|dep| TrimmedDep {
                    id: dep.id,
                    implied_by: dep.by,
                })
                .collect();
            if !removed.is_empty() {
                let detail = removed
                    .iter()
                    .map(|dep| format!("{} (implied by {})", dep.id, dep.implied_by))
                    .collect::<Vec<_>>()
                    .join(", ");
                add_event(patch, "dependency", format!("Trimmed dependency {detail}"));
            }
            patch.depends_on = kept.clone();
            trimmed.push(TrimmedDeps {
                id: patch.id.clone(),
                depends_on: kept,
                removed,
            });
        }
        if !dry_run && !trimmed.is_empty() {
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: trim depends-on")?;
        }
        Ok(trimmed)
    })
}

/// Marks `id` merged on an operator's word. `via` is checked against the
/// evidence it names, so the label can be trusted; `manual` is not checked.
pub fn mark_merged(
    repo: &Path,
    id: &str,
    via: MergeVia,
    upstream_sha: Option<&str>,
) -> Result<Patch> {
    verify_merge_claim(repo, id, via, upstream_sha)?;
    record_merged(repo, id, via, upstream_sha)
}

fn verify_merge_claim(
    repo: &Path,
    id: &str,
    via: MergeVia,
    upstream_sha: Option<&str>,
) -> Result<()> {
    let queue = read_queue_file(repo)?;
    let patch = get_patch(&queue, id)?;
    let need_sha = || {
        upstream_sha.ok_or_else(|| {
            Error::msg(format!(
                "--via {via} needs --sha: the upstream commit that carries {id}"
            ))
        })
    };
    let resolve = |sha: &str| -> Result<String> {
        let resolved = git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{sha}^{{commit}}"),
            ],
            GitOpts::allow_fail(),
        )?;
        if resolved.code != 0 {
            return Err(Error::msg(format!("{sha} is not a commit in this clone")));
        }
        Ok(resolved.stdout)
    };
    match via {
        MergeVia::Manual => Ok(()),
        MergeVia::Pr => match patch.upstream.as_ref().and_then(|u| u.pr_number) {
            Some(_) => Ok(()),
            None => Err(Error::msg(format!(
                "--via pr: {id} has no recorded public PR"
            ))),
        },
        MergeVia::StablePatchId | MergeVia::Trailer => {
            let sha = resolve(need_sha()?)?;
            let stable = commit_stable_patch_id(repo, &sha)?;
            if stable.is_none() || stable.as_deref() != patch.patch_id_stable.as_deref() {
                return Err(Error::msg(format!(
                    "--via {via}: the stable patch-id of {sha} does not match {id}"
                )));
            }
            if via == MergeVia::Trailer && !has_trailer(repo, &queue, &sha, id) {
                return Err(Error::msg(format!(
                    "--via trailer: {sha} has no `{}: {id}` trailer",
                    queue.config.trailer_key
                )));
            }
            Ok(())
        }
        MergeVia::EmptyRebase => {
            let at = match upstream_sha {
                Some(sha) => resolve(sha)?,
                None => "uplink/upstream".to_string(),
            };
            if patch_already_applied_on(repo, &at, &crate::state::patch_text(repo, id)?)? {
                Ok(())
            } else {
                Err(Error::msg(format!(
                    "--via empty-rebase: {id} is not contained in {at}"
                )))
            }
        }
    }
}

/// Marks `id` merged with evidence the caller has already verified.
pub(super) fn record_merged(
    repo: &Path,
    id: &str,
    via: MergeVia,
    upstream_sha: Option<&str>,
) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        if !queue.is_upstream(id) {
            get_patch(&queue, id)?;
            return Err(Error::msg(format!(
                "{id} is not in the upstream queue; internal-only and tooling patches are never submitted upstream"
            )));
        }
        let status = get_patch(&queue, id)?.status;
        if !status.is_active() {
            return Err(Error::msg(format!("{id} is already {status}")));
        }
        let merged = PatchMerged {
            via,
            at: stamp(),
            upstream_sha: upstream_sha.map(str::to_string),
        };
        set_merged(&mut queue, id, merged, merged_detail(via, upstream_sha))?;
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: merged {id}"))?;
        Ok(get_patch(&queue, id)?.clone())
    })
}

pub(super) const STATE_PATCH_PUSH_ATTEMPTS: u32 = 8;

pub(super) fn origin_amended_after_local(local: &Patch, origin: &Patch) -> bool {
    let local_had_amend = local.events.iter().any(|e| e.kind == "amended");
    let origin_amended = origin.events.iter().any(|e| e.kind == "amended");
    origin_amended && !local_had_amend
}

/// Appends the events from `src` that `dest` lacks. Events compare in full,
/// timestamp included, so a repeated approval with the same detail survives.
fn merge_events(dest: &mut Vec<PatchEvent>, src: Vec<PatchEvent>) {
    for event in src {
        if !dest.contains(&event) {
            dest.push(event);
        }
    }
}

pub(super) fn replay_recorded_patch_onto_origin(
    repo: &Path,
    id: &str,
    from_sha: &str,
) -> Result<()> {
    let source = queue_at(repo, from_sha)?;
    let src = get_patch(&source, id)?.clone();
    let mut queue = read_queue_file(repo)?;
    {
        let dest = get_patch_mut(&mut queue, id)?;
        if dest.status == PatchStatus::Conflict && src.status != PatchStatus::Conflict {
            return Err(Error::msg(format!(
                "{id} is conflict on origin; not recording the pull request"
            )));
        }
        if dest.status == PatchStatus::Dropped {
            return Err(Error::msg(format!(
                "{id} is dropped on origin; not recording the pull request"
            )));
        }
        if origin_amended_after_local(&src, dest) {
            return Err(Error::msg(format!(
                "{id} was amended on origin after this recording; not clobbering with a stale PR"
            )));
        }
        dest.status = src.status;
        dest.upstream = src.upstream.clone();
        dest.approvals = src.approvals.clone();
        if let Some(src_conflict) = src.conflict {
            match dest.conflict.as_mut() {
                Some(dest_conflict) => {
                    dest_conflict.pr_number = src_conflict.pr_number;
                    dest_conflict.pr_url = src_conflict.pr_url;
                }
                None => dest.conflict = Some(src_conflict),
            }
        }
        merge_events(&mut dest.events, src.events);
        dest.updated_at = stamp();
    }
    write_queue_file(repo, &queue)?;
    if let Ok((_, _, receipt)) = report_paths(id)
        && path_exists_at(repo, from_sha, &receipt)?
    {
        restore_paths_from(repo, from_sha, &[receipt])?;
    }
    Ok(())
}

pub(super) fn push_state_replaying_patch(
    repo: &Path,
    remote: &str,
    branch: &str,
    id: &str,
    local_sha: &str,
    message: &str,
) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..STATE_PATCH_PUSH_ATTEMPTS {
        match push_state_branch(repo, remote, branch) {
            Ok(()) => return Ok(()),
            Err(err) if is_push_lease_rejected(&err) && attempt + 1 < STATE_PATCH_PUSH_ATTEMPTS => {
                let Some(remote_sha) = fetch_state_tracking(repo, remote, branch)? else {
                    return Err(err);
                };
                if is_ancestor(repo, local_sha, &remote_sha)? {
                    set_state_branch(repo, branch, &remote_sha)?;
                    return Ok(());
                }
                set_state_branch(repo, branch, &remote_sha)?;
                replay_recorded_patch_onto_origin(repo, id, local_sha)?;
                commit_queue(repo, message)?;
                last_error = Some(err);
                thread::sleep(Duration::from_millis(40 * 2u64.pow(attempt)));
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| Error::msg("push failed")))
}

pub(super) fn push_recorded_patch(
    repo: &Path,
    id: &str,
    state_branch: &str,
    message: &str,
    push_remote: Option<&str>,
) -> Result<()> {
    let Some(remote) = push_remote else {
        return Ok(());
    };
    let local_sha = rev_parse(repo, state_branch)?;
    push_state_replaying_patch(repo, remote, state_branch, id, &local_sha, message)
}

pub fn record_pull_request(
    repo: &Path,
    id: &str,
    number: u64,
    url: &str,
    branch: &str,
    push_remote: Option<&str>,
) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let state_branch = STATE_BRANCH;
        let patch = get_patch(&queue, id)?.clone();
        if let Some(existing) = patch.upstream.as_ref() {
            if existing.pr_number == Some(number)
                && existing.pr_url.as_deref() == Some(url)
                && patch.status == PatchStatus::Submitted
            {
                let message = format!("uplink: submit {id} as PR {number}");
                push_recorded_patch(repo, id, state_branch, &message, push_remote)?;
                return Ok(patch);
            }
            if let Some(recorded_number) = existing.pr_number
                && (recorded_number != number || existing.pr_url.as_deref() != Some(url))
            {
                let recorded = existing
                    .pr_url
                    .clone()
                    .unwrap_or_else(|| format!("#{recorded_number}"));
                return Err(Error::msg(format!(
                    "{id} is already submitted as {recorded}; will not retarget to {url}"
                )));
            }
        }
        // A public PR exists only for content that was approved and exported.
        if !matches!(patch.status, PatchStatus::Approved | PatchStatus::Submitted) {
            return Err(Error::msg(format!(
                "{id} is {}; record a public PR only after approve and submit.",
                patch.status
            )));
        }
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = PatchStatus::Submitted;
            patch.upstream = Some(PatchUpstream {
                contrib_branch: branch.into(),
                pr_number: Some(number),
                pr_url: Some(url.into()),
                submitted_at: Some(stamp()),
            });
            add_event(patch, "submitted", format!("Upstream PR {url}"));
        }
        write_queue_file(repo, &queue)?;
        let message = format!("uplink: submit {id} as PR {number}");
        commit_queue(repo, &message)?;
        push_recorded_patch(repo, id, state_branch, &message, push_remote)?;
        Ok(get_patch(&read_queue_file(repo)?, id)?.clone())
    })
}

pub fn record_gated_pr(
    repo: &Path,
    id: &str,
    number: u64,
    url: &str,
    push_remote: Option<&str>,
) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let state_branch = STATE_BRANCH;
        let patch = get_patch(&queue, id)?.clone();
        if patch.status != PatchStatus::Conflict {
            return Err(Error::msg(format!("{id} is not in conflict")));
        }
        let Some(conflict) = patch.conflict.clone() else {
            return Err(Error::msg(format!("{id} has no conflict record")));
        };
        if conflict.pr_number == Some(number) && conflict.pr_url.as_deref() == Some(url) {
            let message = format!("uplink: conflict PR {id} #{number}");
            push_recorded_patch(repo, id, state_branch, &message, push_remote)?;
            return Ok(patch);
        }
        if let Some(recorded_number) = conflict.pr_number
            && (recorded_number != number || conflict.pr_url.as_deref() != Some(url))
        {
            let recorded = conflict
                .pr_url
                .unwrap_or_else(|| format!("#{recorded_number}"));
            return Err(Error::msg(format!(
                "{id} already has conflict PR {recorded}; will not retarget to {url}"
            )));
        }
        {
            let patch = get_patch_mut(&mut queue, id)?;
            let conflict = patch
                .conflict
                .as_mut()
                .ok_or_else(|| Error::msg(format!("{id} has no conflict record")))?;
            conflict.pr_number = Some(number);
            conflict.pr_url = Some(url.into());
            add_event(patch, "conflict-pr", format!("Conflict PR {url}"));
        }
        write_queue_file(repo, &queue)?;
        let message = format!("uplink: conflict PR {id} #{number}");
        commit_queue(repo, &message)?;
        push_recorded_patch(repo, id, state_branch, &message, push_remote)?;
        Ok(get_patch(&read_queue_file(repo)?, id)?.clone())
    })
}

/// Stores company assessment-hook extras for patch `id` on `uplink/state` and
/// marks them fresh for its current content. The packet reuses them until the
/// patch changes.
pub fn store_patch_extras(
    repo: &Path,
    id: &str,
    dir: &Path,
    source: Option<String>,
) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let patch = get_patch_mut(&mut queue, id)?;
        store_extras(repo, patch, dir, source)?;
        add_event(patch, "extras", "Stored company assessment-hook extras");
        let patch = patch.clone();
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: store assessment extras {id}"))?;
        Ok(patch)
    })
}

/// Whether the contribution packet of `patch` shows an assessment made for
/// it: an upstream-bound patch that is waiting for contribution.
fn assessed_for_packet(queue: &QueueState, patch: &Patch) -> bool {
    queue.is_upstream(&patch.id)
        && matches!(
            patch.status,
            PatchStatus::Queued
                | PatchStatus::Waiting
                | PatchStatus::Rejected
                | PatchStatus::Approved
                | PatchStatus::Submitted
        )
}

/// Stores `report` as the assessment the contribution packet of `id` shows.
fn store_packet_assessment(
    repo: &Path,
    queue: &mut QueueState,
    id: &str,
    report: AssessReport,
) -> Result<Patch> {
    let detail = if report.ok {
        "Assessed the patch for the contribution packet"
    } else {
        "Assessed the patch for the contribution packet; it has findings to fix"
    };
    let current = get_patch_mut(queue, id)?;
    current.assess = Some(report);
    add_event(current, "assessed", detail);
    let current = current.clone();
    write_queue_file(repo, queue)?;
    commit_queue(repo, &format!("uplink: assess {id} for its packet"))?;
    Ok(current)
}

/// Runs the upstream assessment on the patch file of `id` as it is in the
/// queue, with the settings in `uplink.toml` as they are today. The result
/// replaces the one stored when the patch was imported, amended or resolved,
/// and is what the contribution packet shows. Nothing is applied: whether
/// the patch still applies on upstream is preflight's question.
///
/// Nothing is assessed for an internal-only patch or a patch that is not
/// waiting for contribution.
pub fn assess_patch_for_packet(repo: &Path, id: &str) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?.clone();
        if !assessed_for_packet(&queue, &patch) {
            return Ok(patch);
        }
        let intent = PatchIntent::from_internal_only(false);
        let report = assess_patch_file(&queue, &patch, &patch_text(repo, id)?, intent);
        store_packet_assessment(repo, &mut queue, id, report)
    })
}

/// Stores the assessment in `package` (`assess --patch <id> --package`) as
/// the one the contribution packet of `id` shows, instead of assessing here.
///
/// The package is used only when it describes the patch as it is now. The
/// patch is assessed once more to tell: the same patch file, title, message
/// and `uplink.toml` give the same result, so a package that is out of date
/// or was changed on its way here is refused.
pub fn store_assess_result(repo: &Path, id: &str, package: &AssessPackage) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?.clone();
        if !assessed_for_packet(&queue, &patch) {
            return Err(Error::msg(format!(
                "{id} is not an upstream-bound patch waiting for contribution (currently {}); it has no assessment to store.",
                patch.status
            )));
        }
        let text = patch_text(repo, id)?;
        let intent = PatchIntent::from_internal_only(false);
        let fresh = assess_patch_file(&queue, &patch, &text, intent);
        if let Some(why) = package.stale_for(repo, &patch, &text, &fresh)? {
            return Err(Error::msg(format!(
                "The assessment result is not for {id} as it is now: {why}. Assess it again (dispatch Uplink submit again)."
            )));
        }
        store_packet_assessment(repo, &mut queue, id, package.report())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(at: &str, kind: &str, detail: &str) -> PatchEvent {
        PatchEvent {
            at: at.into(),
            kind: kind.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn merge_events_keeps_repeated_approvals() {
        let first = event("2026-01-01T00:00:00Z", "approved", "delta approval");
        let second = event("2026-01-02T00:00:00Z", "approved", "delta approval");
        let mut dest = vec![first.clone()];
        merge_events(&mut dest, vec![first.clone(), second.clone()]);
        assert_eq!(dest, vec![first, second]);
    }
}

/// The `waiting` event detail for the submit run `run_url`.
fn waiting_detail(run_url: Option<&str>) -> String {
    match run_url {
        Some(url) => format!("Waiting on the {TO_UPSTREAM_ENVIRONMENT} review in {url}"),
        None => format!("Waiting on the {TO_UPSTREAM_ENVIRONMENT} review"),
    }
}

fn run_url_or_env(run_url: Option<&str>) -> Option<String> {
    run_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(github_run_url_opt)
}

/// What [`mark_waiting`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingOutcome {
    /// The patch is now `waiting`.
    Waiting,
    /// An approval already covers the patch; its status is left as it is.
    Covered,
}

/// Records that a submit run passed every check and now waits on the
/// to-upstream review: the patch is upstream-bound, its upstream
/// dependencies are merged, its assessment passed, and `preflight` is a
/// passing result for the patch as it is now. Refused, with the status left
/// as it was, when any of that does not hold.
pub fn mark_waiting(
    repo: &Path,
    id: &str,
    preflight: &ScriptVerdict,
    run_url: Option<&str>,
) -> Result<(Patch, WaitingOutcome)> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        if !queue.is_upstream(id) {
            get_patch(&queue, id)?;
            return Err(Error::msg(format!(
                "{id} is not upstream-bound; only upstream patches wait on the {TO_UPSTREAM_ENVIRONMENT} review."
            )));
        }
        let patch = get_patch(&queue, id)?;
        match patch.status {
            PatchStatus::Queued | PatchStatus::Rejected | PatchStatus::Waiting => {}
            PatchStatus::Approved | PatchStatus::Submitted => {
                if !approval_stale(repo, &queue, patch) {
                    return Ok((patch.clone(), WaitingOutcome::Covered));
                }
            }
            status => {
                return Err(Error::msg(format!(
                    "{id} is {status} and cannot wait on the {TO_UPSTREAM_ENVIRONMENT} review."
                )));
            }
        }
        super::submit::ensure_upstream_deps_merged(&queue, patch, "Waiting")?;
        super::submit::ensure_assessed_ok(patch)?;
        let token = review_token(repo, patch)?;
        if !super::submit::passed_for_content(preflight, &token) {
            return Err(Error::msg(format!(
                "{id} has no passing preflight result for the patch as it is now; not waiting on the {TO_UPSTREAM_ENVIRONMENT} review."
            )));
        }
        let run_url = run_url_or_env(run_url);
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = PatchStatus::Waiting;
            add_event(patch, "waiting", waiting_detail(run_url.as_deref()));
        }
        write_queue_file(repo, &queue)?;
        commit_queue(
            repo,
            &format!("uplink: {id} waiting on {TO_UPSTREAM_ENVIRONMENT}"),
        )?;
        Ok((get_patch(&queue, id)?.clone(), WaitingOutcome::Waiting))
    })
}

/// How the review of a waiting patch ended without approval.
#[derive(Debug, Clone, Default)]
pub struct Rejection {
    /// The submit run that set `waiting`. Without it, any waiting run matches.
    pub run_url: Option<String>,
    /// The run was cancelled instead of rejected.
    pub cancelled: bool,
    /// Who rejected it.
    pub by: Option<String>,
    /// The reviewer's comment.
    pub comment: Option<String>,
}

/// Records that the submit run that set `waiting` was rejected or
/// cancelled. Returns `None`, and changes nothing, when the patch is no
/// longer waiting for that run: an amend, a resolve or a newer run took
/// over.
pub fn mark_rejected(repo: &Path, id: &str, rejection: &Rejection) -> Result<Option<Patch>> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?;
        if patch.status != PatchStatus::Waiting {
            return Ok(None);
        }
        let run_url = rejection
            .run_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(url) = run_url {
            let latest = patch.events.iter().rev().find(|e| e.kind == "waiting");
            if latest.is_none_or(|event| event.detail != waiting_detail(Some(url))) {
                return Ok(None);
            }
        }
        let mut detail = if rejection.cancelled {
            "The submit run was cancelled while waiting on the review".to_string()
        } else {
            match rejection
                .by
                .as_deref()
                .map(str::trim)
                .filter(|by| !by.is_empty())
            {
                Some(by) => format!("{by} rejected the {TO_UPSTREAM_ENVIRONMENT} review"),
                None => format!("The {TO_UPSTREAM_ENVIRONMENT} review was rejected"),
            }
        };
        if let Some(comment) = rejection
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|comment| !comment.is_empty())
        {
            detail.push_str(&format!(": {comment}"));
        }
        if let Some(url) = run_url {
            detail.push_str(&format!(" ({url})"));
        }
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = PatchStatus::Rejected;
            add_event(patch, "rejected", detail);
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: {id} rejected"))?;
        Ok(Some(get_patch(&queue, id)?.clone()))
    })
}
