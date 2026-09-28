use super::*;

#[derive(Default)]
pub struct AddPatchOpts {
    pub title: String,
    pub message: Option<String>,
    pub internal_only: bool,
    pub author: Option<String>,
    pub depends_on: Vec<String>,
    pub from_ref: Option<String>,
    pub head_ref: Option<String>,
    pub note: Option<String>,
    pub internal_pr_number: Option<u64>,
    pub internal_pr_url: Option<String>,
    pub preflight_command: Option<String>,
}

pub fn add_patch(repo: &Path, opts: AddPatchOpts) -> Result<Patch> {
    let queued = read_queue_file(repo)?;
    let head_ref = opts.head_ref.as_deref().unwrap_or("HEAD");
    let from_ref = opts
        .from_ref
        .as_deref()
        .unwrap_or(&queued.config.internal_branch);
    let shas = ensure_revs(repo, &[from_ref, head_ref])?;
    let from_sha = shas[0].clone();
    let head_sha = shas[1].clone();

    with_queue_lock(repo, || {
        add_patch_attempt(repo, &opts, &from_sha, &head_sha)
    })
}

pub(super) fn add_patch_attempt(
    repo: &Path,
    opts: &AddPatchOpts,
    from_sha: &str,
    head_sha: &str,
) -> Result<Patch> {
    let mut queue = read_queue_file(repo)?;
    if let Some(pr) = opts.internal_pr_number
        && let Some(existing) = queue
            .all_patches()
            .find(|p| p.source.internal_pr_number == Some(pr))
    {
        return Ok(existing.clone());
    }
    add_patch_once(repo, &mut queue, opts, from_sha, head_sha)
}

pub(super) fn add_patch_once(
    repo: &Path,
    queue: &mut QueueState,
    opts: &AddPatchOpts,
    from_sha: &str,
    head_sha: &str,
) -> Result<Patch> {
    let destination = if opts.internal_only {
        "internal"
    } else {
        "upstream"
    };
    let id = new_patch_id();
    let created_at = stamp();
    let raw_message = opts
        .message
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(opts.title.as_str());
    let depends_on = depends_on_from_message(raw_message, &opts.depends_on);
    let mut patch = Patch {
        id: id.clone(),
        title: opts.title.clone(),
        commit_message: String::new(),
        status: PatchStatus::Queued,
        depends_on,
        created_at: created_at.clone(),
        updated_at: created_at,
        patch_id_stable: None,
        source: PatchSource {
            author: opts.author.clone(),
            note: opts.note.clone(),
            internal_pr_number: opts.internal_pr_number,
            internal_pr_url: opts.internal_pr_url.clone(),
        },
        assess: None,
        upstream: None,
        merged: None,
        conflict: None,
        approvals: Vec::new(),
        events: Vec::new(),
        kind: None,
    };

    for dep_id in &patch.depends_on {
        get_patch(queue, dep_id)?;
        if cannot_depend_on(queue, opts.internal_only, dep_id) {
            return Err(Error::msg(format!(
                "Upstream-bound patch \"{}\" cannot depend on internal-only patch {dep_id}.",
                opts.title
            )));
        }
    }

    add_event(
        &mut patch,
        "created",
        if let Some(pr) = opts.internal_pr_number {
            format!("Internally approved via PR #{pr}; imported as {destination}")
        } else {
            format!(
                "Imported from {}..{} as {destination}",
                &from_sha[..from_sha.len().min(8)],
                &head_sha[..head_sha.len().min(8)]
            )
        },
    );
    patch.assess = Some(assess_from_message(
        repo,
        queue,
        from_sha,
        head_sha,
        raw_message,
        Some(&opts.title),
        if opts.internal_only {
            "internal-only"
        } else {
            "upstream"
        },
    )?);
    if let Some(report) = &patch.assess {
        patch.commit_message = report.commit_message.clone();
        if !opts.internal_only {
            assert_assess_ok(report, &opts.title)?;
        }
    }

    let message = company_commit_message(&patch);
    write_product_patch(repo, &id, from_sha, &message, head_sha)?;
    let rel = patch_path(&id)?.to_string_lossy().into_owned();
    patch.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
    if let Some(duplicate) = queue.all_patches().find(|item| {
        item.patch_id_stable.is_some() && item.patch_id_stable == patch.patch_id_stable
    }) {
        return Ok(duplicate.clone());
    }

    let candidate_abs = repo.join(patch_path(&id)?);
    if !opts.internal_only {
        let preflight = assert_export_preflight(
            repo,
            queue,
            &patch,
            &candidate_abs,
            opts.preflight_command.clone().map(Some),
        );
        if let Err(err) = preflight {
            let _ = fs::remove_file(&candidate_abs);
            return Err(err);
        }
    }

    let explicit_range = opts.from_ref.is_some() && opts.head_ref.is_some();
    if !explicit_range
        && let Err(err) =
            assert_change_already_on_company(repo, queue, &candidate_abs, &opts.title, head_sha)
    {
        let _ = fs::remove_file(&candidate_abs);
        return Err(err);
    }

    let rebuild = !opts.internal_only && queue.internal.iter().any(is_active);
    queue.push_patch(patch, opts.internal_only);
    write_queue_file(repo, queue)?;
    commit_queue(repo, &format!("uplink: add {id} {}", opts.title))?;
    if rebuild {
        rebuild_once(repo)?;
    } else {
        mark_empty_if_already_upstream(repo, &id)?;
    }
    Ok(get_patch(&read_queue_file(repo)?, &id)?.clone())
}
