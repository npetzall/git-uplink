use super::*;

/// The queue as origin has it: `origin/uplink/state` as last fetched. Read
/// with git; the `.uplink/` folder and local branches are never read.
#[derive(Debug)]
pub struct OriginStatus {
    /// `origin/uplink/state`.
    pub state_ref: String,
    pub state_sha: String,
    pub queue: QueueState,
    /// The company branch on origin, as last fetched.
    pub company_head: Option<String>,
    /// `origin/uplink/upstream`, as last fetched.
    pub upstream_head: Option<String>,
    /// Approved or submitted patches that changed since their last approval.
    pub stale_approvals: Vec<String>,
}

/// The working queue in `.uplink/` and the product files at HEAD, for
/// inspecting what a command did in this clone. Commands show the queue
/// with [`origin_status`] instead.
pub struct WorkingSnapshot {
    pub queue: QueueState,
    pub company_head: String,
    pub upstream_head: Option<String>,
    pub product_files: std::collections::BTreeMap<String, String>,
    /// Approved or submitted patches that changed since their last approval.
    pub stale_approvals: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub state_ref: String,
    pub state_sha: String,
    pub counts: QueueCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooling: Option<Patch>,
    pub upstream: Vec<Patch>,
    pub internal: Vec<Patch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<LastSync>,
    /// Approved or submitted patches that changed since their last
    /// approval; `submit` refuses them until they are approved again.
    pub stale_approvals: Vec<String>,
}

pub fn working_snapshot(repo: &Path) -> Result<WorkingSnapshot> {
    let queue = read_queue_file(repo)?;
    let company_head = rev_parse(repo, &queue.config.internal_branch)?;
    let upstream_head = if has_ref(repo, "uplink/upstream")? {
        Some(rev_parse(repo, "uplink/upstream")?)
    } else {
        None
    };
    let listing = git_ok(repo, &["ls-tree", "-r", "--name-only", "HEAD"])?;
    let mut product_files = std::collections::BTreeMap::new();
    for file in listing
        .lines()
        .filter(|n| !n.is_empty() && !n.starts_with(".uplink/"))
    {
        product_files.insert(
            file.to_string(),
            git_ok(repo, &["show", &format!("HEAD:{file}")])?,
        );
    }
    let stale_approvals = stale_approvals(repo, &queue);
    Ok(WorkingSnapshot {
        stale_approvals,
        queue,
        company_head,
        upstream_head,
        product_files,
    })
}

fn optional_rev(repo: &Path, git_ref: &str) -> Result<Option<String>> {
    if has_ref(repo, git_ref)? {
        Ok(Some(rev_parse(repo, git_ref)?))
    } else {
        Ok(None)
    }
}

/// The queue on `origin/uplink/state` as last fetched. Fetches nothing and
/// moves no branch; `refresh_from_origin` fetches.
pub fn origin_status(repo: &Path) -> Result<OriginStatus> {
    let state_ref = format!("{COMPANY_REMOTE}/{STATE_BRANCH}");
    let Some(state_sha) = optional_rev(repo, &state_ref)? else {
        return Err(Error::msg(format!(
            "no {state_ref}; run `git uplink refresh` to fetch it"
        )));
    };
    let queue = queue_at(repo, &state_ref)?;
    let company_head = optional_rev(
        repo,
        &format!("{COMPANY_REMOTE}/{}", queue.config.internal_branch),
    )?;
    let upstream_head = optional_rev(repo, &format!("{COMPANY_REMOTE}/{UPSTREAM_REF}"))?;
    let stale_approvals = stale_approvals_at(repo, &state_ref, &queue);
    Ok(OriginStatus {
        state_ref,
        state_sha,
        queue,
        company_head,
        upstream_head,
        stale_approvals,
    })
}

pub fn status_report(status: &OriginStatus) -> StatusReport {
    StatusReport {
        state_ref: status.state_ref.clone(),
        state_sha: status.state_sha.clone(),
        counts: summarize_queue(&status.queue),
        tooling: status.queue.tooling.clone(),
        upstream: status.queue.upstream.clone(),
        internal: status.queue.internal.clone(),
        last_sync: status.queue.last_sync.clone(),
        stale_approvals: status.stale_approvals.clone(),
    }
}

pub fn format_status_table(status: &OriginStatus) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  {}  (as last fetched)",
        status.state_ref,
        short_sha(&status.state_sha)
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<12}  {:<10}  {:<14}  title  link",
        "id", "status", "queue"
    );
    for patch in status.queue.all_patches() {
        let layer = crate::queue::layer_label(&status.queue, &patch.id);
        let link = patch
            .upstream
            .as_ref()
            .and_then(|u| u.pr_url.clone())
            .unwrap_or_else(|| layer.to_string());
        let _ = writeln!(
            out,
            "{:<12}  {:<10}  {:<14}  {}  {link}",
            patch.id,
            patch.status,
            layer,
            patch.subject()
        );
    }
    if !status.stale_approvals.is_empty() {
        let _ = writeln!(
            out,
            "needs approval before submit (content changed since the last approval): {}",
            status.stale_approvals.join(", ")
        );
    }
    if let Some(sync) = &status.queue.last_sync {
        let _ = writeln!(out, "last sync: {} @ {}", sync.result, sync.at);
        if let Some(msg) = &sync.message {
            let _ = writeln!(out, "{msg}");
        }
    }
    out
}

pub(super) fn short_sha(sha: &str) -> &str {
    match sha.char_indices().nth(7) {
        Some((i, _)) => &sha[..i],
        None => sha,
    }
}

/// Queue counters. The first five count upstream-bound patches only.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueCounts {
    pub queued: u32,
    pub waiting: u32,
    pub submitted: u32,
    pub merged: u32,
    pub rejected: u32,
    /// Internal-only patches that are `queued` (shown as active).
    pub internal: u32,
    /// `queued` patches in every layer: tooling, upstream and internal.
    pub active: u32,
}

pub fn summarize_queue(queue: &QueueState) -> QueueCounts {
    let mut counts = QueueCounts {
        queued: 0,
        waiting: 0,
        submitted: 0,
        merged: 0,
        rejected: 0,
        internal: 0,
        active: 0,
    };
    for patch in queue.all_patches() {
        let queued = patch.status == PatchStatus::Queued;
        if queued {
            counts.active += 1;
        }
        if queue.is_internal(&patch.id) {
            if queued {
                counts.internal += 1;
            }
            continue;
        }
        if !queue.is_upstream(&patch.id) {
            continue;
        }
        match patch.status {
            PatchStatus::Queued => counts.queued += 1,
            PatchStatus::Waiting => counts.waiting += 1,
            PatchStatus::Submitted => counts.submitted += 1,
            PatchStatus::Merged => counts.merged += 1,
            PatchStatus::Rejected => counts.rejected += 1,
            PatchStatus::Approved | PatchStatus::Dropped | PatchStatus::Conflict => {}
        }
    }
    counts
}
