use super::*;

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub queue: QueueState,
    pub needs_approval: bool,
    pub pending_sha: Option<String>,
    pub flowed_back: Vec<String>,
    pub foreign_commits: Vec<String>,
    pub report_path: Option<String>,
    pub report: Option<String>,
}

impl SyncResult {
    fn applied(queue: QueueState) -> Self {
        Self {
            queue,
            needs_approval: false,
            pending_sha: None,
            flowed_back: Vec::new(),
            foreign_commits: Vec::new(),
            report_path: None,
            report: None,
        }
    }

    fn applied_with(queue: QueueState, flowed_back: Vec<String>) -> Self {
        Self {
            queue,
            needs_approval: false,
            pending_sha: None,
            flowed_back,
            foreign_commits: Vec::new(),
            report_path: None,
            report: None,
        }
    }
}

pub(super) struct IncomingMatch {
    sha: String,
    patch_id: String,
    via: MergeVia,
}

pub(super) struct IncomingClassification {
    pending_sha: String,
    from_sha: Option<String>,
    flowed_back: Vec<IncomingMatch>,
    foreign: Vec<String>,
}

impl IncomingClassification {
    fn flowed_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for item in &self.flowed_back {
            if !ids.contains(&item.patch_id) {
                ids.push(item.patch_id.clone());
            }
        }
        ids
    }
}

pub(super) fn eligible_for_flow_back(queue: &QueueState, patch: &Patch) -> bool {
    patch.status.is_active() && queue.is_upstream(&patch.id)
}

pub(super) fn commit_stable_patch_id(repo: &Path, sha: &str) -> Result<Option<String>> {
    let shown = git(repo, &["show", "--binary", sha], GitOpts::allow_fail())?;
    if shown.code != 0 || shown.stdout.trim().is_empty() {
        return Ok(None);
    }
    let ident = git(
        repo,
        &["patch-id", "--stable"],
        GitOpts {
            input: Some(shown.stdout.as_bytes()),
            ..GitOpts::default()
        },
    )?;
    Ok(ident
        .stdout
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string))
}

pub(super) fn match_commit_to_patch(
    repo: &Path,
    queue: &QueueState,
    sha: &str,
) -> Result<Option<(String, MergeVia)>> {
    // Patch ids are public, so a trailer alone proves nothing: it counts only
    // when the diff is also ours. Anything else goes to from-upstream review.
    let message = git_ok(repo, &["log", "-1", "--format=%B", sha]).unwrap_or_default();
    let commit_stable = commit_stable_patch_id(repo, sha)?;
    for patch in queue
        .all_patches()
        .filter(|p| eligible_for_flow_back(queue, p))
    {
        let trailer = format!("{}: {}", queue.config.trailer_key, patch.id);
        if message.lines().any(|line| line.trim() == trailer)
            && commit_stable.is_some()
            && patch.patch_id_stable == commit_stable
        {
            return Ok(Some((patch.id.clone(), MergeVia::Trailer)));
        }
    }
    if let Some(stable) = commit_stable {
        for patch in queue
            .all_patches()
            .filter(|p| eligible_for_flow_back(queue, p))
        {
            if patch.patch_id_stable.as_deref() == Some(stable.as_str()) {
                return Ok(Some((patch.id.clone(), MergeVia::PatchId)));
            }
        }
    }
    Ok(None)
}

pub(super) fn classify_incoming(
    repo: &Path,
    queue: &QueueState,
    from_sha: Option<&str>,
    pending_sha: &str,
) -> Result<IncomingClassification> {
    let mut range = Vec::new();
    if let Some(from) = from_sha {
        let list = git_ok(
            repo,
            &["rev-list", "--reverse", &format!("{from}..{pending_sha}")],
        )?;
        range = list
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
    let mut flowed_back = Vec::new();
    let mut foreign = Vec::new();
    for sha in range {
        match match_commit_to_patch(repo, queue, &sha)? {
            Some((patch_id, via)) => flowed_back.push(IncomingMatch { sha, patch_id, via }),
            None => foreign.push(sha),
        }
    }
    Ok(IncomingClassification {
        pending_sha: pending_sha.to_string(),
        from_sha: from_sha.map(str::to_string),
        flowed_back,
        foreign,
    })
}

pub(super) fn write_incoming_packet(
    repo: &Path,
    queue: &QueueState,
    class: &IncomingClassification,
) -> Result<String> {
    let flowed: Vec<IncomingFlowedBack<'_>> = class
        .flowed_back
        .iter()
        .map(|item| {
            let title = queue
                .all_patches()
                .find(|p| p.id == item.patch_id)
                .map(|p| p.title.as_str())
                .unwrap_or("");
            IncomingFlowedBack {
                id: item.patch_id.as_str(),
                via: item.via.as_str(),
                title,
                sha: item.sha.as_str(),
            }
        })
        .collect();
    format_incoming_packet(
        repo,
        class.from_sha.as_deref(),
        &class.pending_sha,
        &flowed,
        &class.foreign,
    )
}

pub fn detect_merged_in_upstream(repo: &Path, queue: &QueueState) -> Result<Vec<String>> {
    let mut merged_ids = Vec::new();
    if !has_ref(repo, "uplink/upstream")? {
        return Ok(merged_ids);
    }
    let live = read_queue_file(repo)?;
    let candidates: Vec<Patch> = queue
        .all_patches()
        .filter_map(|p| live.all_patches().find(|l| l.id == p.id))
        .filter(|p| p.status.is_active() && live.is_upstream(&p.id))
        .cloned()
        .collect();
    let mut upstream_ids: Option<HashMap<String, String>> = None;
    for patch in candidates {
        let grep = format!("{}: {}", live.config.trailer_key, patch.id);
        let trailer = git(
            repo,
            &[
                "log",
                "--fixed-strings",
                "--grep",
                &grep,
                "--format=%H",
                "-1",
                "uplink/upstream",
            ],
            GitOpts::allow_fail(),
        )?;
        if !trailer.stdout.trim().is_empty() {
            mark_merged(
                repo,
                &patch.id,
                MergeVia::Trailer,
                Some(trailer.stdout.trim()),
            )?;
            merged_ids.push(patch.id);
            continue;
        }
        let Some(stable) = &patch.patch_id_stable else {
            continue;
        };
        if upstream_ids.is_none() {
            upstream_ids = Some(upstream_patch_ids(repo)?);
        }
        if let Some(sha) = upstream_ids.as_ref().and_then(|ids| ids.get(stable)) {
            mark_merged(repo, &patch.id, MergeVia::PatchId, Some(sha))?;
            merged_ids.push(patch.id.clone());
        }
    }
    Ok(merged_ids)
}

/// Stable patch id -> newest commit for the last 400 non-merge commits on
/// uplink/upstream, from one `git log -p` piped into one `git patch-id`.
fn upstream_patch_ids(repo: &Path) -> Result<HashMap<String, String>> {
    let log = git_ok(
        repo,
        &[
            "log",
            "--no-merges",
            "--max-count=400",
            "-p",
            "--binary",
            "--format=commit %H",
            "uplink/upstream",
        ],
    )?;
    let mut input = log;
    input.push('\n');
    let ids = git(
        repo,
        &["patch-id", "--stable"],
        GitOpts {
            input: Some(input.as_bytes()),
            ..GitOpts::default()
        },
    )?;
    let mut map = HashMap::new();
    for line in ids.stdout.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(id), Some(sha)) = (parts.next(), parts.next()) {
            map.entry(id.to_string()).or_insert_with(|| sha.to_string());
        }
    }
    Ok(map)
}

pub(super) fn restore_company_branch(repo: &Path, company_branch: &str) -> Result<()> {
    git(
        repo,
        &["checkout", "-f", "--quiet", company_branch],
        GitOpts::default(),
    )?;
    Ok(())
}

pub(super) fn persist_apply_conflict(
    repo: &Path,
    queue: &mut QueueState,
    snapshot: &Path,
    company_branch: &str,
    upstream_ref: &str,
    patch: &Patch,
    files: Vec<String>,
) -> Result<ConflictError> {
    let onto = rev_parse(repo, "HEAD")?;
    let (branch, work) = cut_gated_work(
        repo,
        GateKind::Conflict,
        &patch.id,
        &onto,
        &format!("uplink: conflict applying {}", patch.id),
    )?;

    let message = format!(
        "Patch {} (\"{}\") does not apply onto the current upstream prefix.",
        patch.id, patch.title
    );
    {
        let current = get_patch_mut(queue, &patch.id)?;
        current.status = PatchStatus::Conflict;
        current.conflict = Some(PatchConflict {
            branch: branch.clone(),
            work_branch: Some(work),
            files: files.clone(),
            message: message.clone(),
            onto: Some(onto),
            pr_number: None,
            pr_url: None,
        });
        add_event(
            current,
            "conflict",
            if files.is_empty() {
                "untracked conflict".into()
            } else {
                files.join(", ")
            },
        );
    }
    queue.last_sync = Some(LastSync {
        at: stamp(),
        upstream_sha: rev_parse(repo, upstream_ref)?,
        result: "conflict".into(),
        message: Some(message.clone()),
    });

    restore_company_branch(repo, company_branch)?;
    fs::create_dir_all(repo.join(".uplink/patches"))?;
    copy_dir(&snapshot.join(".uplink"), &repo.join(".uplink"))?;
    write_queue_file(repo, queue)?;
    commit_queue(repo, &format!("uplink: conflict on {}", patch.id))?;
    Ok(ConflictError::new(message, &patch.id, files))
}

pub(super) fn apply_fetched_upstream(repo: &Path, sha: &str) -> Result<QueueState> {
    promote_upstream(repo, sha)?;
    let merged = detect_merged_in_upstream(repo, &read_queue_file(repo)?)?;
    match rebuild(repo) {
        Ok(mut queue) => {
            queue.pending_upstream = None;
            queue.last_sync = Some(LastSync {
                at: stamp(),
                upstream_sha: sha.to_string(),
                result: "ok".into(),
                message: Some(if merged.is_empty() {
                    "Synced with upstream".into()
                } else {
                    format!("Marked merged: {}", merged.join(", "))
                }),
            });
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: sync with upstream")?;
            Ok(queue)
        }
        Err(Error::Conflict(_)) => {
            let mut queue = read_queue_file(repo)?;
            if queue.pending_upstream.is_some() {
                queue.pending_upstream = None;
                write_queue_file(repo, &queue)?;
                commit_queue(repo, "uplink: clear pending upstream")?;
            }
            Ok(queue)
        }
        Err(err) => Err(err),
    }
}

pub fn sync(repo: &Path) -> Result<SyncResult> {
    with_queue_lock(repo, || {
        let fetched = read_queue_file(repo)?;
        let previous = fetched
            .last_sync
            .as_ref()
            .map(|sync| sync.upstream_sha.clone());
        ensure_upstream_ref(repo)?;
        let from_sha = if has_ref(repo, "uplink/upstream")? {
            Some(rev_parse(repo, "uplink/upstream")?)
        } else {
            None
        };
        let sha = fetch_upstream_remote(repo, &fetched)?;
        if previous.as_deref() == Some(sha.as_str()) && from_sha.as_deref() == Some(sha.as_str()) {
            let mut queue = read_queue_file(repo)?;
            queue.pending_upstream = None;
            queue.last_sync = Some(LastSync {
                at: stamp(),
                upstream_sha: sha,
                result: "ok".into(),
                message: Some("Synced with upstream".into()),
            });
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: sync with upstream")?;
            return Ok(SyncResult::applied(queue));
        }
        let class = classify_incoming(repo, &fetched, from_sha.as_deref(), &sha)?;
        if class.foreign.is_empty() {
            let queue = apply_fetched_upstream(repo, &sha)?;
            return Ok(SyncResult::applied_with(queue, class.flowed_ids()));
        }
        let report = write_incoming_packet(repo, &fetched, &class)?;
        let report_path = from_upstream_report_paths().1;
        let abs = repo.join(&report_path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut body = report.clone();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        fs::write(&abs, body)?;
        let mut queue = read_queue_file(repo)?;
        queue.pending_upstream = Some(PendingUpstream {
            sha: class.pending_sha.clone(),
            from_sha: class.from_sha.clone(),
            at: stamp(),
            flowed_back: class.flowed_ids(),
            foreign_commits: class.foreign.clone(),
        });
        queue.last_sync = Some(LastSync {
            at: stamp(),
            upstream_sha: from_sha.clone().unwrap_or_else(|| sha.clone()),
            result: "pending-approval".into(),
            message: Some(format!(
                "Waiting for from-upstream approval of {}",
                class.pending_sha
            )),
        });
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: from-upstream packet")?;
        Ok(SyncResult {
            queue,
            needs_approval: true,
            pending_sha: Some(class.pending_sha.clone()),
            flowed_back: class.flowed_ids(),
            foreign_commits: class.foreign,
            report_path: Some(report_path),
            report: Some(report),
        })
    })
}

pub fn accept_upstream(repo: &Path) -> Result<SyncResult> {
    with_queue_lock(repo, || {
        let queue = read_queue_file(repo)?;
        let pending = queue.pending_upstream.clone().ok_or_else(|| {
            Error::msg("No pending upstream to accept. Run `git uplink sync` first.")
        })?;
        fetch_upstream_remote(repo, &queue)?;
        if !has_ref(repo, &pending.sha)? {
            return Err(Error::msg(format!(
                "Pending upstream {} is not available; fetch public main and try again.",
                pending.sha
            )));
        }
        let flowed_back = pending.flowed_back.clone();
        let queue = apply_fetched_upstream(repo, &pending.sha)?;
        Ok(SyncResult::applied_with(queue, flowed_back))
    })
}
