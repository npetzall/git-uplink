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
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        {
            if queue.is_internal(id) || queue.is_tooling(id) {
                return Err(Error::msg(format!(
                    "{id} is internal-only and cannot be approved for upstream."
                )));
            }
            let patch = get_patch_mut(&mut queue, id)?;
            if patch.assess.as_ref().is_some_and(|p| !p.ok) {
                return Err(Error::msg(format!(
                    "{id} is not ready for contribution. Fix assess-for-upstream findings first."
                )));
            }
            let kind = if patch.status == PatchStatus::Queued {
                "initial"
            } else if patch.status == PatchStatus::Amended {
                "delta"
            } else if patch.status == PatchStatus::Approved
                || patch.status == PatchStatus::Submitted
            {
                return Ok(patch.clone());
            } else {
                return Err(Error::msg(format!(
                    "{id} is {} and cannot be approved for contribution.",
                    patch.status
                )));
            };
            patch.status = PatchStatus::Approved;
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
            });
            add_event(
                patch,
                "approved",
                if kind == "delta" {
                    "IP approved the delta since the previous contribution approval"
                } else {
                    "IP and contribution review passed; patch may leave the enterprise"
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
        for event in src.events {
            if !dest
                .events
                .iter()
                .any(|existing| existing.kind == event.kind && existing.detail == event.detail)
            {
                dest.events.push(event);
            }
        }
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
