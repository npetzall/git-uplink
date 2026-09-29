use super::*;

pub struct StatusSnapshot {
    pub queue: QueueState,
    pub company_head: String,
    pub upstream_head: Option<String>,
    pub product_files: std::collections::BTreeMap<String, String>,
    pub state: StateStatus,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateStatus {
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    pub uncommitted: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub counts: QueueCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooling: Option<Patch>,
    pub upstream: Vec<Patch>,
    pub internal: Vec<Patch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<LastSync>,
    pub state: StateStatus,
}

pub fn status_snapshot(repo: &Path) -> Result<StatusSnapshot> {
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
    let state = state_status(repo)?;
    Ok(StatusSnapshot {
        queue,
        company_head,
        upstream_head,
        product_files,
        state,
    })
}

pub(super) fn state_status(repo: &Path) -> Result<StateStatus> {
    state_status_at(repo, true)
}

pub fn state_status_at(repo: &Path, fetch: bool) -> Result<StateStatus> {
    let branch = STATE_BRANCH.to_string();
    let local = if has_ref(repo, &branch)? {
        Some(rev_parse(repo, &branch)?)
    } else {
        None
    };
    let uncommitted = uplink_uncommitted_paths(repo, &branch)?;
    let remote = if fetch {
        fetch_state_tracking(repo, COMPANY_REMOTE, &branch)?
    } else {
        let tracking = format!("{COMPANY_REMOTE}/{branch}");
        if has_ref(repo, &tracking)? {
            Some(rev_parse(repo, &tracking)?)
        } else {
            None
        }
    };
    let (remote_ref, ahead, behind) = if let Some(remote_sha) = remote.as_deref() {
        let remote_ref = format!("{COMPANY_REMOTE}/{branch}");
        let (ahead, behind) = match local.as_deref() {
            Some(local_sha) => ahead_behind(repo, local_sha, remote_sha)?,
            None => (
                0,
                git_ok(repo, &["rev-list", "--count", remote_sha])?
                    .trim()
                    .parse()
                    .unwrap_or(0),
            ),
        };
        (Some(remote_ref), Some(ahead), Some(behind))
    } else {
        (None, None, None)
    };
    Ok(StateStatus {
        branch,
        local,
        remote,
        remote_ref,
        ahead,
        behind,
        uncommitted,
    })
}

pub fn status_report(snapshot: &StatusSnapshot) -> StatusReport {
    StatusReport {
        counts: summarize_queue(&snapshot.queue),
        tooling: snapshot.queue.tooling.clone(),
        upstream: snapshot.queue.upstream.clone(),
        internal: snapshot.queue.internal.clone(),
        last_sync: snapshot.queue.last_sync.clone(),
        state: snapshot.state.clone(),
    }
}

pub fn format_status_table(snapshot: &StatusSnapshot) -> String {
    let mut out = String::new();
    let state = &snapshot.state;
    let local_short = state.local.as_deref().map(short_sha).unwrap_or("(none)");
    let sync = match (state.ahead, state.behind) {
        (Some(0), Some(0)) => "up to date".to_string(),
        (Some(ahead), Some(behind)) => format!("ahead {ahead}  behind {behind}"),
        _ => "no origin tracking".to_string(),
    };
    let _ = writeln!(out, "{}  {local_short}  {sync}", state.branch);
    if !state.uncommitted.is_empty() {
        let _ = writeln!(out, "uncommitted:");
        for path in &state.uncommitted {
            let _ = writeln!(out, "  {path}");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<12}  {:<10}  {:<14}  title  link",
        "id", "status", "queue"
    );
    for patch in snapshot.queue.all_patches() {
        let layer = crate::queue::layer_label(&snapshot.queue, &patch.id);
        let link = patch
            .upstream
            .as_ref()
            .and_then(|u| u.pr_url.clone())
            .unwrap_or_else(|| layer.to_string());
        let _ = writeln!(
            out,
            "{:<12}  {:<10}  {:<14}  {}  {link}",
            patch.id, patch.status, layer, patch.title
        );
    }
    if let Some(sync) = &snapshot.queue.last_sync {
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

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueCounts {
    pub queued: u32,
    pub approved: u32,
    pub submitted: u32,
    pub amended: u32,
    pub merged: u32,
    pub dropped: u32,
    pub conflict: u32,
    pub tooling: u32,
    pub internal: u32,
    pub internal_only: u32,
}

pub fn summarize_queue(queue: &QueueState) -> QueueCounts {
    let mut counts = QueueCounts {
        queued: 0,
        approved: 0,
        submitted: 0,
        amended: 0,
        merged: 0,
        dropped: 0,
        conflict: 0,
        tooling: 0,
        internal: 0,
        internal_only: 0,
    };
    for patch in queue.all_patches() {
        match patch.status {
            PatchStatus::Queued => counts.queued += 1,
            PatchStatus::Approved => counts.approved += 1,
            PatchStatus::Submitted => counts.submitted += 1,
            PatchStatus::Amended => counts.amended += 1,
            PatchStatus::Merged => counts.merged += 1,
            PatchStatus::Dropped => counts.dropped += 1,
            PatchStatus::Conflict => counts.conflict += 1,
        }
        if queue.is_tooling(&patch.id) && patch.status != PatchStatus::Dropped {
            counts.tooling += 1;
        }
        if queue.is_internal(&patch.id) && patch.status != PatchStatus::Dropped {
            counts.internal += 1;
            counts.internal_only += 1;
        }
    }
    counts
}
