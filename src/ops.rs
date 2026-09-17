use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::error::{ConflictError, Error, Result};
use crate::git::{GitOpts, configure_repo, git, git_ok};
use crate::lock::{is_push_lease_rejected, with_queue_lock};
use crate::preflight::assert_export_preflight;
use crate::prepare::{
    assert_prepare_ok, company_commit_message, depends_on_from_message, prepare_from_message,
};
use crate::queue::{
    add_event, empty_queue, get_patch, get_patch_mut, read_queue as read_queue_file,
    topological_active, write_queue as write_queue_file,
};
use crate::repo::{
    COMPANY_REMOTE, apply_patch_file, commit_queue, conflicted_files, copy_dir,
    ensure_configured_remotes, ensure_state_worktree, ensure_upstream_ref, fetch_origin_state,
    fetch_upstream, has_ref, new_patch_id, patch_already_applied_on, push_company_branch,
    push_state_branch, refresh_company_branch, refresh_state_branch, refresh_upstream_ref,
    rev_parse, stable_patch_id, stable_patch_id_from_contents, stamp, state_branch, state_exists,
    try_fetch_origin_state, write_product_patch,
};
use crate::types::{
    LastSync, MergeVia, Patch, PatchApproval, PatchConflict, PatchMerged, PatchSource,
    PatchUpstream, QUEUE_PATH, QueueConfig, QueueState,
};

pub fn read_queue(repo: &Path) -> Result<QueueState> {
    read_queue_file(repo)
}

pub fn write_queue(repo: &Path, queue: &QueueState) -> Result<()> {
    write_queue_file(repo, queue)
}

fn snapshot_uplink(repo: &Path) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("uplink-{}", uuid::Uuid::new_v4()));
    copy_dir(&repo.join(".uplink"), &dir.join(".uplink"))?;
    Ok(dir)
}

#[derive(Debug, Clone, Default)]
pub struct InitOpts {
    pub upstream_url: Option<String>,
    pub contrib_url: Option<String>,
    pub upstream_remote_name: Option<String>,
    pub upstream_branch: Option<String>,
    pub contrib_remote_name: Option<String>,
    pub internal_branch: Option<String>,
}

impl InitOpts {
    pub fn has_args(&self) -> bool {
        self.upstream_url.is_some()
            || self.contrib_url.is_some()
            || self.upstream_remote_name.is_some()
            || self.upstream_branch.is_some()
            || self.contrib_remote_name.is_some()
            || self.internal_branch.is_some()
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

fn config_from_opts(opts: &InitOpts) -> QueueConfig {
    let mut config = QueueConfig::default();
    if let Some(name) = &opts.upstream_remote_name {
        config.upstream_remote = name.clone();
    }
    if let Some(branch) = &opts.upstream_branch {
        config.upstream_branch = branch.clone();
    }
    if let Some(name) = &opts.contrib_remote_name {
        config.contrib_remote = name.clone();
    }
    if let Some(branch) = &opts.internal_branch {
        config.internal_branch = branch.clone();
    }
    config.upstream_url = nonempty(opts.upstream_url.clone());
    config.contrib_url = nonempty(opts.contrib_url.clone());
    config
}

/// Create or hydrate an uplink queue. No CLI args fetches `origin` `uplink/state`
/// and reconstitutes remotes from stored URLs. Args create the queue when state
/// is missing, or sanity-check an existing queue.
pub fn init(repo: &Path, opts: InitOpts) -> Result<QueueState> {
    configure_repo(repo)?;
    if !opts.has_args() {
        return hydrate_from_origin(repo);
    }
    try_fetch_origin_state(repo)?;
    if state_exists(repo)? {
        return init_existing(repo, &opts);
    }
    init_repo(repo, config_from_opts(&opts))
}

fn hydrate_from_origin(repo: &Path) -> Result<QueueState> {
    fetch_origin_state(repo)?;
    let queue = read_queue_file(repo)?;
    require_stored_urls(&queue.config)?;
    ensure_configured_remotes(repo, &queue.config)?;
    refresh_upstream_ref(repo, COMPANY_REMOTE)?;
    Ok(queue)
}

fn require_stored_urls(config: &QueueConfig) -> Result<()> {
    let missing_upstream = config.upstream_url.as_deref().is_none_or(|s| s.is_empty());
    let missing_contrib = config.contrib_url.as_deref().is_none_or(|s| s.is_empty());
    if missing_upstream || missing_contrib {
        return Err(Error::msg(
            "uplink/state is missing upstreamUrl or contribUrl. \
Re-run `git uplink init --upstream <url> --contrib <url>` to record remotes.",
        ));
    }
    Ok(())
}

fn check_name(out: &mut Vec<String>, field: &str, requested: Option<&str>, stored: &str) {
    if let Some(requested) = requested {
        if requested != stored {
            out.push(format!(
                "  {field}: stored \"{stored}\", requested \"{requested}\""
            ));
        }
    }
}

fn init_existing(repo: &Path, opts: &InitOpts) -> Result<QueueState> {
    ensure_state_worktree(repo)?;
    let mut queue = read_queue_file(repo)?;
    let mut mismatches = Vec::new();
    check_name(
        &mut mismatches,
        "upstreamRemote",
        opts.upstream_remote_name.as_deref(),
        &queue.config.upstream_remote,
    );
    check_name(
        &mut mismatches,
        "upstreamBranch",
        opts.upstream_branch.as_deref(),
        &queue.config.upstream_branch,
    );
    check_name(
        &mut mismatches,
        "contribRemote",
        opts.contrib_remote_name.as_deref(),
        &queue.config.contrib_remote,
    );
    check_name(
        &mut mismatches,
        "internalBranch",
        opts.internal_branch.as_deref(),
        &queue.config.internal_branch,
    );
    if !mismatches.is_empty() {
        return Err(Error::msg(format!(
            "Cannot change uplink remote or branch names on an existing queue:\n{}",
            mismatches.join("\n")
        )));
    }

    let mut urls_changed = false;
    if let Some(url) = nonempty(opts.upstream_url.clone()) {
        if queue.config.upstream_url.as_deref() != Some(url.as_str()) {
            queue.config.upstream_url = Some(url);
            urls_changed = true;
        }
    }
    if let Some(url) = nonempty(opts.contrib_url.clone()) {
        if queue.config.contrib_url.as_deref() != Some(url.as_str()) {
            queue.config.contrib_url = Some(url);
            urls_changed = true;
        }
    }
    if urls_changed {
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: update remote urls")?;
    }
    ensure_configured_remotes(repo, &queue.config)?;
    refresh_upstream_ref(repo, COMPANY_REMOTE)?;
    read_queue_file(repo)
}

pub fn init_repo(repo: &Path, config: QueueConfig) -> Result<QueueState> {
    configure_repo(repo)?;
    crate::repo::ensure_uplink_dirs(repo)?;
    let queue = empty_queue(config);
    write_queue_file(repo, &queue)?;
    commit_queue(repo, "uplink: initialize patch queue")?;
    ensure_state_worktree(repo)?;
    ensure_configured_remotes(repo, &queue.config)?;
    let has_head = git(
        repo,
        &["rev-parse", "--verify", "HEAD"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    let remotes = git_ok(repo, &["remote"]).unwrap_or_default();
    if remotes
        .split('\n')
        .any(|r| r == queue.config.upstream_remote)
    {
        fetch_upstream(repo, &queue)?;
    } else if has_head.code == 0 {
        let parent = git(
            repo,
            &["rev-parse", "--verify", "HEAD~1"],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        )?;
        let target = if parent.code == 0 { "HEAD~1" } else { "HEAD" };
        git(
            repo,
            &["branch", "-f", "uplink/upstream", target],
            GitOpts::default(),
        )?;
    }
    Ok(queue)
}

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
    pub refresh_remote: Option<String>,
    pub push_remote: Option<String>,
}

pub fn add_patch(repo: &Path, opts: AddPatchOpts) -> Result<Patch> {
    let queued = read_queue_file(repo)?;
    let head_sha = rev_parse(repo, opts.head_ref.as_deref().unwrap_or("HEAD"))?;
    let from_sha = rev_parse(
        repo,
        opts.from_ref
            .as_deref()
            .unwrap_or(&queued.config.internal_branch),
    )?;

    with_queue_lock(repo, || {
        let attempts = if opts.push_remote.is_some() || opts.refresh_remote.is_some() {
            8
        } else {
            1
        };
        let refresh_remote = opts
            .refresh_remote
            .clone()
            .or_else(|| opts.push_remote.clone());
        let mut last_error = None;
        for attempt in 0..attempts {
            match add_patch_attempt(repo, &opts, &from_sha, &head_sha, refresh_remote.as_deref()) {
                Ok(patch) => return Ok(patch),
                Err(err) => {
                    let retry = opts.push_remote.is_some()
                        && is_push_lease_rejected(&err)
                        && attempt + 1 < attempts;
                    if !retry {
                        return Err(err);
                    }
                    last_error = Some(err);
                    thread::sleep(Duration::from_millis(40 * 2u64.pow(attempt as u32)));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| Error::msg("add failed")))
    })
}

fn add_patch_attempt(
    repo: &Path,
    opts: &AddPatchOpts,
    from_sha: &str,
    head_sha: &str,
    refresh_remote: Option<&str>,
) -> Result<Patch> {
    if let Some(remote) = refresh_remote {
        let queued = read_queue_file(repo)?;
        refresh_company_branch(repo, remote, &queued.config.internal_branch)?;
        refresh_state_branch(repo, remote, &queued.config.state_branch)?;
        refresh_upstream_ref(repo, remote)?;
    }
    let mut queue = read_queue_file(repo)?;
    let expected_sha = if let Some(remote) = &opts.push_remote {
        Some(rev_parse(
            repo,
            &format!("{remote}/{}", queue.config.internal_branch),
        )?)
    } else {
        None
    };
    if let Some(pr) = opts.internal_pr_number {
        if let Some(existing) = queue
            .patches
            .iter()
            .find(|p| p.source.internal_pr_number == Some(pr))
        {
            let id = existing.id.clone();
            apply_new_patch_on_company(repo, &id, false)?;
            if let (Some(remote), Some(expected)) = (&opts.push_remote, expected_sha.as_ref()) {
                let latest = read_queue_file(repo)?;
                push_state_branch(repo, remote, &latest.config.state_branch)?;
                push_company_branch(repo, remote, &queue.config.internal_branch, expected)?;
            }
            return Ok(get_patch(&read_queue_file(repo)?, &id)?.clone());
        }
    }
    let patch = add_patch_once(repo, &mut queue, opts, from_sha, head_sha)?;
    if let (Some(remote), Some(expected)) = (&opts.push_remote, expected_sha) {
        let latest = read_queue_file(repo)?;
        push_state_branch(repo, remote, &latest.config.state_branch)?;
        push_company_branch(repo, remote, &queue.config.internal_branch, &expected)?;
    }
    Ok(patch)
}

fn add_patch_once(
    repo: &Path,
    queue: &mut QueueState,
    opts: &AddPatchOpts,
    from_sha: &str,
    head_sha: &str,
) -> Result<Patch> {
    let intent = if opts.internal_only {
        "internal-only"
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
        intent: intent.into(),
        status: "queued".into(),
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
        prepare: None,
        upstream: None,
        merged: None,
        conflict: None,
        approvals: Vec::new(),
        events: Vec::new(),
    };

    for dep_id in &patch.depends_on {
        let dep = get_patch(queue, dep_id)?;
        if intent == "upstream" && dep.intent == "internal-only" {
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
            format!("Internally approved via PR #{pr}; imported as {intent}")
        } else {
            format!(
                "Imported from {}..{} as {intent}",
                &from_sha[..from_sha.len().min(8)],
                &head_sha[..head_sha.len().min(8)]
            )
        },
    );
    patch.prepare = Some(prepare_from_message(
        repo,
        queue,
        from_sha,
        head_sha,
        raw_message,
        Some(&opts.title),
        intent,
    )?);
    if let Some(report) = &patch.prepare {
        patch.commit_message = report.commit_message.clone();
        if intent == "upstream" {
            assert_prepare_ok(report, &opts.title)?;
        }
    }

    let message = company_commit_message(&patch);
    write_product_patch(repo, &id, from_sha, &message, head_sha)?;
    patch.patch_id_stable = Some(stable_patch_id(
        repo,
        &format!(".uplink/patches/{id}.patch"),
    )?);
    if let Some(duplicate) = queue.patches.iter().find(|item| {
        item.patch_id_stable.is_some() && item.patch_id_stable == patch.patch_id_stable
    }) {
        return Ok(duplicate.clone());
    }

    let candidate_abs = repo.join(format!(".uplink/patches/{id}.patch"));
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

    queue.patches.push(patch);
    write_queue_file(repo, queue)?;
    commit_queue(repo, &format!("uplink: add {id} {}", opts.title))?;
    apply_new_patch_on_company(repo, &id, true)?;
    Ok(get_patch(&read_queue_file(repo)?, &id)?.clone())
}

fn apply_new_patch_on_company(repo: &Path, id: &str, mark_empty_merged: bool) -> Result<()> {
    ensure_upstream_ref(repo)?;
    let mut queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    let patch = get_patch(&queue, id)?.clone();
    let patch_file = repo.join(format!(".uplink/patches/{id}.patch"));

    if mark_empty_merged
        && patch.intent == "upstream"
        && has_ref(repo, "uplink/upstream")?
        && patch_already_applied_on(repo, "uplink/upstream", &patch_file)?
    {
        let current = get_patch_mut(&mut queue, id)?;
        current.status = "merged".into();
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
        return Ok(());
    }

    let snapshot = snapshot_uplink(repo)?;
    git(
        repo,
        &["checkout", "-f", "--quiet", &company_branch],
        GitOpts::default(),
    )?;
    let result = apply_patch_file(repo, &patch, &patch_file, false);
    let result = match result {
        Ok(value) => value,
        Err(err) => {
            let _ = fs::remove_dir_all(&snapshot);
            return Err(err);
        }
    };
    if result == "conflict" {
        let files = conflicted_files(repo)?;
        let upstream_ref = if has_ref(repo, "uplink/upstream")? {
            "uplink/upstream"
        } else {
            company_branch.as_str()
        };
        let err = persist_apply_conflict(
            repo,
            &mut queue,
            &snapshot,
            &company_branch,
            upstream_ref,
            &patch.id,
            &patch.title,
            files,
        )?;
        let _ = fs::remove_dir_all(&snapshot);
        return Err(Error::Conflict(err));
    }
    let _ = fs::remove_dir_all(&snapshot);
    if result == "empty" {
        return Ok(());
    }
    commit_queue(repo, &format!("uplink: record applied patch {id}"))?;
    Ok(())
}

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
            let patch = get_patch_mut(&mut queue, id)?;
            if patch.intent == "internal-only" {
                return Err(Error::msg(format!(
                    "{id} is internal-only and cannot be approved for upstream."
                )));
            }
            if patch.prepare.as_ref().is_some_and(|p| !p.ok) {
                return Err(Error::msg(format!(
                    "{id} is not ready for contribution. Fix prepare-for-upstream findings first."
                )));
            }
            let kind = if patch.status == "queued" {
                "initial"
            } else if patch.status == "amended" {
                "delta"
            } else if patch.status == "approved" || patch.status == "submitted" {
                return Ok(patch.clone());
            } else {
                return Err(Error::msg(format!(
                    "{id} is {} and cannot be approved for contribution.",
                    patch.status
                )));
            };
            patch.status = "approved".into();
            let sha = match sha {
                Some(value) if !value.is_empty() => value.to_string(),
                _ => rev_parse(repo, &state_branch(repo))?,
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

fn github_run_url_opt() -> Option<String> {
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
            patch.status = "dropped".into();
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
            patch.status = "merged".into();
            patch.conflict = None;
            patch.merged = Some(PatchMerged {
                via: via.clone(),
                at: stamp(),
                upstream_sha: upstream_sha.map(str::to_string),
            });
            add_event(patch, "merged", format!("Detected via {}", via.as_str()));
        }
        write_queue_file(repo, &queue)?;
        Ok(get_patch(&queue, id)?.clone())
    })
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
        let state_branch = queue.config.state_branch.clone();
        let patch = get_patch(&queue, id)?.clone();
        if let Some(existing) = patch.upstream.as_ref() {
            if existing.pr_number == Some(number)
                && existing.pr_url.as_deref() == Some(url)
                && patch.status == "submitted"
            {
                if let Some(remote) = push_remote {
                    push_state_branch(repo, remote, &state_branch)?;
                }
                return Ok(patch);
            }
            if existing.pr_number.is_some()
                && (existing.pr_number != Some(number) || existing.pr_url.as_deref() != Some(url))
            {
                let recorded = existing
                    .pr_url
                    .clone()
                    .unwrap_or_else(|| format!("#{}", existing.pr_number.unwrap()));
                return Err(Error::msg(format!(
                    "{id} is already submitted as {recorded}; will not retarget to {url}"
                )));
            }
        }
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = "submitted".into();
            patch.upstream = Some(PatchUpstream {
                contrib_branch: branch.into(),
                pr_number: Some(number),
                pr_url: Some(url.into()),
                submitted_at: Some(stamp()),
            });
            add_event(patch, "submitted", format!("Upstream PR {url}"));
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: submit {id} as PR {number}"))?;
        if let Some(remote) = push_remote {
            push_state_branch(repo, remote, &state_branch)?;
        }
        Ok(get_patch(&queue, id)?.clone())
    })
}

pub fn record_conflict_issue(
    repo: &Path,
    id: &str,
    number: u64,
    url: &str,
    push_remote: Option<&str>,
) -> Result<Patch> {
    with_queue_lock(repo, || {
        let mut queue = read_queue_file(repo)?;
        let state_branch = queue.config.state_branch.clone();
        let patch = get_patch(&queue, id)?.clone();
        if patch.status != "conflict" {
            return Err(Error::msg(format!("{id} is not in conflict")));
        }
        let Some(conflict) = patch.conflict.clone() else {
            return Err(Error::msg(format!("{id} has no conflict record")));
        };
        if conflict.issue_number == Some(number) && conflict.issue_url.as_deref() == Some(url) {
            if let Some(remote) = push_remote {
                push_state_branch(repo, remote, &state_branch)?;
            }
            return Ok(patch);
        }
        if conflict.issue_number.is_some()
            && (conflict.issue_number != Some(number) || conflict.issue_url.as_deref() != Some(url))
        {
            let recorded = conflict
                .issue_url
                .unwrap_or_else(|| format!("#{}", conflict.issue_number.unwrap()));
            return Err(Error::msg(format!(
                "{id} already has conflict issue {recorded}; will not retarget to {url}"
            )));
        }
        {
            let patch = get_patch_mut(&mut queue, id)?;
            let conflict = patch
                .conflict
                .as_mut()
                .ok_or_else(|| Error::msg(format!("{id} has no conflict record")))?;
            conflict.issue_number = Some(number);
            conflict.issue_url = Some(url.into());
            add_event(patch, "conflict-issue", format!("Conflict issue {url}"));
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: conflict issue {id} #{number}"))?;
        if let Some(remote) = push_remote {
            push_state_branch(repo, remote, &queue.config.state_branch)?;
        }
        Ok(get_patch(&queue, id)?.clone())
    })
}

pub fn detect_merged_in_upstream(repo: &Path, queue: &QueueState) -> Result<Vec<String>> {
    let mut merged_ids = Vec::new();
    if !has_ref(repo, "uplink/upstream")? {
        return Ok(merged_ids);
    }
    let ids: Vec<String> = queue.patches.iter().map(|p| p.id.clone()).collect();
    for id in ids {
        let live = read_queue_file(repo)?;
        let patch = match live.patches.iter().find(|p| p.id == id) {
            Some(p) => p.clone(),
            None => continue,
        };
        if patch.status == "merged" || patch.status == "dropped" || patch.intent == "internal-only"
        {
            continue;
        }
        let grep = format!("Uplink-Patch-Id: {}", patch.id);
        let trailer = git(
            repo,
            &[
                "log",
                "uplink/upstream",
                "--grep",
                &grep,
                "--format=%H",
                "-1",
            ],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
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
        if let Some(stable) = &patch.patch_id_stable {
            let commits = git_ok(
                repo,
                &[
                    "rev-list",
                    "--no-merges",
                    "--max-count=400",
                    "uplink/upstream",
                ],
            )?;
            for sha in commits.lines().filter(|s| !s.is_empty()) {
                let shown = git(
                    repo,
                    &["show", "--binary", sha],
                    GitOpts {
                        allow_fail: true,
                        ..GitOpts::default()
                    },
                )?;
                let ident = git(
                    repo,
                    &["patch-id", "--stable"],
                    GitOpts {
                        input: Some(shown.stdout.as_bytes()),
                        ..GitOpts::default()
                    },
                )?;
                let their_id = ident.stdout.split_whitespace().next().unwrap_or("");
                if !their_id.is_empty() && their_id == stable {
                    mark_merged(repo, &patch.id, MergeVia::PatchId, Some(sha))?;
                    merged_ids.push(patch.id.clone());
                    break;
                }
            }
        }
        let _ = live;
    }
    Ok(merged_ids)
}

fn restore_company_branch(repo: &Path, company_branch: &str) -> Result<()> {
    git(
        repo,
        &["checkout", "-f", "--quiet", company_branch],
        GitOpts::default(),
    )?;
    Ok(())
}

fn persist_apply_conflict(
    repo: &Path,
    queue: &mut QueueState,
    snapshot: &Path,
    company_branch: &str,
    upstream_ref: &str,
    patch_id: &str,
    title: &str,
    files: Vec<String>,
) -> Result<ConflictError> {
    let onto = rev_parse(repo, "HEAD")?;
    let branch = format!("uplink/conflict/{patch_id}");
    git(repo, &["branch", "-f", &branch, "HEAD"], GitOpts::default())?;
    git(
        repo,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
        GitOpts::default(),
    )?;
    git(repo, &["add", "-A"], GitOpts::default())?;
    let staged = git(
        repo,
        &["diff", "--cached", "--quiet"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if staged.code != 0 {
        git(
            repo,
            &[
                "commit",
                "-m",
                &format!("uplink: conflict applying {patch_id}"),
            ],
            GitOpts::default(),
        )?;
    }

    let message =
        format!("Patch {patch_id} (\"{title}\") does not apply onto the current upstream prefix.");
    {
        let current = get_patch_mut(queue, patch_id)?;
        current.status = "conflict".into();
        current.conflict = Some(PatchConflict {
            branch: branch.clone(),
            files: files.clone(),
            message: message.clone(),
            onto: Some(onto),
            issue_number: None,
            issue_url: None,
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
    commit_queue(repo, &format!("uplink: conflict on {patch_id}"))?;
    Ok(ConflictError::new(message, patch_id, files))
}

pub fn rebuild(repo: &Path) -> Result<QueueState> {
    with_queue_lock(repo, || rebuild_once(repo))
}

fn rebuild_once(repo: &Path) -> Result<QueueState> {
    let mut queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    ensure_upstream_ref(repo)?;
    let upstream_ref = if has_ref(repo, "uplink/upstream")? {
        "uplink/upstream"
    } else {
        company_branch.as_str()
    };
    let snapshot = snapshot_uplink(repo)?;
    let outcome = (|| -> Result<QueueState> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", upstream_ref],
            GitOpts::default(),
        )?;
        for patch in topological_active(&queue)? {
            if patch.status == "conflict" {
                restore_company_branch(repo, &company_branch)?;
                return Err(Error::Conflict(ConflictError::new(
                    format!("Queue is blocked on conflict in {}", patch.id),
                    patch.id,
                    patch.conflict.map(|c| c.files).unwrap_or_default(),
                )));
            }
            let patch_file = snapshot
                .join(".uplink/patches")
                .join(format!("{}.patch", patch.id));
            let result = apply_patch_file(repo, &patch, &patch_file, false)?;
            if result == "empty" {
                let current = get_patch_mut(&mut queue, &patch.id)?;
                if current.intent == "upstream" {
                    current.status = "merged".into();
                    current.merged = Some(PatchMerged {
                        via: MergeVia::EmptyRebase,
                        at: stamp(),
                        upstream_sha: Some(rev_parse(repo, upstream_ref)?),
                    });
                    add_event(
                        current,
                        "merged",
                        "Became empty on rebuild; treating as already present upstream",
                    );
                }
                continue;
            }
            if result == "conflict" {
                let files = conflicted_files(repo)?;
                return Err(Error::Conflict(persist_apply_conflict(
                    repo,
                    &mut queue,
                    &snapshot,
                    &company_branch,
                    upstream_ref,
                    &patch.id,
                    &patch.title,
                    files,
                )?));
            }

            let contents = fs::read_to_string(&patch_file)?;
            let current = get_patch_mut(&mut queue, &patch.id)?;
            current.patch_id_stable = Some(stable_patch_id_from_contents(repo, &contents)?);
            if current.status == "conflict" {
                current.status = if current
                    .upstream
                    .as_ref()
                    .and_then(|u| u.pr_number)
                    .is_some()
                {
                    "submitted"
                } else {
                    "queued"
                }
                .into();
            }
            current.conflict = None;
        }

        git(repo, &["add", "-A"], GitOpts::default())?;
        let still = git(
            repo,
            &["diff", "--cached", "--quiet"],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        )?;
        if still.code != 0 {
            git(
                repo,
                &[
                    "commit",
                    "-m",
                    &format!("uplink: rebuild company {company_branch} onto upstream"),
                ],
                GitOpts::default(),
            )?;
        }
        git(
            repo,
            &["branch", "-f", &company_branch, "HEAD"],
            GitOpts::default(),
        )?;
        git(
            repo,
            &["checkout", "-f", "--quiet", &company_branch],
            GitOpts::default(),
        )?;
        fs::create_dir_all(repo.join(".uplink/patches"))?;
        copy_dir(&snapshot.join(".uplink"), &repo.join(".uplink"))?;
        queue.last_sync = Some(LastSync {
            at: stamp(),
            upstream_sha: rev_parse(repo, upstream_ref)?,
            result: "ok".into(),
            message: Some("Rebuild completed".into()),
        });
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: record rebuild status")?;
        Ok(queue)
    })();
    let _ = fs::remove_dir_all(&snapshot);
    outcome
}

pub fn sync(repo: &Path) -> Result<QueueState> {
    with_queue_lock(repo, || {
        let fetched = read_queue_file(repo)?;
        let previous = fetched
            .last_sync
            .as_ref()
            .map(|sync| sync.upstream_sha.clone());
        let sha = fetch_upstream(repo, &fetched)?;
        let merged = detect_merged_in_upstream(repo, &read_queue_file(repo)?)?;
        if previous.as_deref() == Some(sha.as_str()) && merged.is_empty() {
            let mut queue = read_queue_file(repo)?;
            queue.last_sync = Some(LastSync {
                at: stamp(),
                upstream_sha: sha,
                result: "ok".into(),
                message: Some("Synced with upstream".into()),
            });
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: sync with upstream")?;
            return Ok(queue);
        }
        match rebuild(repo) {
            Ok(mut queue) => {
                queue.last_sync = Some(LastSync {
                    at: stamp(),
                    upstream_sha: sha,
                    result: "ok".into(),
                    message: Some(if merged.is_empty() {
                        "Synced with upstream".into()
                    } else {
                        format!("Dropped merged patches: {}", merged.join(", "))
                    }),
                });
                write_queue_file(repo, &queue)?;
                commit_queue(repo, "uplink: sync with upstream")?;
                Ok(queue)
            }
            Err(Error::Conflict(_)) => read_queue_file(repo),
            Err(err) => Err(err),
        }
    })
}

pub fn resolve_conflict(repo: &Path, id: &str) -> Result<QueueState> {
    with_queue_lock(repo, || {
        let expected_branch = format!("uplink/conflict/{id}");
        let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        if head != expected_branch {
            return Err(Error::msg(format!(
                "Check out {expected_branch} before resolving {id} (currently on {head})."
            )));
        }
        let queue_ref = state_branch(repo);
        if has_ref(repo, &queue_ref)? {
            git(
                repo,
                &[
                    "restore",
                    "--source",
                    &queue_ref,
                    "--worktree",
                    "--",
                    ".uplink",
                ],
                GitOpts::default(),
            )?;
        }
        let mut queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?.clone();
        if patch.status != "conflict" {
            return Err(Error::msg(format!("{id} is not in conflict")));
        }
        let unmerged = conflicted_files(repo)?;
        if !unmerged.is_empty() {
            return Err(Error::msg(format!(
                "Conflict still has unmerged files: {}. Fix and git add them first.",
                unmerged.join(", ")
            )));
        }
        let markers = git(
            repo,
            &["grep", "-I", "-l", "^<<<<<<<", "--", ".", ":!.uplink"],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        )?;
        if markers.code == 0 && !markers.stdout.trim().is_empty() {
            return Err(Error::msg(format!(
                "Conflict markers still present in {}. Remove them before resolve.",
                markers.stdout.trim().replace('\n', ", ")
            )));
        }
        git(repo, &["add", "-A"], GitOpts::default())?;
        let staged = git(
            repo,
            &["diff", "--cached", "--quiet"],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        )?;
        if staged.code != 0 {
            let message = company_commit_message(&patch);
            git(repo, &["commit", "-m", &message], GitOpts::default())?;
        }
        if let Some(onto) = patch.conflict.as_ref().and_then(|c| c.onto.as_deref()) {
            git(repo, &["reset", "--soft", onto], GitOpts::default())?;
            let staged = git(
                repo,
                &["diff", "--cached", "--quiet"],
                GitOpts {
                    allow_fail: true,
                    ..GitOpts::default()
                },
            )?;
            if staged.code != 0 {
                let message = company_commit_message(&patch);
                git(repo, &["commit", "-m", &message], GitOpts::default())?;
            }
        }
        let formatted = git_ok(repo, &["format-patch", "--full-index", "-1", "--stdout"])?;
        fs::create_dir_all(repo.join(".uplink/patches"))?;
        let body = if formatted.ends_with('\n') {
            formatted
        } else {
            format!("{formatted}\n")
        };
        fs::write(repo.join(format!(".uplink/patches/{id}.patch")), body)?;
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = if patch.upstream.is_some() {
                "amended"
            } else {
                "queued"
            }
            .into();
            patch.conflict = None;
            patch.patch_id_stable = Some(stable_patch_id(
                repo,
                &format!(".uplink/patches/{id}.patch"),
            )?);
            add_event(
                patch,
                "amended",
                "Conflict resolved; patch refreshed for rebuild and upstream PR",
            );
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: amend {id} after conflict"))?;
        rebuild(repo)
    })
}

#[derive(Debug)]
pub struct SubmitResult {
    pub queue: QueueState,
    pub branch: String,
    pub sha: String,
}

pub fn submit_patch(repo: &Path, id: &str) -> Result<SubmitResult> {
    with_queue_lock(repo, || {
        ensure_upstream_ref(repo)?;
        let queue = read_queue_file(repo)?;
        let patch = get_patch(&queue, id)?.clone();
        if patch.intent != "upstream" {
            return Err(Error::msg(format!("{id} is internal-only")));
        }
        if patch.status != "approved" && patch.status != "submitted" {
            return Err(Error::msg(format!(
                "{id} must be approved before submit (currently {})",
                patch.status
            )));
        }
        for dep_id in &patch.depends_on {
            let dep = get_patch(&queue, dep_id)?;
            if dep.intent == "upstream" && dep.status != "merged" && dep.status != "submitted" {
                return Err(Error::msg(format!("Submit {dep_id} before {id}")));
            }
        }
        if patch.prepare.as_ref().is_some_and(|p| !p.ok) {
            return Err(Error::msg(format!(
                "{id} is not ready for contribution. Fix prepare-for-upstream findings first."
            )));
        }
        assert_export_preflight(
            repo,
            &queue,
            &patch,
            &repo.join(format!(".uplink/patches/{id}.patch")),
            None,
        )?;

        let branch = format!("uplink/{id}");
        let start = submit_base(repo, &queue, &patch)?;
        let snapshot = snapshot_uplink(repo)?;
        let applied = (|| -> Result<&'static str> {
            git(
                repo,
                &["checkout", "-f", "--quiet", "--detach", &start],
                GitOpts::default(),
            )?;
            apply_patch_file(
                repo,
                &patch,
                &snapshot.join(".uplink/patches").join(format!("{id}.patch")),
                true,
            )
        })();
        let applied = match applied {
            Ok(v) => v,
            Err(err) => {
                let _ = fs::remove_dir_all(&snapshot);
                return Err(err);
            }
        };
        if applied == "conflict" {
            let files = conflicted_files(repo).unwrap_or_default();
            git(
                repo,
                &["checkout", "-f", "--quiet", &queue.config.internal_branch],
                GitOpts::default(),
            )?;
            let _ = fs::remove_dir_all(&snapshot);
            return Err(Error::Conflict(ConflictError::new(
                format!("Cannot export {id} onto upstream"),
                id,
                files,
            )));
        }
        if applied == "empty" {
            git(
                repo,
                &["checkout", "-f", "--quiet", &queue.config.internal_branch],
                GitOpts::default(),
            )?;
            let _ = fs::remove_dir_all(&snapshot);
            return Err(Error::msg(format!(
                "{id} applies empty onto upstream; mark it merged instead."
            )));
        }
        git(repo, &["branch", "-f", &branch, "HEAD"], GitOpts::default())?;
        let _ = fs::remove_dir_all(&snapshot);

        let sha = rev_parse(repo, &branch)?;
        git(
            repo,
            &["checkout", "-f", "--quiet", &queue.config.internal_branch],
            GitOpts::default(),
        )?;
        let remotes = git_ok(repo, &["remote"]).unwrap_or_default();
        if remotes
            .split('\n')
            .any(|r| r == queue.config.contrib_remote)
        {
            git(
                repo,
                &[
                    "push",
                    "--force",
                    &queue.config.contrib_remote,
                    &format!("{branch}:{branch}"),
                ],
                GitOpts::default(),
            )?;
        }

        Ok(SubmitResult {
            queue: read_queue_file(repo)?,
            branch,
            sha,
        })
    })
}

fn submit_base(repo: &Path, queue: &QueueState, patch: &Patch) -> Result<String> {
    let submitted_deps: Vec<&Patch> = patch
        .depends_on
        .iter()
        .filter_map(|id| queue.patches.iter().find(|p| p.id == *id))
        .filter(|dep| dep.intent == "upstream" && dep.status == "submitted")
        .collect();
    if let Some(last) = submitted_deps.last() {
        if let Some(branch) = last.upstream.as_ref().map(|u| u.contrib_branch.as_str()) {
            if has_ref(repo, branch)? {
                return Ok(branch.to_string());
            }
        }
    }
    ensure_upstream_ref(repo)?;
    Ok("uplink/upstream".into())
}

pub struct StatusSnapshot {
    pub queue: QueueState,
    pub company_head: String,
    pub upstream_head: Option<String>,
    pub product_files: std::collections::BTreeMap<String, String>,
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
    Ok(StatusSnapshot {
        queue,
        company_head,
        upstream_head,
        product_files,
    })
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
        internal_only: 0,
    };
    for patch in &queue.patches {
        match patch.status.as_str() {
            "queued" => counts.queued += 1,
            "approved" => counts.approved += 1,
            "submitted" => counts.submitted += 1,
            "amended" => counts.amended += 1,
            "merged" => counts.merged += 1,
            "dropped" => counts.dropped += 1,
            "conflict" => counts.conflict += 1,
            _ => {}
        }
        if patch.intent == "internal-only" && patch.status != "dropped" {
            counts.internal_only += 1;
        }
    }
    counts
}

pub fn _queue_path() -> &'static str {
    QUEUE_PATH
}
