use super::*;

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

/// True when `approval` was given for the content `token` identifies.
/// Approvals from before tokens existed compare the stable patch id.
pub(super) fn approval_covers(approval: &PatchApproval, patch: &Patch, token: &str) -> bool {
    match approval.reviewed.as_deref() {
        Some(reviewed) => reviewed == token,
        None => {
            approval.patch_id_stable.is_some() && approval.patch_id_stable == patch.patch_id_stable
        }
    }
}

/// True for an approved or submitted upstream patch whose newest approval
/// does not cover its current content, for example after a replay onto a
/// moved upstream. `submit` refuses it until it is approved again.
pub fn approval_stale(repo: &Path, queue: &QueueState, patch: &Patch) -> bool {
    if !queue.is_upstream(&patch.id)
        || !matches!(patch.status, PatchStatus::Approved | PatchStatus::Submitted)
    {
        return false;
    }
    let Ok(token) = review_token(repo, patch) else {
        return true;
    };
    !patch
        .last_approval()
        .is_some_and(|approval| approval_covers(approval, patch, &token))
}

/// Ids of the patches [`approval_stale`] is true for, in queue order.
pub fn stale_approvals(repo: &Path, queue: &QueueState) -> Vec<String> {
    queue
        .all_patches()
        .filter(|patch| approval_stale(repo, queue, patch))
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
            if patch.assess.as_ref().is_some_and(|p| !p.ok) {
                return Err(Error::msg(format!(
                    "{id} is not ready for contribution. Fix the upstream assessment findings first."
                )));
            }
            let kind = match patch.status {
                PatchStatus::Queued => "initial",
                PatchStatus::Amended => "delta",
                PatchStatus::Approved | PatchStatus::Submitted => {
                    if patch
                        .last_approval()
                        .is_some_and(|approval| approval_covers(approval, patch, &token))
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
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = PatchStatus::Dropped;
            patch.conflict = None;
            add_event(patch, "dropped", reason);
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: drop {id}"))?;
        rebuild(repo)?;
        Ok(get_patch(&read_queue_file(repo)?, id)?.clone())
    })
}

pub fn mark_merged(
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
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = PatchStatus::Merged;
            patch.conflict = None;
            patch.merged = Some(PatchMerged {
                via,
                at: stamp(),
                upstream_sha: upstream_sha.map(str::to_string),
            });
            add_event(patch, "merged", format!("Detected via {}", via.as_str()));
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: merged {id}"))?;
        Ok(get_patch(&queue, id)?.clone())
    })
}

pub(super) const STATE_PATCH_PUSH_ATTEMPTS: u32 = 8;

pub(super) fn origin_amended_after_local(local: &Patch, origin: &Patch) -> bool {
    let local_had_amend = local.events.iter().any(|e| e.kind == "amended");
    let origin_amended =
        origin.status == PatchStatus::Amended || origin.events.iter().any(|e| e.kind == "amended");
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
