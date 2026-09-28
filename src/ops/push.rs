use super::*;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetResult {
    pub internal_branch: String,
    pub internal_sha: String,
    pub state_sha: String,
    pub upstream_sha: String,
}

pub type RefreshResult = ResetResult;

/// Fetch origin tracking refs for company main, `uplink/state`, and `uplink/upstream`.
/// Does not move local branches or restore `.uplink/`.
pub fn refresh_from_origin(repo: &Path) -> Result<RefreshResult> {
    let state_sha = fetch_tracking_sha(repo, COMPANY_REMOTE, STATE_BRANCH)?;
    let upstream_sha = fetch_tracking_sha(repo, COMPANY_REMOTE, UPSTREAM_REF)?;
    let queue = queue_at(repo, &format!("{COMPANY_REMOTE}/{STATE_BRANCH}"))?;
    let internal_branch = queue.config.internal_branch.clone();
    let internal_sha = fetch_tracking_sha(repo, COMPANY_REMOTE, &internal_branch)?;
    Ok(RefreshResult {
        internal_branch,
        internal_sha,
        state_sha,
        upstream_sha,
    })
}

/// Fetch origin and hard-reset company main, `uplink/state`, and `uplink/upstream`.
/// Leaves HEAD on the configured internal branch with `.uplink/` restored from origin.
pub fn reset_from_origin(repo: &Path) -> Result<ResetResult> {
    let fetched = refresh_from_origin(repo)?;
    refresh_company_branch(repo, COMPANY_REMOTE, &fetched.internal_branch)?;
    apply_state_sha(repo, STATE_BRANCH, &fetched.state_sha)?;
    point_branch_at(repo, UPSTREAM_REF, &fetched.upstream_sha)?;
    Ok(fetched)
}

#[derive(Debug, Clone, Default)]
pub struct PushOpts {
    pub push_remote: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PushResult {
    pub action: String,
    pub remote: String,
    pub branch: String,
    pub sha: String,
}

pub fn push_queue(repo: &Path, opts: PushOpts) -> Result<PushResult> {
    with_queue_lock(repo, || {
        let remote = opts
            .push_remote
            .clone()
            .unwrap_or_else(|| COMPANY_REMOTE.to_string());
        let mut last_error = None;
        for attempt in 0..8 {
            match push_queue_once(repo, &remote) {
                Ok(result) => return Ok(result),
                Err(err) => {
                    let retry = is_push_lease_rejected(&err) && attempt + 1 < 8;
                    if !retry {
                        return Err(err);
                    }
                    last_error = Some(err);
                    thread::sleep(Duration::from_millis(40 * 2u64.pow(attempt as u32)));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| Error::msg("push failed")))
    })
}

pub(super) fn push_queue_once(repo: &Path, remote: &str) -> Result<PushResult> {
    let queue = read_queue_file(repo)?;
    let branch = queue.config.state_branch.clone();
    if !has_ref(repo, &branch)? {
        return Err(Error::msg(
            "uplink/state is missing; run `git uplink init` before pushing",
        ));
    }
    let local_sha = rev_parse(repo, &branch)?;
    let Some(remote_sha) = fetch_state_tracking(repo, remote, &branch)? else {
        push_state_branch(repo, remote, &branch)?;
        return Ok(PushResult {
            action: "pushed".into(),
            remote: remote.into(),
            branch,
            sha: rev_parse(repo, &state_branch(repo))?,
        });
    };

    if local_sha == remote_sha {
        return Ok(PushResult {
            action: "up-to-date".into(),
            remote: remote.into(),
            branch,
            sha: local_sha,
        });
    }

    if is_ancestor(repo, &remote_sha, &local_sha)? {
        push_state_branch(repo, remote, &branch)?;
        return Ok(PushResult {
            action: "pushed".into(),
            remote: remote.into(),
            branch,
            sha: local_sha,
        });
    }

    if is_ancestor(repo, &local_sha, &remote_sha)? {
        set_state_branch(repo, &branch, &remote_sha)?;
        return Ok(PushResult {
            action: "fast-forwarded".into(),
            remote: remote.into(),
            branch,
            sha: remote_sha,
        });
    }

    if merge_base(repo, &local_sha, &remote_sha)?.is_none() {
        return Err(Error::msg(format!(
            "{branch} has unrelated histories with {remote}; cannot restack"
        )));
    }

    let action = restack_local_patches(repo, &branch, &local_sha, &remote_sha)?;
    push_state_branch(repo, remote, &branch)?;
    Ok(PushResult {
        action,
        remote: remote.into(),
        branch,
        sha: rev_parse(repo, &state_branch(repo))?,
    })
}

pub(super) fn restack_local_patches(
    repo: &Path,
    branch: &str,
    local_sha: &str,
    remote_sha: &str,
) -> Result<String> {
    let local_queue = queue_at(repo, local_sha)?;
    let remote_queue = queue_at(repo, remote_sha)?;
    let remote_ids: HashSet<&str> = remote_queue.all_patches().map(|p| p.id.as_str()).collect();
    let remote_prs: HashSet<u64> = remote_queue
        .all_patches()
        .filter_map(|p| p.source.internal_pr_number)
        .collect();
    let should_carry = |patch: &Patch| {
        if remote_ids.contains(patch.id.as_str()) {
            return false;
        }
        if let Some(pr) = patch.source.internal_pr_number
            && remote_prs.contains(&pr)
        {
            return false;
        }
        true
    };
    let carry_tooling = local_queue
        .tooling
        .filter(|p| remote_queue.tooling.is_none() && should_carry(p));
    let carry_upstream: Vec<Patch> = local_queue
        .upstream
        .iter()
        .filter(|p| should_carry(p))
        .cloned()
        .collect();
    let carry_internal: Vec<Patch> = local_queue
        .internal
        .iter()
        .filter(|p| should_carry(p))
        .cloned()
        .collect();
    let carry: Vec<Patch> = carry_tooling
        .iter()
        .cloned()
        .chain(carry_upstream.iter().cloned())
        .chain(carry_internal.iter().cloned())
        .collect();

    if carry.is_empty() {
        set_state_branch(repo, branch, remote_sha)?;
        return Ok("fast-forwarded".into());
    }

    let mut paths = Vec::new();
    for patch in &carry {
        let path = patch_path(&patch.id)?.to_string_lossy().into_owned();
        if !path_exists_at(repo, local_sha, &path)? {
            return Err(Error::msg(format!(
                "local-only patch {} has no patch file on {branch}",
                patch.id
            )));
        }
        paths.push(path);
    }

    let outcome = (|| -> Result<()> {
        set_state_branch(repo, branch, remote_sha)?;
        restore_paths_from(repo, local_sha, &paths)?;
        let mut queue = read_queue_file(repo)?;
        if queue.tooling.is_none() {
            queue.tooling = carry_tooling.clone();
        }
        queue.upstream.extend(carry_upstream);
        queue.internal.extend(carry_internal);
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: restack onto origin")?;
        Ok(())
    })();
    if let Err(err) = outcome {
        let _ = set_state_branch(repo, branch, local_sha);
        return Err(err);
    }
    Ok("restacked".into())
}

pub(super) fn assert_change_already_on_company(
    repo: &Path,
    queue: &QueueState,
    patch_file: &Path,
    title: &str,
    head_sha: &str,
) -> Result<()> {
    ensure_upstream_ref(repo)?;
    if has_ref(repo, "uplink/upstream")?
        && patch_already_applied_on(repo, "uplink/upstream", patch_file)?
    {
        return Ok(());
    }
    let branch = queue.config.internal_branch.as_str();
    if patch_already_applied_on(repo, branch, patch_file)? {
        return Ok(());
    }
    if is_ancestor(repo, head_sha, branch)? {
        return Ok(());
    }
    for sha in branch_history_shas(repo, branch)? {
        if sha == head_sha
            || is_ancestor(repo, head_sha, &sha).unwrap_or(false)
            || patch_already_applied_on(repo, &sha, patch_file).unwrap_or(false)
        {
            return Ok(());
        }
    }
    Err(Error::msg(format!(
        "\"{title}\" is not on company {branch} yet. Merge the internal PR first, then import.",
    )))
}

pub(super) fn git_path(repo: &Path, spec: &str) -> Result<PathBuf> {
    let rel = git_ok(repo, &["rev-parse", "--git-path", spec])?;
    let path = PathBuf::from(&rel);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo.join(path))
    }
}

pub(super) fn branch_history_shas(repo: &Path, branch: &str) -> Result<Vec<String>> {
    let mut shas = Vec::new();
    let mut seen = HashSet::new();
    let path = git_path(repo, &format!("logs/refs/heads/{branch}"))?;
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(_) => return Ok(shas),
    };
    for line in body.lines().rev() {
        let mut parts = line.split_whitespace();
        let old = parts.next().unwrap_or("");
        let new = parts.next().unwrap_or("");
        for candidate in [new, old] {
            if !looks_like_sha(candidate) {
                continue;
            }
            if seen.insert(candidate.to_string()) {
                shas.push(candidate.to_string());
            }
        }
        for token in line.split_whitespace() {
            if looks_like_sha(token) && seen.insert(token.to_string()) {
                shas.push(token.to_string());
            }
        }
    }
    Ok(shas)
}

pub(super) fn looks_like_sha(value: &str) -> bool {
    value.len() == 40
        && value.chars().all(|c| c.is_ascii_hexdigit())
        && !value.chars().all(|c| c == '0')
}

pub(super) fn mark_empty_if_already_upstream(repo: &Path, id: &str) -> Result<()> {
    ensure_upstream_ref(repo)?;
    let mut queue = read_queue_file(repo)?;
    if !queue.is_upstream(id) || !has_ref(repo, "uplink/upstream")? {
        return Ok(());
    }
    let patch_file = repo.join(patch_path(id)?);
    if !patch_already_applied_on(repo, "uplink/upstream", &patch_file)? {
        return Ok(());
    }
    let company_branch = queue.config.internal_branch.clone();
    let current = get_patch_mut(&mut queue, id)?;
    current.status = PatchStatus::Merged;
    current.merged = Some(PatchMerged {
        via: MergeVia::EmptyRebase,
        at: stamp(),
        upstream_sha: Some(rev_parse(repo, "uplink/upstream")?),
    });
    add_event(
        current,
        "merged",
        "Became empty on import; treating as already present upstream",
    );
    write_queue_file(repo, &queue)?;
    commit_queue(repo, &format!("uplink: empty apply {id}"))?;
    git(
        repo,
        &["checkout", "-f", "--quiet", &company_branch],
        GitOpts::default(),
    )?;
    Ok(())
}
