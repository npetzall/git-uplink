use super::*;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetResult {
    pub internal_branch: String,
    pub internal_sha: String,
    pub state_sha: String,
    pub upstream_sha: String,
    /// `None` when origin has no `uplink/hooks` yet; an upgrade creates it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks_sha: Option<String>,
}

pub type RefreshResult = ResetResult;

/// Fetch origin tracking refs for company main, `uplink/state`, `uplink/upstream`,
/// and `uplink/hooks` when origin has it. Does not move local branches or restore `.uplink/`.
pub fn refresh_from_origin(repo: &Path) -> Result<RefreshResult> {
    let state_sha = fetch_tracking_sha(repo, COMPANY_REMOTE, STATE_BRANCH)?;
    let upstream_sha = fetch_tracking_sha(repo, COMPANY_REMOTE, UPSTREAM_REF)?;
    let queue = queue_at(repo, &format!("{COMPANY_REMOTE}/{STATE_BRANCH}"))?;
    let internal_branch = queue.config.internal_branch.clone();
    let internal_sha = fetch_tracking_sha(repo, COMPANY_REMOTE, &internal_branch)?;
    let hooks_sha = fetch_state_tracking(repo, COMPANY_REMOTE, HOOKS_BRANCH)?;
    Ok(RefreshResult {
        internal_branch,
        internal_sha,
        state_sha,
        upstream_sha,
        hooks_sha,
    })
}

/// Fetch origin and hard-reset company main, `uplink/state`, `uplink/upstream`,
/// and `uplink/hooks` when origin has it. A local `uplink/hooks` origin lacks
/// is kept. Leaves HEAD on the configured internal branch with `.uplink/`
/// restored from origin.
pub fn reset_from_origin(repo: &Path) -> Result<ResetResult> {
    crate::lock::refuse_while_running(repo)?;
    let fetched = refresh_from_origin(repo)?;
    refresh_company_branch(repo, COMPANY_REMOTE, &fetched.internal_branch)?;
    apply_state_sha(repo, STATE_BRANCH, &fetched.state_sha)?;
    point_branch_at(repo, UPSTREAM_REF, &fetched.upstream_sha)?;
    if let Some(hooks_sha) = &fetched.hooks_sha {
        point_branch_at(repo, HOOKS_BRANCH, hooks_sha)?;
    }
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
    /// What happened to `uplink/hooks`, pushed after `uplink/state`.
    pub hooks: HooksPushAction,
}

/// How often [`record_on_remote`] records again after losing a push race.
const RECORD_PUSH_ATTEMPTS: u32 = 8;

/// Runs `record` on `uplink/state` as `remote` has it, and pushes what it
/// recorded there as a fast-forward. Never forced.
///
/// Before each attempt, local `uplink/state` and `.uplink/` are moved to the
/// remote's `uplink/state`, and the company branch and `uplink/upstream` back
/// to where they were, so `record` (and a rebuild or sync it runs) works on
/// the newest state from the same start. When the push
/// loses a race, `record` runs again on the state that won. `record` must
/// be safe to run again: an import is, since it records a pull request once.
///
/// Refuses when `.uplink/` has uncommitted changes, or local `uplink/state`
/// has commits the remote lacks: moving it would lose them.
///
/// Returns what `record` returned and the remote's `uplink/state` it was
/// recorded on.
pub fn record_on_remote<T>(
    repo: &Path,
    remote: &str,
    record: impl FnMut() -> Result<T>,
) -> Result<(T, String)> {
    record_on_remote_if(repo, remote, record, |_| true)
}

/// [`record_on_remote`], pushing only when `keep` says so for what `record`
/// returned. Otherwise local `uplink/state` and `.uplink/` go back to the
/// remote's and nothing is pushed.
pub fn record_on_remote_if<T>(
    repo: &Path,
    remote: &str,
    mut record: impl FnMut() -> Result<T>,
    keep: impl Fn(&T) -> bool,
) -> Result<(T, String)> {
    with_queue_lock(repo, || {
        ensure_state_worktree(repo)?;
        let uncommitted = uplink_uncommitted_paths(repo, STATE_BRANCH)?;
        if !uncommitted.is_empty() {
            return Err(Error::msg(format!(
                ".uplink/ has uncommitted changes ({}); commit or reset them first",
                uncommitted.join(", ")
            )));
        }
        let company_branch = read_queue_file(repo)?.config.internal_branch;
        let main_start = if has_ref(repo, &company_branch)? {
            Some(rev_parse(repo, &company_branch)?)
        } else {
            None
        };
        let upstream_start = if has_ref(repo, UPSTREAM_REF)? {
            Some(rev_parse(repo, UPSTREAM_REF)?)
        } else {
            None
        };
        let mut last_error = None;
        for attempt in 0..RECORD_PUSH_ATTEMPTS {
            let Some(base) = fetch_state_tracking(repo, remote, STATE_BRANCH)? else {
                return Err(Error::msg(format!(
                    "Could not fetch {STATE_BRANCH} from {remote}."
                )));
            };
            let local = rev_parse(repo, STATE_BRANCH)?;
            if attempt == 0 && !is_ancestor(repo, &local, &base)? {
                return Err(Error::msg(format!(
                    "{STATE_BRANCH} has commits {remote} lacks; run `git uplink push`, then run this again"
                )));
            }
            if local != base {
                apply_state_sha(repo, STATE_BRANCH, &base)?;
            }
            if let Some(start) = main_start.as_deref()
                && rev_parse(repo, &company_branch)? != start
            {
                git(
                    repo,
                    &["checkout", "-f", "--quiet", "-B", &company_branch, start],
                    GitOpts::default(),
                )?;
            }
            if let Some(start) = upstream_start.as_deref()
                && rev_parse(repo, UPSTREAM_REF)? != start
            {
                point_branch_at(repo, UPSTREAM_REF, start)?;
            }
            let value = record()?;
            if rev_parse(repo, STATE_BRANCH)? == base {
                return Ok((value, base));
            }
            if !keep(&value) {
                apply_state_sha(repo, STATE_BRANCH, &base)?;
                return Ok((value, base));
            }
            match push_state_branch(repo, remote, STATE_BRANCH) {
                Ok(()) => return Ok((value, base)),
                Err(err) if is_push_lease_rejected(&err) => {
                    last_error = Some(err);
                    thread::sleep(Duration::from_millis(40 * 2u64.pow(attempt)));
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_error.unwrap_or_else(|| Error::msg("push failed")))
    })
}

/// What [`push_imported_main`] did with the company branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MainPush {
    /// The import did not rebuild it: nothing to push.
    Unchanged,
    /// Force-pushed with a lease on the merge commit.
    Pushed,
    /// The remote branch is no longer the merge commit (a merge landed
    /// after it), so the rebuilt branch was not pushed.
    Moved,
    /// The rebuilt branch has another tree than the merge commit, so pushing
    /// it would change content, not only order. Not pushed.
    Differs,
    /// The remote branch has commits no rebuild wrote (a merge that is not
    /// imported yet), which the rebuild left out. Not pushed: that import
    /// rebuilds from the recorded state and pushes the branch.
    Unrecorded,
}

/// Pushes the company branch an import rebuilt, only when that changes the
/// order of commits and nothing else, and only while the remote branch is
/// still `merged_as`: the import's scope then includes its head.
///
/// The rebuilt tree must equal the tree of `merged_as`. The push is forced,
/// with a lease on `merged_as`, so a merge that landed while the import ran
/// is never overwritten. A lost lease is not an error: the import of that
/// merge rebuilds and pushes the branch.
pub fn push_imported_main(
    repo: &Path,
    remote: &str,
    main_start: Option<&str>,
    merged_as: &str,
) -> Result<MainPush> {
    let company_branch = read_queue_file(repo)?.config.internal_branch;
    if !has_ref(repo, &company_branch)? {
        return Ok(MainPush::Unchanged);
    }
    let main = rev_parse(repo, &company_branch)?;
    if main_start == Some(main.as_str()) {
        return Ok(MainPush::Unchanged);
    }
    let tree = rev_parse(repo, &format!("{main}^{{tree}}"))?;
    if tree != rev_parse(repo, &format!("{merged_as}^{{tree}}"))? {
        return Ok(MainPush::Differs);
    }
    let lease = format!("--force-with-lease=refs/heads/{company_branch}:{merged_as}");
    let dest = format!("{main}:refs/heads/{company_branch}");
    match git(repo, &["push", &lease, remote, &dest], GitOpts::default()) {
        Ok(_) => Ok(MainPush::Pushed),
        Err(err) if is_push_lease_rejected(&err) => Ok(MainPush::Moved),
        Err(err) => Err(err),
    }
}

/// Pushes the company branch a resolve, amend, transfer or sync rebuilt,
/// only when that overwrites nothing on the remote: a force-push with a
/// lease on `lease` (the remote branch as fetched right before), and only
/// when `uplink/state` already had everything `lease` has. That holds when
/// every commit on `lease` above `uplink/upstream` was written by a rebuild
/// (has an `Uplink-Patch-Id` naming a patch on `recorded_on`), or when a rebuild of `recorded_on` (the
/// state the command started from) on `upstream_start` gives the tree of
/// `lease`. A merge that is not recorded yet is never dropped from the
/// remote; its import pushes the branch.
pub fn push_rebuilt_main(
    repo: &Path,
    remote: &str,
    main_start: Option<&str>,
    lease: &str,
    recorded_on: &str,
    upstream_start: &str,
    force: bool,
) -> Result<MainPush> {
    let company_branch = read_queue_file(repo)?.config.internal_branch;
    if !has_ref(repo, &company_branch)? {
        return Ok(MainPush::Unchanged);
    }
    let main = rev_parse(repo, &company_branch)?;
    if main_start == Some(main.as_str()) || main == lease {
        return Ok(MainPush::Unchanged);
    }
    // A commit counts as written by a rebuild only when its Uplink-Patch-Id
    // names a patch uplink/state has: a message can carry the trailer too.
    let known: HashSet<String> = queue_at(repo, recorded_on)?
        .all_patches()
        .map(|patch| patch.id.clone())
        .collect();
    let unrecorded = crate::repo::commits_with_patch_ids(
        repo,
        &["--first-parent", lease, "--not", UPSTREAM_REF],
    )?
    .into_iter()
    .any(|(_, ids)| ids.is_empty() || ids.iter().any(|id| !known.contains(id)));
    if unrecorded && !force && !recorded_in_history(repo, lease, recorded_on, upstream_start)? {
        return Ok(MainPush::Unrecorded);
    }
    let lease_arg = format!("--force-with-lease=refs/heads/{company_branch}:{lease}");
    let dest = format!("{main}:refs/heads/{company_branch}");
    match git(
        repo,
        &["push", &lease_arg, remote, &dest],
        GitOpts::default(),
    ) {
        Ok(_) => Ok(MainPush::Pushed),
        Err(err) if is_push_lease_rejected(&err) => Ok(MainPush::Moved),
        Err(err) => Err(err),
    }
}

/// How far back [`recorded_in_history`] looks in `uplink/state`.
const RECORDED_HISTORY_DEPTH: &str = "20";

/// True when a rebuild of `recorded_on`, or of one of its recent
/// first-parent ancestors on `uplink/state`, gives the tree of `main`: then
/// everything `main` has is in the history of `uplink/state`, and replacing
/// it loses nothing. An older state matches when the queue changed since
/// `main` was built from it, for example the tooling patch of an
/// `init --upgrade`, a drop or an amend. Each state is rebuilt on the
/// upstream `main` was built on.
fn recorded_in_history(
    repo: &Path,
    main: &str,
    recorded_on: &str,
    upstream_start: &str,
) -> Result<bool> {
    let main_tree = rev_parse(repo, &format!("{main}^{{tree}}"))?;
    let upstream = merge_base(repo, main, upstream_start)?.unwrap_or_else(|| upstream_start.into());
    let states = git_ok(
        repo,
        &[
            "rev-list",
            "--first-parent",
            "--max-count",
            RECORDED_HISTORY_DEPTH,
            recorded_on,
        ],
    )?;
    for state in states.lines().filter(|line| !line.is_empty()) {
        if !path_exists_at(repo, state, crate::types::QUEUE_PATH)? {
            continue;
        }
        // A state that does not rebuild here is no match.
        let rebuilt = super::rebuild::rebuilt_tree_at(repo, state, &upstream)
            .ok()
            .flatten();
        if rebuilt.as_deref() == Some(main_tree.as_str()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The push of an operator's `rebuild --push`: local `uplink/state` as a
/// fast-forward (never forced), then the rebuilt company branch as
/// [`push_rebuilt_main`] does, with a lease on the remote branch fetched
/// right before. `force` pushes it also when `uplink/state` lacks something
/// the remote branch has; the lease still holds.
pub fn push_operator_rebuild(
    repo: &Path,
    remote: &str,
    recorded_on: &str,
    upstream_start: &str,
    force: bool,
) -> Result<MainPush> {
    push_state_branch(repo, remote, STATE_BRANCH)?;
    let company_branch = read_queue_file(repo)?.config.internal_branch;
    let Some(lease) = fetch_state_tracking(repo, remote, &company_branch)? else {
        // The first publish: leased on the branch not existing yet.
        let lease = format!("--force-with-lease=refs/heads/{company_branch}:");
        let dest = format!("refs/heads/{company_branch}:refs/heads/{company_branch}");
        return match git(repo, &["push", &lease, remote, &dest], GitOpts::default()) {
            Ok(_) => Ok(MainPush::Pushed),
            Err(err) if is_push_lease_rejected(&err) => Ok(MainPush::Moved),
            Err(err) => Err(err),
        };
    };
    push_rebuilt_main(
        repo,
        remote,
        None,
        &lease,
        recorded_on,
        upstream_start,
        force,
    )
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
                Ok(mut result) => {
                    result.hooks = push_hooks_branch(repo, &remote)?;
                    return Ok(result);
                }
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
    read_queue_file(repo)?;
    let branch = STATE_BRANCH.to_string();
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
            sha: rev_parse(repo, STATE_BRANCH)?,
            hooks: HooksPushAction::Absent,
        });
    };

    if local_sha == remote_sha {
        return Ok(PushResult {
            action: "up-to-date".into(),
            remote: remote.into(),
            branch,
            sha: local_sha,
            hooks: HooksPushAction::Absent,
        });
    }

    if is_ancestor(repo, &remote_sha, &local_sha)? {
        push_state_branch(repo, remote, &branch)?;
        return Ok(PushResult {
            action: "pushed".into(),
            remote: remote.into(),
            branch,
            sha: local_sha,
            hooks: HooksPushAction::Absent,
        });
    }

    if is_ancestor(repo, &local_sha, &remote_sha)? {
        set_state_branch(repo, &branch, &remote_sha)?;
        return Ok(PushResult {
            action: "fast-forwarded".into(),
            remote: remote.into(),
            branch,
            sha: remote_sha,
            hooks: HooksPushAction::Absent,
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
        sha: rev_parse(repo, STATE_BRANCH)?,
        hooks: HooksPushAction::Absent,
    })
}

/// The local patches origin's queue lacks, by layer, in queue order: not
/// on origin by id, nor by the internal PR that added them. A tooling
/// patch is carried only when origin has none.
pub(super) struct LocalOnly {
    pub tooling: Option<Patch>,
    pub upstream: Vec<Patch>,
    pub internal: Vec<Patch>,
}

impl LocalOnly {
    pub fn of(local_queue: &QueueState, remote_queue: &QueueState) -> Self {
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
        Self {
            tooling: local_queue
                .tooling
                .clone()
                .filter(|p| remote_queue.tooling.is_none() && should_carry(p)),
            upstream: local_queue
                .upstream
                .iter()
                .filter(|p| should_carry(p))
                .cloned()
                .collect(),
            internal: local_queue
                .internal
                .iter()
                .filter(|p| should_carry(p))
                .cloned()
                .collect(),
        }
    }
}

pub(super) fn restack_local_patches(
    repo: &Path,
    branch: &str,
    local_sha: &str,
    remote_sha: &str,
) -> Result<String> {
    let local_queue = queue_at(repo, local_sha)?;
    let remote_queue = queue_at(repo, remote_sha)?;
    let LocalOnly {
        tooling: carry_tooling,
        upstream: carry_upstream,
        internal: carry_internal,
    } = LocalOnly::of(&local_queue, &remote_queue);
    let carry: Vec<Patch> = carry_tooling
        .iter()
        .cloned()
        .chain(carry_upstream.iter().cloned())
        .chain(carry_internal.iter().cloned())
        .collect();
    let replay =
        locally_changed_shared_patches(repo, &local_queue, local_sha, &remote_queue, remote_sha)?;

    if carry.is_empty() && replay.is_empty() {
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
    for (patch, _) in &replay {
        paths.extend(patch_state_paths(repo, local_sha, &patch.id)?);
    }

    let outcome = (|| -> Result<()> {
        set_state_branch(repo, branch, remote_sha)?;
        restore_paths_from(repo, local_sha, &paths)?;
        let mut queue = read_queue_file(repo)?;
        for (patch, layer) in &replay {
            replace_patch(&mut queue, patch.clone(), *layer);
        }
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

/// Patches on both sides that local changed since the merge base (a drop,
/// approve, merged, transfer or refreshed patch file). Refuses when origin
/// changed the same patch too, rather than picking one side.
pub(super) fn locally_changed_shared_patches(
    repo: &Path,
    local: &QueueState,
    local_sha: &str,
    remote: &QueueState,
    remote_sha: &str,
) -> Result<Vec<(Patch, PatchLayer)>> {
    let Some(base_sha) = merge_base(repo, local_sha, remote_sha)? else {
        return Ok(Vec::new());
    };
    let base = queue_at(repo, &base_sha)?;
    let mut replay = Vec::new();
    for patch in local.all_patches() {
        let id = patch.id.as_str();
        let (Some(layer), Some(_)) = (local.layer_of(id), remote.layer_of(id)) else {
            continue;
        };
        if base.layer_of(id).is_none() {
            continue;
        }
        if !patch_side_changed(repo, &base, &base_sha, local, local_sha, id)? {
            continue;
        }
        if patch_side_changed(repo, local, local_sha, remote, remote_sha, id)?
            && patch_side_changed(repo, &base, &base_sha, remote, remote_sha, id)?
        {
            return Err(Error::msg(format!(
                "{id} changed both locally and on origin; run `git uplink reset` and redo the local change"
            )));
        }
        replay.push((patch.clone(), layer));
    }
    Ok(replay)
}

/// True when `id` differs between two `uplink/state` commits: its queue
/// layer, its queue entry, its patch file or its reports.
fn patch_side_changed(
    repo: &Path,
    before: &QueueState,
    before_sha: &str,
    after: &QueueState,
    after_sha: &str,
    id: &str,
) -> Result<bool> {
    if before.layer_of(id) != after.layer_of(id) {
        return Ok(true);
    }
    let old = before.all_patches().find(|p| p.id == id);
    let new = after.all_patches().find(|p| p.id == id);
    if serde_json::to_value(old)? != serde_json::to_value(new)? {
        return Ok(true);
    }
    let file = patch_path(id)?.to_string_lossy().into_owned();
    let (reports, _, _) = report_paths(id)?;
    Ok(!git_succeeds(
        repo,
        &[
            "diff", "--quiet", before_sha, after_sha, "--", &file, &reports,
        ],
    )?)
}

/// Patch file and report files for `id` as they exist at `sha`.
pub(super) fn patch_state_paths(repo: &Path, sha: &str, id: &str) -> Result<Vec<String>> {
    let file = patch_path(id)?.to_string_lossy().into_owned();
    let (reports, _, _) = report_paths(id)?;
    let listed = git_ok(
        repo,
        &["ls-tree", "-r", "--name-only", sha, "--", &file, &reports],
    )?;
    Ok(listed
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Puts `patch` in `layer`, in place when it is already there, otherwise
/// removed from its old layer and appended.
pub(super) fn replace_patch(queue: &mut QueueState, patch: Patch, layer: PatchLayer) {
    if queue.layer_of(&patch.id) == Some(layer) {
        if let Some(slot) = queue.all_patches_mut().find(|p| p.id == patch.id) {
            *slot = patch;
        }
        return;
    }
    if queue.tooling.as_ref().is_some_and(|p| p.id == patch.id) {
        queue.tooling = None;
    }
    queue.upstream.retain(|p| p.id != patch.id);
    queue.internal.retain(|p| p.id != patch.id);
    match layer {
        PatchLayer::Tooling => queue.tooling = Some(patch),
        PatchLayer::Upstream => queue.upstream.push(patch),
        PatchLayer::Internal => queue.internal.push(patch),
    }
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
    ensure_clean_worktree(repo, "an empty import")?;
    let merged = PatchMerged {
        via: MergeVia::EmptyRebase,
        at: stamp(),
        upstream_sha: Some(rev_parse(repo, "uplink/upstream")?),
    };
    set_merged(
        &mut queue,
        id,
        merged,
        merged_detail(MergeVia::EmptyRebase, None),
    )?;
    write_queue_file(repo, &queue)?;
    commit_queue(repo, &format!("uplink: empty apply {id}"))?;
    git(
        repo,
        &["checkout", "-f", "--quiet", &company_branch],
        GitOpts::default(),
    )?;
    Ok(())
}
