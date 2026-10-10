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
    /// With `upgrade`: what to do with the upgrade that paused on a patch.
    pub resume: Option<UpgradeResume>,
    pub adopt_groups: Option<Vec<AdoptGroup>>,
    /// `None` detects a TTY. Tests set `Some(false)` so adopt never opens the TUI.
    pub interactive: Option<bool>,
    pub progress: ProgressMode,
    /// Answers for `uplink.toml`, and the command to seed `preflight.sh` with.
    pub settings: SettingsFlags,
    /// Ask on the terminal for settings without an answer. Off by default so
    /// library callers and tests never block on stdin; the CLI turns it on
    /// when it has a terminal.
    pub ask_settings: bool,
}

/// What `init --upgrade` does with an upgrade that paused on a queued patch
/// which changes pack files and does not apply on the new pack. The checkout
/// was left on the new pack with the failed apply, as `git rebase` leaves a
/// commit that does not apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeResume {
    /// The files are resolved and added: what they hold becomes the patch.
    /// A patch resolved to what the new pack has is dropped.
    Continue,
    /// Drop the patch.
    Drop,
    /// Put the checkout back; nothing was written.
    Abort,
}

/// Queue produced by `init`. `tooling_changed` is set only by `--upgrade` when
/// the embedded pack's patch substance differed from the stored tooling patch.
#[derive(Debug)]
pub struct InitResult {
    pub queue: QueueState,
    pub tooling_changed: bool,
    pub report: InitReport,
    /// What the rebuild of an `--upgrade` that changed the tooling changes
    /// on company main.
    pub changes: Option<MainChanges>,
    /// Patches `--upgrade` dropped, id and title: the new pack holds what
    /// they change, or they no longer apply on it and the operator chose to.
    pub dropped: Vec<(String, String)>,
    /// Patches amended during `--upgrade` to apply on the new pack, by
    /// resolving the failed apply and `--continue`: id and title.
    pub amended: Vec<(String, String)>,
    /// Patches that change pack files on top of the new pack.
    pub overrides: Vec<ToolingOverride>,
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
        changes: None,
        dropped: Vec::new(),
        amended: Vec::new(),
        overrides: Vec::new(),
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
    let created = init_repo_with_progress(repo, config, &mut progress)?;
    // Before adoption: assessing adopted patches reads uplink.toml.
    ensure_hooks_step(repo, &created, HooksMode::Create, &opts, &mut progress)?;
    let queue = finish_first_init(repo, &opts, &mut progress)?;
    append_init_health_checks(repo, &queue, &mut progress);
    Ok(settled(queue, &progress))
}

pub(super) fn hydrate_from_origin(repo: &Path) -> Result<QueueState> {
    replace_state_from_origin(repo)?;
    let queue = read_queue_file(repo)?;
    require_stored_urls(&queue.config)?;
    ensure_configured_remotes(repo, &queue.config)?;
    refresh_upstream_ref(repo, COMPANY_REMOTE)?;
    require_origin_upstream(repo)?;
    ensure_company_branch_ref(repo, COMPANY_REMOTE, &queue.config.internal_branch)?;
    if let Some(forge) = queue.config.forge {
        ensure_hooks_branch(
            repo,
            forge,
            HooksMode::FetchOnly,
            &Settings::default(),
            None,
        )?;
    }
    read_queue_file(repo)
}

/// Create `uplink/hooks` locally when neither this clone nor origin has it.
/// With [`HooksMode::Upgrade`], also add pack files and settings the branch
/// lacks. What it writes is asked for first; existing values and files never
/// change.
pub(super) fn ensure_hooks_step(
    repo: &Path,
    queue: &QueueState,
    mode: HooksMode,
    opts: &InitOpts,
    progress: &mut StepProgress,
) -> Result<()> {
    let Some(forge) = queue.config.forge else {
        return progress.run_step("hooks-branch", "uplink/hooks branch", || {
            Ok(((), StepOutcome::skip("no forge recorded")))
        });
    };
    let questions = hooks_questions(repo, mode)?;
    let seed = if questions.preflight_script {
        preflight_seed(opts, queue.config.preflight_command.as_deref())?
    } else {
        None
    };
    let legacy = Settings {
        redact_keywords: queue.config.redact_keywords.clone(),
        internal_email_domains: queue.config.internal_email_domains.clone(),
        problem: None,
    };
    let (answers, unanswered) = answer_settings(
        &questions.settings,
        &opts.settings,
        &legacy,
        opts.ask_settings,
    )?;
    progress.run_step("hooks-branch", "uplink/hooks branch", || {
        let outcome = ensure_hooks_branch(repo, forge, mode, &answers, seed.as_deref())?;
        let wrote_settings = match &outcome {
            HooksOutcome::Created => true,
            HooksOutcome::Completed(paths) => paths.iter().any(|p| p == SETTINGS_PATH),
            _ => false,
        };
        let mut step = hooks_step_outcome(outcome);
        if wrote_settings && !unanswered.is_empty() {
            step.detail.push_str(&format!(
                ". Left empty in {SETTINGS_PATH}: {}; edit it on uplink/hooks or re-run with flags",
                unanswered.join(", ")
            ));
        }
        Ok(((), step))
    })
}

/// The command a new `preflight.sh` starts with: the flag, a question in a
/// terminal (offering `legacy`, the value an older `queue.json` held), else
/// `legacy`.
fn preflight_seed(opts: &InitOpts, legacy: Option<&str>) -> Result<Option<String>> {
    let seed = match &opts.settings.preflight {
        Some(command) => command.clone(),
        None if opts.ask_settings => crate::prompt::ask_line(
            "Preflight command, run on the export tree (empty for none)",
            legacy.unwrap_or_default(),
        )?,
        None => legacy.unwrap_or_default().to_string(),
    };
    let seed = seed.trim();
    Ok((!seed.is_empty()).then(|| seed.to_string()))
}

pub(super) fn missing_forge_error() -> Error {
    Error::msg("pass --forge github or --forge try-it-on-github when creating an uplink queue")
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

/// A queue that lives on origin comes with origin's `uplink/upstream`: the
/// base every export and preflight builds on, moved only by an accepted
/// sync. Without it, seeding from public upstream would put work on a base
/// nobody accepted, so init stops instead.
fn require_origin_upstream(repo: &Path) -> Result<()> {
    if has_ref(repo, UPSTREAM_REF)? || !has_ref(repo, &format!("{COMPANY_REMOTE}/{STATE_BRANCH}"))?
    {
        return Ok(());
    }
    Err(Error::msg(format!(
        "{COMPANY_REMOTE} has {STATE_BRANCH} but no {UPSTREAM_REF}. That branch is the accepted \
public base; it is not seeded from public upstream again. Push it back from a clone that has \
it (git push {COMPANY_REMOTE} {UPSTREAM_REF}), or redo the init setup."
    )))
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
        require_origin_upstream(repo)?;
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
    let mode = if opts.upgrade {
        HooksMode::Upgrade
    } else {
        HooksMode::Create
    };
    // Before resuming adoption: assessing adopted patches reads uplink.toml.
    ensure_hooks_step(repo, &queue, mode, opts, progress)?;
    resume_incomplete_init(repo, opts, progress)?;
    read_queue_file(repo)
}

pub(super) fn init_upgrade(
    repo: &Path,
    opts: &InitOpts,
    progress: &mut StepProgress,
) -> Result<InitResult> {
    if opts.resume.is_some() {
        // The upgrade that paused did the steps before the tooling.
        return upgrade_tooling(repo, opts.resume, progress);
    }
    if let Some(paused) = read_paused(repo)? {
        return Err(Error::msg(format!(
            "an upgrade is paused on {}. Continue it, drop the patch, or abort:\n  \
git uplink init --upgrade --continue\n  \
git uplink init --upgrade --drop\n  \
git uplink init --upgrade --abort",
            paused.patch_id
        )));
    }
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
                "queue.json has no forge. Re-run `git uplink init --upgrade --forge github` \
(or --forge try-it-on-github) to record it.",
            )
        })?;
        progress.run_step("record-forge", "Record forge in queue", || {
            queue.config.forge = Some(forge);
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: record forge")?;
            Ok(((), StepOutcome::pass(format!("forge {forge} recorded"))))
        })?;
    }
    upgrade_tooling(repo, None, progress)
}

/// The tooling step of `--upgrade`, and what the command reports of it.
fn upgrade_tooling(
    repo: &Path,
    resume: Option<UpgradeResume>,
    progress: &mut StepProgress,
) -> Result<InitResult> {
    crate::lock::with_queue_lock(repo, || {
        // The command prints what changed, so the step line does not.
        let outcome = progress.run_step("upgrade-tooling", "Refresh forge tooling", || {
            Ok(match prepare_tooling_upgrade(repo, resume)? {
                Prepared::Ready(ready) => (
                    Ok(Some(finish_tooling_upgrade(repo, *ready)?)),
                    StepOutcome::pass(""),
                ),
                Prepared::Unchanged => (Ok(None), StepOutcome::pass("")),
                Prepared::Aborted => (Ok(None), StepOutcome::skip("aborted")),
                Prepared::Paused(paused) => {
                    let step = StepOutcome::warn(format!("paused on {}", paused.patch_id));
                    (Err(paused), step)
                }
            })
        })?;
        let mut result = settled(read_queue_file(repo)?, progress);
        match outcome {
            Err(paused) => return Err(Error::msg(paused.instructions())),
            Ok(None) => {}
            Ok(Some(upgraded)) => {
                result.tooling_changed = true;
                result.changes = upgraded.changes;
                result.dropped = upgraded.dropped;
                result.amended = upgraded.amended;
                result.overrides = upgraded.overrides;
            }
        }
        append_init_health_checks(repo, &result.queue, progress);
        result.report = InitReport::from_checks(progress.checks().to_vec());
        Ok(result)
    })
}

/// A patch amended to apply on the new pack, not written yet.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmendedPatch {
    id: String,
    title: String,
    contents: String,
    assess: AssessReport,
}

/// An upgrade that stopped on a patch, in `.git/uplink-upgrade.json`: what
/// was settled before it and where the checkout was. Nothing of it is on
/// uplink/state yet.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PausedUpgrade {
    /// The checkout the upgrade started from, as [`checkout_identity`] gives it.
    original: String,
    original_sha: String,
    dropped: Vec<(String, String)>,
    amended: Vec<AmendedPatch>,
    patch_id: String,
    patch_title: String,
    /// The commit the patch failed to apply on.
    onto: String,
    files: Vec<String>,
    /// An upstream-bound patch is amended with `git uplink amend`, not here.
    can_amend: bool,
}

impl PausedUpgrade {
    fn instructions(&self) -> String {
        let id = &self.patch_id;
        let resolve = if self.can_amend {
            "Resolve them, git add them, then:  git uplink init --upgrade --continue\n\
Drop the patch instead:            git uplink init --upgrade --drop\n\
Stop, changing nothing:            git uplink init --upgrade --abort\n\
A patch resolved to what the new pack has is dropped."
        } else {
            "It is upstream-bound, so it is not amended here.\n\
Drop the patch:          git uplink init --upgrade --drop\n\
Stop, changing nothing:  git uplink init --upgrade --abort"
        };
        format!(
            "upgrade paused: {id} (\"{}\") changes files of the tooling pack and does not apply on the new pack.\n\
The checkout holds the new pack with the failed apply. In conflict:\n  {}\n\
{resolve}\n\
A dropped patch stays in .uplink/patches/{id}.patch.",
            self.patch_title,
            self.files.join("\n  ")
        )
    }
}

fn paused_path(repo: &Path) -> PathBuf {
    repo.join(".git/uplink-upgrade.json")
}

fn read_paused(repo: &Path) -> Result<Option<PausedUpgrade>> {
    match fs::read_to_string(paused_path(repo)) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Takes up a paused upgrade: settles the patch it stopped on as `resume`
/// says and puts the checkout back. `None` after an abort.
fn resume_paused(repo: &Path, resume: UpgradeResume) -> Result<Option<PausedUpgrade>> {
    let Some(mut paused) = read_paused(repo)? else {
        return Err(Error::msg(
            "no upgrade is paused; run git uplink init --upgrade",
        ));
    };
    let patch = (paused.patch_id.clone(), paused.patch_title.clone());
    match resume {
        UpgradeResume::Abort => {}
        UpgradeResume::Drop => paused.dropped.push(patch),
        UpgradeResume::Continue if !paused.can_amend => {
            return Err(Error::msg(paused.instructions()));
        }
        UpgradeResume::Continue => {
            assert_resolution_clean(repo)?;
            let queue = read_queue_file(repo)?;
            let stored = get_patch(&queue, &paused.patch_id)?;
            commit_resolution(repo, &paused.onto, &company_commit_message(stored))?;
            if rev_parse(repo, "HEAD")? == paused.onto {
                paused.dropped.push(patch);
            } else {
                let assess = assess_from_message(
                    repo,
                    &queue,
                    &paused.onto,
                    "HEAD",
                    &stored_commit_message(stored),
                    Some(&stored.title),
                    PatchIntent::InternalOnly,
                )?;
                paused.amended.push(AmendedPatch {
                    id: patch.0,
                    title: patch.1,
                    contents: format_patch_at_head(repo)?,
                    assess,
                });
            }
        }
    }
    restore_checkout(repo, &paused.original, &paused.original_sha)?;
    ensure_state_worktree(repo)?;
    fs::remove_file(paused_path(repo))?;
    Ok((resume != UpgradeResume::Abort).then_some(paused))
}

/// A tooling refresh that is ready to be written.
struct ReadyUpgrade {
    plan: crate::tooling::ToolingPlan,
    base: Option<ChangeBase>,
    dropped: Vec<(String, String)>,
    amended: Vec<AmendedPatch>,
    overrides: Vec<ToolingOverride>,
}

/// How far `--upgrade` got with the tooling patch before writing anything.
enum Prepared {
    /// The stored patch already holds the embedded pack.
    Unchanged,
    /// A paused upgrade was aborted.
    Aborted,
    /// A queued patch that changes pack files does not apply on the new
    /// pack. The checkout is left on the failed apply.
    Paused(Box<PausedUpgrade>),
    Ready(Box<ReadyUpgrade>),
}

/// What `--upgrade` did besides refreshing the tooling patch.
struct Upgraded {
    changes: Option<MainChanges>,
    dropped: Vec<(String, String)>,
    amended: Vec<(String, String)>,
    overrides: Vec<ToolingOverride>,
}

fn upgrade_reason(what: &str) -> String {
    format!(
        "{what} by init --upgrade for the tooling pack of git-uplink {}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Synthesizes the tooling patch from the embedded pack and replays the
/// queue on it, writing nothing to uplink/state. A patch that changes pack
/// files is dealt with here: one the new pack makes empty is dropped, and
/// one that no longer applies pauses the upgrade on the failed apply, for
/// `resume` to settle in the next run. The replay starts over after that,
/// with what was settled so far.
fn prepare_tooling_upgrade(repo: &Path, resume: Option<UpgradeResume>) -> Result<Prepared> {
    let (dropped, amended) = match resume {
        None => (Vec::new(), Vec::new()),
        Some(resume) => match resume_paused(repo, resume)? {
            Some(paused) => (paused.dropped, paused.amended),
            None => return Ok(Prepared::Aborted),
        },
    };
    let Some(plan) = crate::tooling::plan_tooling_refresh(repo)? else {
        return Ok(Prepared::Unchanged);
    };
    let queue = read_queue_file(repo)?;
    ensure_clean_worktree(repo, "an upgrade")?;
    let base = ChangeBase::of_company(repo, &queue.config.internal_branch)?;
    let mut ready = ReadyUpgrade {
        plan,
        base,
        dropped,
        amended,
        overrides: Vec::new(),
    };
    // A tooling patch that is new, or moves into the tooling layer, has no
    // place in the queue to replay it from yet.
    if !queue.is_tooling(&ready.plan.id) {
        return Ok(Prepared::Ready(Box::new(ready)));
    }
    let mut pack = patch_paths(&ready.plan.formatted);
    pack.extend(tooling_paths(repo, &queue)?);
    let in_pack = |id: &str| -> Result<Vec<String>> {
        Ok(stored_patch_paths(repo, id)?
            .intersection(&pack)
            .cloned()
            .collect())
    };

    let mut trial_queue = queue.clone();
    for (id, _) in &ready.dropped {
        mark_dropped(&mut trial_queue, id, "trial")?;
    }
    let mut replace = vec![(ready.plan.id.clone(), ready.plan.formatted.clone())];
    replace.extend(
        ready
            .amended
            .iter()
            .map(|patch| (patch.id.clone(), patch.contents.clone())),
    );
    let (original, original_sha) = checkout_identity(repo)?;
    let mut paused = None;
    let trial = trial_replay(
        repo,
        &trial_queue,
        UPSTREAM_REF,
        &replace,
        |patch, files| {
            let touched = in_pack(&patch.id)?;
            // Any other conflict is recorded by the rebuild, as always.
            if touched.is_empty() {
                return Ok(false);
            }
            let pause = PausedUpgrade {
                original,
                original_sha,
                dropped: ready.dropped.clone(),
                amended: ready.amended.clone(),
                patch_id: patch.id.clone(),
                patch_title: patch.title.clone(),
                onto: rev_parse(repo, "HEAD")?,
                files: if files.is_empty() {
                    touched
                } else {
                    files.to_vec()
                },
                can_amend: !queue.is_upstream(&patch.id),
            };
            fs::write(paused_path(repo), serde_json::to_string_pretty(&pause)?)?;
            paused = Some(pause);
            Ok(true)
        },
    )?;
    if let Some(paused) = paused {
        return Ok(Prepared::Paused(Box::new(paused)));
    }

    for id in &trial.empty {
        let touched = stored_patch_paths(repo, id)?;
        if !queue.is_tooling(id)
            && !queue.is_upstream(id)
            && !touched.is_empty()
            && touched.is_subset(&pack)
        {
            ready
                .dropped
                .push((id.clone(), get_patch(&queue, id)?.title.clone()));
        }
    }
    for id in trial.applied.iter().filter(|id| !queue.is_tooling(id)) {
        let files = in_pack(id)?;
        if !files.is_empty() {
            ready.overrides.push(ToolingOverride {
                id: id.clone(),
                title: get_patch(&queue, id)?.title.clone(),
                files,
            });
        }
    }
    Ok(Prepared::Ready(Box::new(ready)))
}

/// Writes what [`prepare_tooling_upgrade`] settled, in one commit on
/// uplink/state, and rebuilds.
fn finish_tooling_upgrade(repo: &Path, ready: ReadyUpgrade) -> Result<Upgraded> {
    let ReadyUpgrade {
        plan,
        base,
        dropped,
        amended,
        overrides,
    } = ready;
    for patch in &amended {
        fs::write(repo.join(patch_path(&patch.id)?), &patch.contents)?;
    }
    crate::tooling::commit_tooling_refresh(repo, plan, |queue| {
        for (id, _) in &dropped {
            mark_dropped(queue, id, &upgrade_reason("Dropped"))?;
        }
        for amended in &amended {
            let stable = stable_patch_id_from_contents(repo, &amended.contents)?;
            let patch = get_patch_mut(queue, &amended.id)?;
            patch.assess = Some(amended.assess.clone());
            patch.conflict = None;
            patch.patch_id_stable = Some(stable);
            add_event(patch, "amended", upgrade_reason("Amended"));
        }
        Ok(())
    })?;
    rebuild_once(repo, None, false)?;
    let queue = read_queue_file(repo)?;
    let changes = match &base {
        Some(base) => Some(main_changes(
            repo,
            &queue,
            base,
            &queue.config.internal_branch,
        )?),
        None => None,
    };
    Ok(Upgraded {
        changes,
        dropped,
        amended: amended
            .into_iter()
            .map(|patch| (patch.id, patch.title))
            .collect(),
        overrides,
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
            rebuild_once(repo, None, false)?;
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
