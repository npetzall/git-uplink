use super::*;

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
    pub progress: ProgressMode,
}

/// Queue produced by `init`. `tooling_changed` is set only by `--upgrade` when
/// the embedded pack's patch substance differed from the stored tooling patch.
#[derive(Debug)]
pub struct InitResult {
    pub queue: QueueState,
    pub tooling_changed: bool,
    pub report: InitReport,
}

impl std::ops::Deref for InitResult {
    type Target = QueueState;

    fn deref(&self) -> &QueueState {
        &self.queue
    }
}

pub(super) fn settled(queue: QueueState, progress: &StepProgress) -> InitResult {
    InitResult {
        queue,
        tooling_changed: false,
        report: InitReport::from_checks(progress.checks().to_vec()),
    }
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

pub(super) fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

pub(super) fn config_from_opts(opts: &InitOpts) -> QueueConfig {
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

/// Create or hydrate an uplink queue. No CLI args fetches `origin` `uplink/state`,
/// `uplink/upstream`, and the configured company branch, materializes that local
/// ref without checking it out, and reconstitutes remotes from stored URLs. Args
/// create the queue when state is missing, or sanity-check an existing queue.
/// `--forge` is required when creating a queue. `--upgrade` amends the stored
/// forge pack in place.
pub fn init(repo: &Path, opts: InitOpts) -> Result<InitResult> {
    let mut progress = StepProgress::from_mode(opts.progress);
    if opts.upgrade {
        return init_upgrade(repo, &opts, &mut progress);
    }
    if !opts.has_args() && opts.forge.is_none() {
        return Ok(settled(hydrate_from_origin(repo)?, &progress));
    }
    progress.run_step("sync-origin-state", "Sync state from origin", || {
        try_replace_state_from_origin(repo)?;
        Ok((
            (),
            StepOutcome::pass("origin uplink/state synced when present"),
        ))
    })?;
    if state_exists(repo)? {
        let queue = init_existing(repo, &opts, &mut progress)?;
        append_init_health_checks(repo, &queue, &mut progress);
        return Ok(settled(queue, &progress));
    }
    let forge = opts.forge.ok_or_else(missing_forge_error)?;
    let mut config = config_from_opts(&opts);
    config.forge = Some(forge);
    init_repo_with_progress(repo, config, &mut progress)?;
    let queue = finish_first_init(repo, &opts, &mut progress)?;
    ensure_hooks_step(repo, &queue, HooksMode::Create, &mut progress)?;
    append_init_health_checks(repo, &queue, &mut progress);
    Ok(settled(queue, &progress))
}

pub(super) fn hydrate_from_origin(repo: &Path) -> Result<QueueState> {
    replace_state_from_origin(repo)?;
    let queue = read_queue_file(repo)?;
    require_stored_urls(&queue.config)?;
    ensure_configured_remotes(repo, &queue.config)?;
    refresh_upstream_ref(repo, COMPANY_REMOTE)?;
    ensure_company_branch_ref(repo, COMPANY_REMOTE, &queue.config.internal_branch)?;
    if let Some(forge) = queue.config.forge {
        ensure_hooks_branch(repo, forge, HooksMode::FetchOnly)?;
    }
    Ok(queue)
}

/// Create `uplink/hooks` locally when neither this clone nor origin has it.
/// With [`HooksMode::Upgrade`], also add pack files the branch lacks. Existing
/// files are never changed.
pub(super) fn ensure_hooks_step(
    repo: &Path,
    queue: &QueueState,
    mode: HooksMode,
    progress: &mut StepProgress,
) -> Result<()> {
    progress.run_step("hooks-branch", "uplink/hooks branch", || {
        let Some(forge) = queue.config.forge else {
            return Ok(((), StepOutcome::skip("no forge recorded")));
        };
        let outcome = ensure_hooks_branch(repo, forge, mode)?;
        Ok(((), hooks_step_outcome(outcome)))
    })
}

pub(super) fn missing_forge_error() -> Error {
    Error::msg("pass --forge ghec or --forge example-github when creating an uplink queue")
}

pub(super) fn require_stored_urls(config: &QueueConfig) -> Result<()> {
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

pub(super) fn check_name(
    out: &mut Vec<String>,
    field: &str,
    requested: Option<&str>,
    stored: &str,
) {
    if let Some(requested) = requested
        && requested != stored
    {
        out.push(format!(
            "  {field}: stored \"{stored}\", requested \"{requested}\""
        ));
    }
}

pub(super) fn append_init_health_checks(
    repo: &Path,
    queue: &QueueState,
    progress: &mut StepProgress,
) {
    if !progress.enabled() {
        return;
    }
    let ids: std::collections::HashSet<String> =
        progress.checks().iter().map(|c| c.id.clone()).collect();
    if !ids.contains("urls-recorded") {
        progress.run_check("urls-recorded", "URLs recorded", || {
            check_urls_recorded(queue)
        });
    }
    if !ids.contains("remotes-configured") {
        progress.run_check("remotes-configured", "Remotes configured", || {
            check_remotes_configured(repo, queue)
        });
    }
    if !ids.contains("upstream-ref") {
        progress.run_check("upstream-ref", "uplink/upstream ref", || {
            check_upstream_ref(repo).unwrap_or_else(|err| StepOutcome::fail(err.to_string()))
        });
    }
    if !ids.contains("forge-tooling") && queue.config.forge.is_some() {
        progress.run_check("forge-tooling", "Forge tooling patch", || {
            check_forge_tooling(repo, queue)
        });
    }
}

pub(super) fn init_needs_upstream_seed(repo: &Path) -> Result<bool> {
    Ok(!has_ref(repo, UPSTREAM_REF)?)
}

pub(super) fn init_needs_tooling(queue: &QueueState) -> bool {
    queue.config.forge.is_some() && queue.tooling.is_none()
}

pub(super) fn resume_incomplete_init(
    repo: &Path,
    opts: &InitOpts,
    progress: &mut StepProgress,
) -> Result<Option<QueueState>> {
    if adopt::has_adopt_from(repo)? {
        progress.run_step("resume-adopt", "Resume adoption", || {
            let queue = finish_adopt(repo, opts)?;
            Ok((queue, StepOutcome::pass("adoption resumed")))
        })?;
        return Ok(None);
    }

    let needs_upstream = init_needs_upstream_seed(repo)?;
    let queue = read_queue_file(repo)?;
    let needs_tooling = init_needs_tooling(&queue);
    if !needs_upstream && !needs_tooling {
        return Ok(None);
    }

    if needs_upstream {
        progress.run_step("resume-upstream-seed", "Seed uplink/upstream", || {
            let queue = read_queue_file(repo)?;
            if queue
                .config
                .upstream_url
                .as_deref()
                .is_none_or(|url| url.is_empty())
            {
                return Ok(((), StepOutcome::skip("no upstream URL recorded yet")));
            }
            ensure_configured_remotes(repo, &queue.config)?;
            let queue = read_queue_file(repo)?;
            let sha = fetch_upstream(repo, &queue)?;
            Ok((
                (),
                StepOutcome::pass(format!("uplink/upstream at {}", short_sha(&sha))),
            ))
        })?;
    }

    let queue = read_queue_file(repo)?;
    if init_needs_tooling(&queue) {
        finish_first_init(repo, opts, progress)?;
        return Ok(None);
    }

    Ok(None)
}

pub(super) fn init_existing(
    repo: &Path,
    opts: &InitOpts,
    progress: &mut StepProgress,
) -> Result<QueueState> {
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
        progress.run_step("update-urls", "Update recorded remote URLs", || {
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: update remote urls")?;
            Ok(((), StepOutcome::pass("queue.json updated")))
        })?;
        queue = read_queue_file(repo)?;
    }
    progress.run_step("remotes-configured", "Configure remotes", || {
        ensure_configured_remotes(repo, &queue.config)?;
        Ok(((), check_remotes_configured(repo, &queue)))
    })?;
    progress.run_step(
        "refresh-origin-upstream",
        "Refresh origin/uplink/upstream",
        || {
            refresh_upstream_ref(repo, COMPANY_REMOTE)?;
            Ok((
                (),
                StepOutcome::pass("origin tracking ref refreshed when present"),
            ))
        },
    )?;
    resume_incomplete_init(repo, opts, progress)?;
    let queue = read_queue_file(repo)?;
    let mode = if opts.upgrade {
        HooksMode::Upgrade
    } else {
        HooksMode::Create
    };
    ensure_hooks_step(repo, &queue, mode, progress)?;
    Ok(queue)
}

pub(super) fn init_upgrade(
    repo: &Path,
    opts: &InitOpts,
    progress: &mut StepProgress,
) -> Result<InitResult> {
    progress.run_step("sync-origin-state", "Sync state from origin", || {
        try_replace_state_from_origin(repo)?;
        Ok((
            (),
            StepOutcome::pass("origin uplink/state synced when present"),
        ))
    })?;
    if !state_exists(repo)? {
        return Err(Error::msg(
            "not initialized; run `git uplink init --upstream <url> --contrib <url> --forge <forge>` first",
        ));
    }
    init_existing(repo, opts, progress)?;
    let mut queue = read_queue_file(repo)?;
    if queue.config.forge.is_none() {
        let forge = opts.forge.ok_or_else(|| {
            Error::msg(
                "queue.json has no forge. Re-run `git uplink init --upgrade --forge ghec` \
(or --forge example-github) to record it.",
            )
        })?;
        progress.run_step("record-forge", "Record forge in queue", || {
            queue.config.forge = Some(forge);
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: record forge")?;
            Ok(((), StepOutcome::pass(format!("forge {forge} recorded"))))
        })?;
    }
    let (queue, tooling_changed) =
        progress.run_step("upgrade-tooling", "Refresh forge tooling", || {
            let (queue, changed) = write_tooling_patch(repo, true)?;
            let detail = if changed {
                "embedded forge pack updated"
            } else {
                "already up-to-date"
            };
            Ok(((queue, changed), StepOutcome::pass(detail)))
        })?;
    append_init_health_checks(repo, &queue, progress);
    Ok(InitResult {
        queue,
        tooling_changed,
        report: InitReport::from_checks(progress.checks().to_vec()),
    })
}

pub(super) fn ensure_tooling_patch(repo: &Path) -> Result<QueueState> {
    Ok(write_tooling_patch(repo, true)?.0)
}

pub(super) fn write_tooling_patch(
    repo: &Path,
    rebuild_if_changed: bool,
) -> Result<(QueueState, bool)> {
    crate::lock::with_queue_lock(repo, || {
        let refresh = crate::tooling::refresh_tooling_patch(repo)?;
        if rebuild_if_changed && refresh.changed {
            rebuild_once(repo)?;
        }
        Ok((read_queue_file(repo)?, refresh.changed))
    })
}

pub(super) fn finish_first_init(
    repo: &Path,
    opts: &InitOpts,
    progress: &mut StepProgress,
) -> Result<QueueState> {
    let analysis = progress.run_step(
        "adopt-analysis",
        "Analyze internal vs uplink/upstream",
        || {
            let analysis = adopt::analyze_ahead(repo)?;
            if analysis.behind {
                return Err(adopt::behind_error(&analysis));
            }
            let detail = if analysis.commits.is_empty() {
                "internal matches uplink/upstream".into()
            } else {
                format!("internal is {} commit(s) ahead", analysis.commits.len())
            };
            Ok((analysis, StepOutcome::pass(detail)))
        },
    )?;
    if analysis.commits.is_empty() {
        return progress.run_step("forge-tooling", "Install forge tooling", || {
            let queue = ensure_tooling_patch(repo)?;
            Ok((queue, StepOutcome::pass("forge tooling installed")))
        });
    }
    adopt::save_adopt_from(repo)?;
    write_tooling_patch(repo, false)?;
    progress.run_step(
        "forge-tooling",
        "Stage forge tooling before adoption",
        || Ok(((), StepOutcome::pass("forge tooling staged"))),
    )?;
    finish_adopt(repo, opts)
}

pub(super) fn finish_adopt(repo: &Path, opts: &InitOpts) -> Result<QueueState> {
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

pub(super) fn groups_for_adopt(
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
    let mut progress = StepProgress::from_mode(ProgressMode::Disabled);
    init_repo_with_progress(repo, config, &mut progress)
}

pub fn init_repo_with_progress(
    repo: &Path,
    config: QueueConfig,
    progress: &mut StepProgress,
) -> Result<QueueState> {
    let queue = progress.run_step("queue-created", "Create uplink queue", || {
        crate::repo::ensure_uplink_dirs(repo)?;
        let queue = QueueState::empty(config);
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: initialize patch queue")?;
        ensure_state_worktree(repo)?;
        let sha = git_ok(repo, &["rev-parse", "--short", STATE_BRANCH])?;
        Ok((queue, StepOutcome::pass(format!("uplink/state at {sha}"))))
    })?;
    progress.run_step("remotes-configured", "Configure remotes", || {
        ensure_configured_remotes(repo, &queue.config)?;
        Ok(((), check_remotes_configured(repo, &queue)))
    })?;
    progress.run_step("upstream-seeded", "Seed uplink/upstream", || {
        let has_head = git_succeeds(repo, &["rev-parse", "--verify", "HEAD"])?;
        let remotes = git_ok(repo, &["remote"]).unwrap_or_default();
        if remotes
            .split('\n')
            .any(|r| r == queue.config.upstream_remote)
        {
            let sha = fetch_upstream(repo, &queue)?;
            Ok((
                (),
                StepOutcome::pass(format!(
                    "fetched {} at {}",
                    queue.config.upstream_branch,
                    short_sha(&sha)
                )),
            ))
        } else if has_head {
            let target = if git_succeeds(repo, &["rev-parse", "--verify", "HEAD~1"])? {
                "HEAD~1"
            } else {
                "HEAD"
            };
            git(
                repo,
                &["branch", "-f", "uplink/upstream", target],
                GitOpts::default(),
            )?;
            Ok((
                (),
                StepOutcome::pass(format!("seeded uplink/upstream from {target}")),
            ))
        } else {
            Ok((
                (),
                StepOutcome::skip("no upstream remote or local commits to seed from"),
            ))
        }
    })?;
    Ok(queue)
}
