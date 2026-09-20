use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::adopt::{self, AdoptGroup};
use crate::error::{ConflictError, Error, Result};
use crate::gate::{
    assert_resolution_clean, commit_resolution, cut_gated_work, format_patch_at_head, recover_onto,
};
use crate::git::{GitOpts, configure_repo, git, git_ok};
use crate::lock::{is_push_lease_rejected, with_queue_lock};
use crate::preflight::{
    assert_export_preflight, assert_upstream_layer_applies, run_preflight_command_in,
};
use crate::prepare::{
    IncomingFlowedBack, assert_prepare_ok, company_commit_message, depends_on_from_message,
    format_incoming_packet, from_upstream_report_paths, prepare_from_message,
};
use crate::queue::{
    add_event, cannot_depend_on, empty_queue, get_patch, get_patch_mut, move_patch, patch_path,
    read_queue as read_queue_file, topological_active, write_queue as write_queue_file,
};
use crate::repo::{
    COMPANY_REMOTE, UPSTREAM_REF, ahead_behind, apply_patch_file, apply_state_sha, commit_queue,
    conflicted_files, copy_dir, ensure_configured_remotes, ensure_revs, ensure_state_worktree,
    ensure_upstream_ref, fetch_state_tracking, fetch_tracking_sha, fetch_upstream,
    fetch_upstream_remote, has_ref, is_ancestor, merge_base, new_patch_id,
    patch_already_applied_on, path_exists_at, point_branch_at, promote_upstream,
    push_branch_force_lease, push_state_branch, queue_at, refresh_company_branch,
    refresh_upstream_ref, replace_state_from_origin, restore_paths_from, rev_parse,
    set_state_branch, stable_patch_id, stable_patch_id_from_contents, stamp, state_branch,
    state_exists, try_replace_state_from_origin, uplink_uncommitted_paths, write_product_patch,
};
use crate::types::{
    Forge, GateKind, LastSync, MergeVia, Patch, PatchApproval, PatchConflict, PatchMerged,
    PatchSource, PatchUpstream, PendingUpstream, QUEUE_PATH, QueueConfig, QueueState, STATE_BRANCH,
    TransferDirection,
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
    pub forge: Option<Forge>,
    pub upgrade: bool,
    pub adopt_groups: Option<Vec<AdoptGroup>>,
    /// `None` detects a TTY. Tests set `Some(false)` so adopt never opens the TUI.
    pub interactive: Option<bool>,
}

impl InitOpts {
    pub fn has_args(&self) -> bool {
        self.upstream_url.is_some()
            || self.contrib_url.is_some()
            || self.upstream_remote_name.is_some()
            || self.upstream_branch.is_some()
            || self.contrib_remote_name.is_some()
            || self.internal_branch.is_some()
            || self.adopt_groups.is_some()
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
    config.forge = opts.forge;
    config
}

/// Create or hydrate an uplink queue. No CLI args fetches `origin` `uplink/state`
/// and reconstitutes remotes from stored URLs. Args create the queue when state
/// is missing, or sanity-check an existing queue. `--forge` is required when
/// creating a queue. `--upgrade` amends the stored forge pack in place.
pub fn init(repo: &Path, opts: InitOpts) -> Result<QueueState> {
    configure_repo(repo)?;
    if opts.upgrade {
        return init_upgrade(repo, &opts);
    }
    if !opts.has_args() && opts.forge.is_none() {
        return hydrate_from_origin(repo);
    }
    try_replace_state_from_origin(repo)?;
    if state_exists(repo)? {
        return init_existing(repo, &opts);
    }
    let forge = opts.forge.ok_or_else(missing_forge_error)?;
    let mut config = config_from_opts(&opts);
    config.forge = Some(forge);
    init_repo(repo, config)?;
    finish_first_init(repo, &opts)
}

fn hydrate_from_origin(repo: &Path) -> Result<QueueState> {
    replace_state_from_origin(repo)?;
    let queue = read_queue_file(repo)?;
    require_stored_urls(&queue.config)?;
    ensure_configured_remotes(repo, &queue.config)?;
    refresh_upstream_ref(repo, COMPANY_REMOTE)?;
    Ok(queue)
}

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
    configure_repo(repo)?;
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

fn missing_forge_error() -> Error {
    Error::msg("pass --forge ghec or --forge example-github when creating an uplink queue")
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
    if let Some(requested) = requested
        && requested != stored
    {
        out.push(format!(
            "  {field}: stored \"{stored}\", requested \"{requested}\""
        ));
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
    if let (Some(stored), Some(requested)) = (queue.config.forge, opts.forge)
        && stored != requested
    {
        mismatches.push(format!(
            "  forge: stored \"{stored}\", requested \"{requested}\""
        ));
    }
    if !mismatches.is_empty() {
        return Err(Error::msg(format!(
            "Cannot change uplink remote, branch, or forge names on an existing queue:\n{}",
            mismatches.join("\n")
        )));
    }

    let mut urls_changed = false;
    if let Some(url) = nonempty(opts.upstream_url.clone())
        && queue.config.upstream_url.as_deref() != Some(url.as_str())
    {
        queue.config.upstream_url = Some(url);
        urls_changed = true;
    }
    if let Some(url) = nonempty(opts.contrib_url.clone())
        && queue.config.contrib_url.as_deref() != Some(url.as_str())
    {
        queue.config.contrib_url = Some(url);
        urls_changed = true;
    }
    if urls_changed {
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: update remote urls")?;
    }
    ensure_configured_remotes(repo, &queue.config)?;
    refresh_upstream_ref(repo, COMPANY_REMOTE)?;
    if adopt::has_adopt_from(repo)? {
        return finish_adopt(repo, opts);
    }
    read_queue_file(repo)
}

fn init_upgrade(repo: &Path, opts: &InitOpts) -> Result<QueueState> {
    try_replace_state_from_origin(repo)?;
    if !state_exists(repo)? {
        return Err(Error::msg(
            "not initialized; run `git uplink init --upstream <url> --contrib <url> --forge <forge>` first",
        ));
    }
    init_existing(repo, opts)?;
    let mut queue = read_queue_file(repo)?;
    if queue.config.forge.is_none() {
        let forge = opts.forge.ok_or_else(|| {
            Error::msg(
                "queue.json has no forge. Re-run `git uplink init --upgrade --forge ghec` \
(or --forge example-github) to record it.",
            )
        })?;
        queue.config.forge = Some(forge);
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: record forge")?;
    }
    ensure_tooling_patch(repo)
}

fn ensure_tooling_patch(repo: &Path) -> Result<QueueState> {
    write_tooling_patch(repo, true)
}

fn write_tooling_patch(repo: &Path, rebuild_if_changed: bool) -> Result<QueueState> {
    crate::lock::with_queue_lock(repo, || {
        let refresh = crate::tooling::refresh_tooling_patch(repo)?;
        if rebuild_if_changed && refresh.changed {
            rebuild_once(repo)?;
        }
        read_queue_file(repo)
    })
}

fn finish_first_init(repo: &Path, opts: &InitOpts) -> Result<QueueState> {
    let analysis = adopt::analyze_ahead(repo)?;
    if analysis.behind {
        return Err(adopt::behind_error(&analysis));
    }
    if analysis.commits.is_empty() {
        return ensure_tooling_patch(repo);
    }
    adopt::save_adopt_from(repo)?;
    write_tooling_patch(repo, false)?;
    finish_adopt(repo, opts)
}

fn finish_adopt(repo: &Path, opts: &InitOpts) -> Result<QueueState> {
    let analysis = adopt::analyze_ahead(repo)?;
    if analysis.behind {
        return Err(adopt::behind_error(&analysis));
    }
    if analysis.commits.is_empty() {
        adopt::clear_adopt_from(repo)?;
        return read_queue_file(repo);
    }
    let queue = read_queue_file(repo)?;
    if adopt::has_product_patches(&queue) {
        return Err(Error::msg(
            "uplink/adopt-from is set but the queue already has product patches; \
delete uplink/adopt-from or reset uplink/state before adopting again",
        ));
    }
    let groups = groups_for_adopt(repo, opts, &queue, &analysis)?;
    let queue = adopt::apply_groups(repo, &analysis, &groups)?;
    adopt::clear_adopt_from(repo)?;
    Ok(queue)
}

fn groups_for_adopt(
    repo: &Path,
    opts: &InitOpts,
    queue: &QueueState,
    analysis: &adopt::AheadAnalysis,
) -> Result<Vec<AdoptGroup>> {
    if let Some(groups) = &opts.adopt_groups {
        return Ok(groups.clone());
    }
    let interactive = opts.interactive.unwrap_or_else(adopt::stdin_is_tty);
    if interactive {
        return crate::tui::run_adopt(repo, queue, analysis);
    }
    Err(Error::msg(
        "internal is ahead of uplink/upstream; pass --adopt-groups <file> \
or re-run git uplink init in a terminal to group commits",
    ))
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

fn add_patch_attempt(
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
        let id = existing.id.clone();
        apply_new_patch_on_company(repo, &id, false)?;
        return Ok(get_patch(&read_queue_file(repo)?, &id)?.clone());
    }
    add_patch_once(repo, &mut queue, opts, from_sha, head_sha)
}

fn add_patch_once(
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
    patch.prepare = Some(prepare_from_message(
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
    if let Some(report) = &patch.prepare {
        patch.commit_message = report.commit_message.clone();
        if !opts.internal_only {
            assert_prepare_ok(report, &opts.title)?;
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

    if let Err(err) =
        assert_change_already_on_company(repo, queue, &candidate_abs, &opts.title, head_sha)
    {
        let _ = fs::remove_file(&candidate_abs);
        return Err(err);
    }

    queue.push_patch(patch, opts.internal_only);
    write_queue_file(repo, queue)?;
    commit_queue(repo, &format!("uplink: add {id} {}", opts.title))?;
    if opts.internal_only {
        apply_new_patch_on_company(repo, &id, true)?;
    } else {
        rebuild_once(repo)?;
    }
    Ok(get_patch(&read_queue_file(repo)?, &id)?.clone())
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

fn push_queue_once(repo: &Path, remote: &str) -> Result<PushResult> {
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

fn restack_local_patches(
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

fn assert_change_already_on_company(
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

fn git_path(repo: &Path, spec: &str) -> Result<PathBuf> {
    let rel = git_ok(repo, &["rev-parse", "--git-path", spec])?;
    let path = PathBuf::from(&rel);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo.join(path))
    }
}

fn branch_history_shas(repo: &Path, branch: &str) -> Result<Vec<String>> {
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

fn looks_like_sha(value: &str) -> bool {
    value.len() == 40
        && value.chars().all(|c| c.is_ascii_hexdigit())
        && !value.chars().all(|c| c == '0')
}

fn apply_new_patch_on_company(repo: &Path, id: &str, mark_empty_merged: bool) -> Result<()> {
    ensure_upstream_ref(repo)?;
    let mut queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    let patch = get_patch(&queue, id)?.clone();
    let patch_file = repo.join(patch_path(id)?);

    if mark_empty_merged
        && queue.is_upstream(id)
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

    git(
        repo,
        &["checkout", "-f", "--quiet", &company_branch],
        GitOpts::default(),
    )?;
    if patch_already_applied_on(repo, &company_branch, &patch_file)? {
        return Ok(());
    }
    Err(Error::msg(format!(
        "\"{}\" is not on company {company_branch} yet. Merge the internal PR first, then import.",
        patch.title
    )))
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
            if queue.is_internal(id) || queue.is_tooling(id) {
                return Err(Error::msg(format!(
                    "{id} is internal-only and cannot be approved for upstream."
                )));
            }
            let patch = get_patch_mut(&mut queue, id)?;
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

pub fn record_gated_pr(
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
        if conflict.pr_number == Some(number) && conflict.pr_url.as_deref() == Some(url) {
            if let Some(remote) = push_remote {
                push_state_branch(repo, remote, &state_branch)?;
            }
            return Ok(patch);
        }
        if conflict.pr_number.is_some()
            && (conflict.pr_number != Some(number) || conflict.pr_url.as_deref() != Some(url))
        {
            let recorded = conflict
                .pr_url
                .unwrap_or_else(|| format!("#{}", conflict.pr_number.unwrap()));
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
        commit_queue(repo, &format!("uplink: conflict PR {id} #{number}"))?;
        if let Some(remote) = push_remote {
            push_state_branch(repo, remote, &queue.config.state_branch)?;
        }
        Ok(get_patch(&queue, id)?.clone())
    })
}

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

struct IncomingMatch {
    sha: String,
    patch_id: String,
    via: MergeVia,
}

struct IncomingClassification {
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

fn eligible_for_flow_back(queue: &QueueState, patch: &Patch) -> bool {
    patch.status != "merged" && patch.status != "dropped" && queue.is_upstream(&patch.id)
}

fn commit_stable_patch_id(repo: &Path, sha: &str) -> Result<Option<String>> {
    let shown = git(
        repo,
        &["show", "--binary", sha],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
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

fn match_commit_to_patch(
    repo: &Path,
    queue: &QueueState,
    sha: &str,
) -> Result<Option<(String, MergeVia)>> {
    let message = git_ok(repo, &["log", "-1", "--format=%B", sha]).unwrap_or_default();
    for patch in queue
        .all_patches()
        .filter(|p| eligible_for_flow_back(queue, p))
    {
        let trailer = format!("{}: {}", queue.config.trailer_key, patch.id);
        if message.lines().any(|line| line.trim() == trailer) {
            return Ok(Some((patch.id.clone(), MergeVia::Trailer)));
        }
    }
    if let Some(stable) = commit_stable_patch_id(repo, sha)? {
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

fn classify_incoming(
    repo: &Path,
    queue: &QueueState,
    from_sha: Option<&str>,
    pending_sha: &str,
) -> Result<IncomingClassification> {
    let mut range = Vec::new();
    if let Some(from) = from_sha {
        let list = git_ok(
            repo,
            &[
                "rev-list",
                "--no-merges",
                "--reverse",
                &format!("{from}..{pending_sha}"),
            ],
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

fn write_incoming_packet(
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
    let ids: Vec<String> = queue.all_patches().map(|p| p.id.clone()).collect();
    for id in ids {
        let live = read_queue_file(repo)?;
        let patch = match live.all_patches().find(|p| p.id == id) {
            Some(p) => p.clone(),
            None => continue,
        };
        if patch.status == "merged" || patch.status == "dropped" || !live.is_upstream(&id) {
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
        current.status = "conflict".into();
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

#[derive(Debug, Clone, Default)]
pub struct RebuildOpts {
    pub branch: Option<String>,
    pub push: bool,
    pub push_remote: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RebuildResult {
    pub queue: QueueState,
    pub branch: String,
    pub preview: bool,
}

pub fn rebuild(repo: &Path) -> Result<QueueState> {
    Ok(rebuild_with(repo, RebuildOpts::default())?.queue)
}

pub fn rebuild_with(repo: &Path, opts: RebuildOpts) -> Result<RebuildResult> {
    with_queue_lock(repo, || {
        let queued = read_queue_file(repo)?;
        let company_branch = queued.config.internal_branch.clone();
        let state_branch = queued.config.state_branch.clone();
        let target = opts
            .branch
            .as_deref()
            .unwrap_or(company_branch.as_str())
            .to_string();
        if is_reserved_rebuild_branch(&target, &queued.config) {
            return Err(Error::msg(format!(
                "cannot rebuild onto reserved branch {target}"
            )));
        }
        let preview = target != company_branch;
        let queue = if preview {
            rebuild_preview(repo, &target)?
        } else {
            rebuild_once(repo)?
        };
        if opts.push {
            let remote = opts.push_remote.as_deref().unwrap_or("origin");
            push_branch_force_lease(repo, remote, &target)?;
            push_state_branch(repo, remote, &state_branch)?;
        }
        Ok(RebuildResult {
            queue,
            branch: target,
            preview,
        })
    })
}

fn is_reserved_rebuild_branch(name: &str, config: &crate::types::QueueConfig) -> bool {
    name == "uplink/state"
        || name == config.state_branch
        || name == "uplink/upstream"
        || name == adopt::ADOPT_FROM_REF
        || name.starts_with("uplink/conflict/")
        || name.starts_with("uplink/transfer-to-upstream/")
        || name.starts_with("uplink/transfer-to-internal/")
}

fn checkout_identity(repo: &Path) -> Result<(String, String)> {
    Ok((
        git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?,
        git_ok(repo, &["rev-parse", "HEAD"])?,
    ))
}

fn restore_checkout(repo: &Path, name: &str, sha: &str) -> Result<()> {
    if name != "HEAD" {
        git(
            repo,
            &["checkout", "-f", "--quiet", name],
            GitOpts::default(),
        )?;
    } else {
        git(
            repo,
            &["checkout", "-f", "--quiet", sha],
            GitOpts::default(),
        )?;
    }
    Ok(())
}

fn rebuild_preview(repo: &Path, branch: &str) -> Result<QueueState> {
    let queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    ensure_upstream_ref(repo)?;
    let upstream_ref = if has_ref(repo, "uplink/upstream")? {
        "uplink/upstream"
    } else {
        company_branch.as_str()
    };
    let (original, original_sha) = checkout_identity(repo)?;
    let snapshot = snapshot_uplink(repo)?;
    let outcome = (|| -> Result<QueueState> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", upstream_ref],
            GitOpts::default(),
        )?;
        let mut last_good = git_ok(repo, &["rev-parse", "HEAD"])?;
        git(
            repo,
            &["branch", "-f", branch, &last_good],
            GitOpts::default(),
        )?;
        for patch in topological_active(&queue)? {
            if patch.status == "conflict" {
                return Err(Error::msg(format!(
                    "Queue is blocked on conflict in {}",
                    patch.id
                )));
            }
            let patch_file = snapshot.join(patch_path(&patch.id)?);
            let result = apply_patch_file(repo, &patch, &patch_file, false)?;
            if result == "empty" {
                continue;
            }
            if result == "conflict" {
                let files = conflicted_files(repo)?;
                git(
                    repo,
                    &["reset", "--hard", "--quiet", &last_good],
                    GitOpts::default(),
                )?;
                git(
                    repo,
                    &["branch", "-f", branch, &last_good],
                    GitOpts::default(),
                )?;
                let list = if files.is_empty() {
                    "untracked conflict".into()
                } else {
                    files.join(", ")
                };
                return Err(Error::msg(format!(
                    "Preview rebuild stopped on {} (\"{}\"): {list}",
                    patch.id, patch.title
                )));
            }
            last_good = git_ok(repo, &["rev-parse", "HEAD"])?;
            git(
                repo,
                &["branch", "-f", branch, &last_good],
                GitOpts::default(),
            )?;
        }
        Ok(queue)
    })();
    let _ = fs::remove_dir_all(&snapshot);
    restore_checkout(repo, &original, &original_sha)?;
    crate::repo::ensure_state_worktree(repo)?;
    outcome
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
            let patch_file = snapshot.join(patch_path(&patch.id)?);
            let result = apply_patch_file(repo, &patch, &patch_file, false)?;
            if result == "empty" {
                let is_upstream = queue.is_upstream(&patch.id);
                let current = get_patch_mut(&mut queue, &patch.id)?;
                if is_upstream {
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
                    &patch,
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

fn apply_fetched_upstream(repo: &Path, sha: &str) -> Result<QueueState> {
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
                    format!("Dropped merged patches: {}", merged.join(", "))
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

pub fn resolve_conflict(repo: &Path, id: &str) -> Result<QueueState> {
    with_queue_lock(repo, || {
        let base = GateKind::Conflict.base_branch(id);
        let work = GateKind::Conflict.work_branch(id);
        let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        if head != base && head != work {
            return Err(Error::msg(format!(
                "Check out {base} or {work} before resolving {id} (currently on {head})."
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
        assert_resolution_clean(repo)?;
        let onto = patch
            .conflict
            .as_ref()
            .and_then(|c| c.onto.clone())
            .unwrap_or(recover_onto(repo, GateKind::Conflict, id, &head)?);
        let message = company_commit_message(&patch);
        commit_resolution(repo, &onto, &message)?;
        fs::create_dir_all(repo.join(".uplink/patches"))?;
        fs::write(repo.join(patch_path(id)?), format_patch_at_head(repo)?)?;
        {
            let patch = get_patch_mut(&mut queue, id)?;
            patch.status = if patch.upstream.is_some() {
                "amended"
            } else {
                "queued"
            }
            .into();
            patch.conflict = None;
            let rel = patch_path(id)?.to_string_lossy().into_owned();
            patch.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
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

#[derive(Debug, Clone)]
pub struct TransferResult {
    pub queue: QueueState,
    pub id: String,
    pub direction: TransferDirection,
    pub transferred: bool,
    pub gated: bool,
    pub base_branch: Option<String>,
    pub work_branch: Option<String>,
    pub onto: Option<String>,
    pub files: Vec<String>,
    pub message: Option<String>,
    pub pr_close_url: Option<String>,
    pub pr_close_number: Option<u64>,
}

pub fn transfer_patch(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    complete: bool,
) -> Result<TransferResult> {
    with_queue_lock(repo, || {
        if complete {
            complete_transfer(repo, id, direction)
        } else {
            start_transfer(repo, id, direction)
        }
    })
}

fn validate_transfer(queue: &QueueState, id: &str, direction: TransferDirection) -> Result<Patch> {
    if queue.is_tooling(id) {
        return Err(Error::msg(format!(
            "{id} is tooling and cannot be transferred"
        )));
    }
    let source_ok = match direction {
        TransferDirection::ToUpstream => queue.is_internal(id),
        TransferDirection::ToInternal => queue.is_upstream(id),
    };
    if !source_ok {
        let want = match direction {
            TransferDirection::ToUpstream => "internal",
            TransferDirection::ToInternal => "upstream",
        };
        return Err(Error::msg(format!(
            "{id} is not in the {want} queue; cannot transfer {}",
            direction.as_str()
        )));
    }
    let patch = get_patch(queue, id)?.clone();
    match patch.status.as_str() {
        "merged" | "dropped" | "conflict" => {
            return Err(Error::msg(format!(
                "{id} cannot be transferred while status is {}",
                patch.status
            )));
        }
        _ => {}
    }

    let mut preview = queue.clone();
    move_patch(&mut preview, id, direction.to_internal())?;
    for dep in &patch.depends_on {
        if cannot_depend_on(&preview, direction.to_internal(), dep) {
            return Err(Error::msg(format!(
                "{id} cannot depend on {dep} after transfer {}",
                direction.as_str()
            )));
        }
    }
    for other in preview.all_patches() {
        if other.id == id || !other.depends_on.iter().any(|d| d == id) {
            continue;
        }
        if cannot_depend_on(&preview, preview.is_internal(&other.id), id) {
            return Err(Error::msg(format!(
                "{} depends on {id}; transferring {} would break layer rules",
                other.id,
                direction.as_str()
            )));
        }
    }
    Ok(patch)
}

fn start_transfer(repo: &Path, id: &str, direction: TransferDirection) -> Result<TransferResult> {
    let queue = read_queue_file(repo)?;
    let patch = validate_transfer(&queue, id, direction)?;
    let company_branch = queue.config.internal_branch.clone();
    let mut preview = queue.clone();
    move_patch(&mut preview, id, direction.to_internal())?;

    ensure_upstream_ref(repo)?;
    let upstream_ref = if has_ref(repo, "uplink/upstream")? {
        "uplink/upstream"
    } else {
        company_branch.as_str()
    };
    let (original, original_sha) = checkout_identity(repo)?;
    let snapshot = snapshot_uplink(repo)?;
    let kind = direction.gate_kind();
    let outcome = (|| -> Result<TransferResult> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", upstream_ref],
            GitOpts::default(),
        )?;
        let mut target_onto = None;
        let mut target_after = None;
        for item in topological_active(&preview)? {
            if item.status == "conflict" {
                return Err(Error::msg(format!(
                    "Queue is blocked on conflict in {}",
                    item.id
                )));
            }
            let onto_here = rev_parse(repo, "HEAD")?;
            let patch_file = snapshot.join(patch_path(&item.id)?);
            let result = apply_patch_file(repo, &item, &patch_file, false)?;
            if result == "conflict" {
                if item.id != id {
                    return Err(Error::msg(format!(
                        "Cannot transfer {id}: {} (\"{}\") would not apply after the move.",
                        item.id, item.title
                    )));
                }
                let files = conflicted_files(repo)?;
                let (base, work) = cut_gated_work(
                    repo,
                    kind,
                    id,
                    &onto_here,
                    &format!("uplink: transfer {id} {}", direction.as_str()),
                )?;
                return Ok(gated_transfer_result(
                    queue.clone(),
                    id,
                    direction,
                    (base, work, onto_here),
                    files,
                    format!("Patch {id} does not apply in the destination layer."),
                ));
            }
            if item.id == id {
                target_onto = Some(onto_here);
                target_after = Some(rev_parse(repo, "HEAD")?);
            }
        }

        copy_dir(&snapshot.join(".uplink"), &repo.join(".uplink"))?;
        let preflight = match direction {
            TransferDirection::ToUpstream => {
                let abs = snapshot.join(patch_path(id)?);
                assert_export_preflight(repo, &preview, &patch, &abs, None)
                    .and_then(|_| assert_upstream_layer_applies(repo, &preview))
            }
            TransferDirection::ToInternal => run_preflight_command_in(&preview, repo),
        };
        let _ = fs::remove_dir_all(repo.join(".uplink"));
        if let Err(err) = preflight {
            let onto = target_onto.ok_or_else(|| {
                Error::msg(format!("{id} was not applied during transfer preview"))
            })?;
            if let Some(after) = &target_after {
                git(
                    repo,
                    &["checkout", "-f", "--quiet", after],
                    GitOpts::default(),
                )?;
            }
            let (base, work) = cut_gated_work(
                repo,
                kind,
                id,
                &onto,
                &format!("uplink: transfer {id} {}", direction.as_str()),
            )?;
            return Ok(gated_transfer_result(
                queue.clone(),
                id,
                direction,
                (base, work, onto),
                Vec::new(),
                err.to_string(),
            ));
        }

        restore_checkout(repo, &original, &original_sha)?;
        crate::repo::ensure_state_worktree(repo)?;
        finish_successful_transfer(repo, id, direction)
    })();
    let _ = fs::remove_dir_all(&snapshot);
    match outcome {
        Ok(result) if result.gated => {
            restore_checkout(repo, &original, &original_sha)?;
            crate::repo::ensure_state_worktree(repo)?;
            Ok(result)
        }
        Ok(result) => Ok(result),
        Err(err) => {
            let _ = restore_checkout(repo, &original, &original_sha);
            let _ = crate::repo::ensure_state_worktree(repo);
            Err(err)
        }
    }
}

fn gated_transfer_result(
    queue: QueueState,
    id: &str,
    direction: TransferDirection,
    (base, work, onto): (String, String, String),
    files: Vec<String>,
    message: String,
) -> TransferResult {
    TransferResult {
        queue,
        id: id.into(),
        direction,
        transferred: false,
        gated: true,
        base_branch: Some(base),
        work_branch: Some(work),
        onto: Some(onto),
        files,
        message: Some(message),
        pr_close_url: None,
        pr_close_number: None,
    }
}

fn finish_successful_transfer(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
) -> Result<TransferResult> {
    let mut queue = read_queue_file(repo)?;
    validate_transfer(&queue, id, direction)?;
    let mut pr_close_url = None;
    let mut pr_close_number = None;
    {
        let patch = get_patch(&queue, id)?;
        if direction.to_internal() {
            pr_close_url = patch.upstream.as_ref().and_then(|u| u.pr_url.clone());
            pr_close_number = patch.upstream.as_ref().and_then(|u| u.pr_number);
        }
    }
    move_patch(&mut queue, id, direction.to_internal())?;
    {
        let patch = get_patch_mut(&mut queue, id)?;
        patch.status = "queued".into();
        patch.conflict = None;
        if direction.to_internal() {
            patch.upstream = None;
            patch.approvals.clear();
        }
        add_event(
            patch,
            "transferred",
            format!("Moved {}", direction.as_str()),
        );
    }
    write_queue_file(repo, &queue)?;
    commit_queue(
        repo,
        &format!("uplink: transfer {id} {}", direction.as_str()),
    )?;
    let queue = rebuild(repo)?;
    Ok(TransferResult {
        queue,
        id: id.into(),
        direction,
        transferred: true,
        gated: false,
        base_branch: None,
        work_branch: None,
        onto: None,
        files: Vec::new(),
        message: None,
        pr_close_url,
        pr_close_number,
    })
}

fn complete_transfer(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
) -> Result<TransferResult> {
    let kind = direction.gate_kind();
    let base = kind.base_branch(id);
    let work = kind.work_branch(id);
    let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if head != base && head != work {
        return Err(Error::msg(format!(
            "Check out {base} or {work} before completing transfer of {id} (currently on {head})."
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
    let queue = read_queue_file(repo)?;
    let patch = validate_transfer(&queue, id, direction)?;
    assert_resolution_clean(repo)?;
    let onto = recover_onto(repo, kind, id, &head)?;
    let message = company_commit_message(&patch);
    commit_resolution(repo, &onto, &message)?;
    fs::create_dir_all(repo.join(".uplink/patches"))?;
    fs::write(repo.join(patch_path(id)?), format_patch_at_head(repo)?)?;

    let mut preview = queue.clone();
    move_patch(&mut preview, id, direction.to_internal())?;
    let preflight = match direction {
        TransferDirection::ToUpstream => {
            let abs = repo.join(patch_path(id)?);
            assert_export_preflight(repo, &preview, get_patch(&preview, id)?, &abs, None)
                .and_then(|_| assert_upstream_layer_applies(repo, &preview))
        }
        TransferDirection::ToInternal => run_preflight_command_in(&preview, repo),
    };
    preflight?;

    let rel = patch_path(id)?.to_string_lossy().into_owned();
    let stable = stable_patch_id(repo, &rel)?;
    let mut queue = read_queue_file(repo)?;
    let mut pr_close_url = None;
    let mut pr_close_number = None;
    {
        let current = get_patch(&queue, id)?;
        if direction.to_internal() {
            pr_close_url = current.upstream.as_ref().and_then(|u| u.pr_url.clone());
            pr_close_number = current.upstream.as_ref().and_then(|u| u.pr_number);
        }
    }
    move_patch(&mut queue, id, direction.to_internal())?;
    {
        let current = get_patch_mut(&mut queue, id)?;
        current.status = "queued".into();
        current.conflict = None;
        current.patch_id_stable = Some(stable);
        if direction.to_internal() {
            current.upstream = None;
            current.approvals.clear();
        }
        add_event(
            current,
            "transferred",
            format!("Moved {} after gated work", direction.as_str()),
        );
    }
    write_queue_file(repo, &queue)?;
    commit_queue(
        repo,
        &format!("uplink: transfer {id} {}", direction.as_str()),
    )?;
    let queue = rebuild(repo)?;
    Ok(TransferResult {
        queue,
        id: id.into(),
        direction,
        transferred: true,
        gated: false,
        base_branch: None,
        work_branch: None,
        onto: None,
        files: Vec::new(),
        message: None,
        pr_close_url,
        pr_close_number,
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
        if !queue.is_upstream(id) {
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
            if queue.is_upstream(dep_id) && dep.status != "merged" && dep.status != "submitted" {
                return Err(Error::msg(format!("Submit {dep_id} before {id}")));
            }
        }
        if patch.prepare.as_ref().is_some_and(|p| !p.ok) {
            return Err(Error::msg(format!(
                "{id} is not ready for contribution. Fix prepare-for-upstream findings first."
            )));
        }
        assert_export_preflight(repo, &queue, &patch, &repo.join(patch_path(id)?), None)?;

        let branch = format!("uplink/{id}");
        let start = submit_base(repo, &queue, &patch)?;
        let snapshot = snapshot_uplink(repo)?;
        let applied = (|| -> Result<&'static str> {
            git(
                repo,
                &["checkout", "-f", "--quiet", "--detach", &start],
                GitOpts::default(),
            )?;
            apply_patch_file(repo, &patch, &snapshot.join(patch_path(id)?), true)
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
        .filter_map(|id| queue.all_patches().find(|p| p.id == *id))
        .filter(|dep| queue.is_upstream(&dep.id) && dep.status == "submitted")
        .collect();
    if let Some(last) = submitted_deps.last()
        && let Some(branch) = last.upstream.as_ref().map(|u| u.contrib_branch.as_str())
        && has_ref(repo, branch)?
    {
        return Ok(branch.to_string());
    }
    ensure_upstream_ref(repo)?;
    Ok("uplink/upstream".into())
}

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

fn state_status(repo: &Path) -> Result<StateStatus> {
    state_status_at(repo, true)
}

pub fn state_status_at(repo: &Path, fetch: bool) -> Result<StateStatus> {
    let branch = state_branch(repo);
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

fn short_sha(sha: &str) -> &str {
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
        if queue.is_tooling(&patch.id) && patch.status != "dropped" {
            counts.tooling += 1;
        }
        if queue.is_internal(&patch.id) && patch.status != "dropped" {
            counts.internal += 1;
            counts.internal_only += 1;
        }
    }
    counts
}

pub fn _queue_path() -> &'static str {
    QUEUE_PATH
}
