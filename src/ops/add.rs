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
    /// Branch the company PR merged into. Anything but the company branch
    /// is refused.
    pub base_branch: Option<String>,
    /// Where the verdict of `preflight.sh` on the export tree comes from.
    pub preflight: ScriptVerdict,
}

pub fn add_patch(repo: &Path, opts: AddPatchOpts) -> Result<Patch> {
    let queued = read_queue_file(repo)?;
    // Only the forge knows where a PR merged: by the time import runs, a
    // rebuild may have rewritten the company branch, so git cannot tell.
    let company_branch = &queued.config.internal_branch;
    if let Some(base) = opts.base_branch.as_deref()
        && base != company_branch
    {
        return Err(Error::msg(format!(
            "\"{}\" was merged into {base}, not company {company_branch}; only changes merged into {company_branch} are imported.",
            opts.title
        )));
    }
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
    let raw_message = opts
        .message
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(opts.title.as_str());
    let mut patch = new_queued_patch(opts, depends_on_from_message(raw_message, &opts.depends_on));
    let id = patch.id.clone();
    check_add_dependencies(queue, opts, &patch.depends_on)?;
    add_event(
        &mut patch,
        "created",
        import_event_detail(opts, from_sha, head_sha),
    );
    assess_new_patch(
        repo,
        queue,
        opts,
        &mut patch,
        from_sha,
        head_sha,
        raw_message,
    )?;

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
    if let Err(err) = validate_candidate(repo, queue, opts, &patch, &candidate_abs, head_sha) {
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

fn new_queued_patch(opts: &AddPatchOpts, depends_on: Vec<String>) -> Patch {
    let created_at = stamp();
    Patch {
        id: new_patch_id(),
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
        extras: None,
        events: Vec::new(),
        kind: None,
    }
}

/// Every dependency must exist, and upstream-bound patches may not depend on
/// internal-only or tooling patches.
fn check_add_dependencies(
    queue: &QueueState,
    opts: &AddPatchOpts,
    depends_on: &[String],
) -> Result<()> {
    for dep_id in depends_on {
        get_patch(queue, dep_id)?;
        if cannot_depend_on(queue, opts.internal_only, dep_id) {
            return Err(Error::msg(format!(
                "Upstream-bound patch \"{}\" cannot depend on internal-only patch {dep_id}.",
                opts.title
            )));
        }
    }
    Ok(())
}

fn import_event_detail(opts: &AddPatchOpts, from_sha: &str, head_sha: &str) -> String {
    let destination = if opts.internal_only {
        "internal"
    } else {
        "upstream"
    };
    if let Some(pr) = opts.internal_pr_number {
        format!("Internally approved via PR #{pr}; imported as {destination}")
    } else {
        format!(
            "Imported from {}..{} as {destination}",
            &from_sha[..from_sha.len().min(8)],
            &head_sha[..head_sha.len().min(8)]
        )
    }
}

/// Records the assess report and commit message. Upstream-bound patches must pass.
fn assess_new_patch(
    repo: &Path,
    queue: &QueueState,
    opts: &AddPatchOpts,
    patch: &mut Patch,
    from_sha: &str,
    head_sha: &str,
    raw_message: &str,
) -> Result<()> {
    let intent = PatchIntent::from_internal_only(opts.internal_only);
    let report = assess_from_message(
        repo,
        queue,
        from_sha,
        head_sha,
        raw_message,
        Some(&opts.title),
        intent,
    )?;
    patch.commit_message = report.commit_message.clone();
    if !opts.internal_only {
        assert_assess_ok(&report, &opts.title)?;
    }
    patch.assess = Some(report);
    Ok(())
}

/// Export preflight for upstream-bound patches, then (unless an explicit
/// --from/--head range was given) that the change is already on company main.
fn validate_candidate(
    repo: &Path,
    queue: &QueueState,
    opts: &AddPatchOpts,
    patch: &Patch,
    candidate_abs: &Path,
    head_sha: &str,
) -> Result<()> {
    if !opts.internal_only {
        export_preflight(repo, queue, patch, candidate_abs, None, &opts.preflight)?;
    }
    let explicit_range = opts.from_ref.is_some() && opts.head_ref.is_some();
    if !explicit_range {
        assert_change_already_on_company(repo, queue, candidate_abs, &opts.title, head_sha)?;
    }
    Ok(())
}
