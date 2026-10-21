use std::sync::Arc;

use super::*;

use crate::adopt::BranchRun;
pub use crate::adopt::{
    AdoptDecider, AdoptDecision, AdoptFailure, AdoptFailureKind, AdoptResume, BranchAdopted,
    BranchAdoption, RunKind,
};
use crate::lock::with_run_lock;
use crate::prompt::Choice;
use crate::tui::TuiOutcome;

/// The branch a run builds company main with its queue on when it ends.
pub const PREVIEW_BRANCH: &str = "uplink/preview/verify";

#[derive(Clone, Default)]
pub struct AdoptBranchOpts {
    /// Local branch (or any local rev) to adopt commits from. Not used to resume.
    pub branch: String,
    /// Groups to adopt; commits left out of every group are skipped.
    pub groups: Option<Vec<AdoptGroup>>,
    /// `None` detects a terminal.
    pub interactive: Option<bool>,
    /// Leave `uplink/adopt-test/*` at upstream + dependencies + each patch.
    pub keep_test_branches: bool,
    /// Where the groups are written; `None` for [`adopt::groups_file_path`].
    pub write_groups: Option<PathBuf>,
    /// Takes up the run in progress instead of starting one.
    pub resume: Option<AdoptResume>,
    /// Says what to do with a group that fails. `None` asks in a terminal
    /// window, and fails the run elsewhere.
    pub decide: Option<AdoptDecider>,
    /// Do not run a failing `preflight.sh` again with queued upstream
    /// patches to find the ones a group needs. Faster; dependencies are then
    /// only what the group needs to apply.
    pub no_preflight_deps: bool,
    /// Do not build [`PREVIEW_BRANCH`] when the run ends.
    pub no_preview: bool,
    /// A line per group on stderr: a spinner while it is checked, then ✓ or ✗.
    pub progress: ProgressMode,
}

/// The preview a run builds when it ends having added patches.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPreview {
    pub branch: String,
    /// Why the preview could not be built; the run's patches stay queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdoptBranchResult {
    pub kind: RunKind,
    pub branch: String,
    /// `None` when the groups were only written, or the run was aborted.
    pub adoption: Option<BranchAdoption>,
    /// The branch forked from an older upstream than uplink/upstream.
    pub behind: bool,
    /// The groups as decided, for `--groups` to replay. Empty for restack.
    pub groups_file: PathBuf,
    /// `--abort` (uplink/state reset) or `--abort-keep` ended the run.
    pub aborted: Option<AdoptResume>,
    pub preview: Option<RunPreview>,
}

/// Adopts picked commits of another branch into the queue, after the
/// patches already there. Runs locally only: every ref must already be in
/// this clone, nothing is fetched or pushed, and company main is untouched.
/// Writes one uplink/state commit per adopted patch, then builds
/// [`PREVIEW_BRANCH`].
pub fn adopt_branch(repo: &Path, opts: AdoptBranchOpts) -> Result<AdoptBranchResult> {
    crate::git::with_offline(|| adopt_branch_offline(repo, opts))
}

fn adopt_branch_offline(repo: &Path, opts: AdoptBranchOpts) -> Result<AdoptBranchResult> {
    if let Some(resume) = opts.resume {
        return resume_run(repo, resume, &opts);
    }
    crate::run::refuse_while_running(repo)?;
    let queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    let interactive = opts.interactive.unwrap_or_else(adopt::stdin_is_tty);
    let analysis = check_refs(repo, &queue, &opts.branch, &company_branch)?;
    // Absolute, so a paused run finds it from any directory.
    let groups_file = match &opts.write_groups {
        Some(path) if path.is_relative() => std::env::current_dir()?.join(path),
        Some(path) => path.clone(),
        None => adopt::groups_file_path(repo, &opts.branch)?,
    };
    let (groups, write_only) = match opts.groups.clone() {
        Some(groups) => (groups, false),
        None if interactive => {
            match crate::tui::run_adopt(repo, &queue, &analysis, Some(&groups_file))? {
                TuiOutcome::Adopt(groups) => (groups, false),
                TuiOutcome::WriteAndExit(groups) => (groups, true),
            }
        }
        None => {
            return Err(Error::msg(
                "pass --groups <file> or re-run git uplink adopt in a terminal to pick commits",
            ));
        }
    };
    let groups = adopt::resolve_groups(&analysis, &groups)?;
    adopt::write_groups_file(&groups_file, &groups)?;
    let mut result = AdoptBranchResult {
        kind: RunKind::Branch,
        branch: opts.branch.clone(),
        adoption: None,
        behind: analysis.behind,
        groups_file: groups_file.clone(),
        aborted: None,
        preview: None,
    };
    if write_only {
        return Ok(result);
    }
    let run = BranchRun::new(
        repo,
        RunKind::Branch,
        opts.branch.clone(),
        groups_file,
        groups,
        opts.keep_test_branches,
        !opts.no_preflight_deps,
    )?;
    let decide = decider(&opts);
    let adoption = with_run_lock(repo, || {
        adopt::run_branch(
            repo,
            Some(&analysis),
            run,
            &ScriptVerdict::Run,
            decide.as_ref(),
            opts.progress,
        )
    })?;
    result.preview = preview_after(repo, &adoption, opts.no_preview);
    result.adoption = Some(adoption);
    Ok(result)
}

/// The operator's decider: the one passed in, else a window in a terminal.
fn decider(opts: &AdoptBranchOpts) -> Option<AdoptDecider> {
    let interactive = opts.interactive.unwrap_or_else(adopt::stdin_is_tty);
    opts.decide
        .clone()
        .or_else(|| interactive.then(prompt_decider))
}

/// Takes up the run in progress, whatever started it (`init`, `adopt` or
/// `restack`), and goes on with it unless it was aborted.
pub(super) fn resume_run(
    repo: &Path,
    resume: AdoptResume,
    opts: &AdoptBranchOpts,
) -> Result<AdoptBranchResult> {
    let decide = decider(opts);
    let mut result = with_run_lock(repo, || {
        let run = adopt::resume_branch(repo, resume)?;
        let mut result = AdoptBranchResult {
            kind: run.kind,
            branch: run.branch.clone(),
            adoption: None,
            behind: false,
            groups_file: run.groups_file.clone().unwrap_or_default(),
            aborted: matches!(resume, AdoptResume::Abort | AdoptResume::AbortKeep)
                .then_some(resume),
            preview: None,
        };
        if result.aborted.is_some() {
            return Ok(result);
        }
        let analysis = match run.kind {
            RunKind::Init => {
                let analysis = adopt::analyze_ahead(repo)?;
                if analysis.behind {
                    return Err(adopt::behind_error(&analysis));
                }
                Some(analysis)
            }
            RunKind::Branch => {
                let queue = read_queue_file(repo)?;
                let company_branch = queue.config.internal_branch.clone();
                Some(check_refs(repo, &queue, &run.branch, &company_branch)?)
            }
            RunKind::Restack => None,
        };
        result.behind = analysis.as_ref().is_some_and(|a| a.behind);
        result.adoption = Some(adopt::run_branch(
            repo,
            analysis.as_ref(),
            run,
            &ScriptVerdict::Run,
            decide.as_ref(),
            opts.progress,
        )?);
        Ok(result)
    })?;
    if let Some(adoption) = &result.adoption {
        result.preview = preview_after(repo, adoption, opts.no_preview);
    }
    Ok(result)
}

/// Builds [`PREVIEW_BRANCH`] when the run added patches: company main as
/// the queue now rebuilds it, without moving company main.
pub(super) fn preview_after(
    repo: &Path,
    adoption: &BranchAdoption,
    no_preview: bool,
) -> Option<RunPreview> {
    if no_preview || adoption.adopted.is_empty() {
        return None;
    }
    let built = rebuild_with(
        repo,
        RebuildOpts {
            branch: Some(PREVIEW_BRANCH.to_string()),
            ..RebuildOpts::default()
        },
    );
    Some(RunPreview {
        branch: PREVIEW_BRANCH.to_string(),
        error: built.err().map(|err| err.to_string()),
    })
}

/// What to do after a run added patches, given the preview it built.
pub fn adopted_next_steps(preview: Option<&RunPreview>, company_branch: &str) -> String {
    let publish = "After verification: git uplink rebuild --push";
    match preview {
        Some(RunPreview {
            branch,
            error: None,
        }) => format!(
            "Company {company_branch} is unchanged. Built the preview {branch}.\n\
Inspect with: git diff {company_branch} {branch}\n{publish}"
        ),
        Some(RunPreview {
            branch,
            error: Some(error),
        }) => format!(
            "Company {company_branch} is unchanged. The preview {branch} could not be built:\n  {}\n\
Fix the queue, then: git uplink rebuild --branch {branch}\n{publish}",
            error.lines().next().unwrap_or_default()
        ),
        None => format!(
            "Company {company_branch} is unchanged.\n\
Preview with: git uplink rebuild --branch {PREVIEW_BRANCH}\n\
Inspect with: git diff {company_branch} {PREVIEW_BRANCH}\n{publish}"
        ),
    }
}

/// Every ref adopt needs is local, and the commits of `branch` to adopt.
fn check_refs(
    repo: &Path,
    queue: &QueueState,
    branch: &str,
    company_branch: &str,
) -> Result<adopt::AheadAnalysis> {
    require_local(repo, branch)?;
    require_local(repo, UPSTREAM_REF)?;
    require_local(repo, company_branch)?;
    if queue.config.forge.is_some() && crate::hooks::hooks_source(repo)?.is_none() {
        return Err(Error::msg(format!(
            "{HOOKS_BRANCH} is not available locally, so preflight has nothing to run; \
fetch it first: git fetch origin {HOOKS_BRANCH}:{HOOKS_BRANCH}"
        )));
    }
    let analysis = adopt::analyze_branch(repo, branch, company_branch)?;
    if analysis.commits.is_empty() {
        return Err(Error::msg(format!(
            "nothing to adopt from {branch}: it has no commits since it left uplink/upstream"
        )));
    }
    Ok(analysis)
}

fn require_local(repo: &Path, rev: &str) -> Result<()> {
    if has_ref(repo, rev)? {
        return Ok(());
    }
    Err(Error::msg(format!(
        "'{rev}' is not available locally, and adopt makes no network calls; \
fetch it first: git fetch origin {rev}:{rev}"
    )))
}

/// Asks in a terminal window that shows what failed.
pub(super) fn prompt_decider() -> AdoptDecider {
    Arc::new(|failure: &AdoptFailure| {
        let button = |label, key| Choice { label, key };
        let mut options = Vec::new();
        if failure.kind == AdoptFailureKind::Preflight {
            options.push((button("Ignore", 'i'), AdoptDecision::IgnorePreflight));
        }
        options.push((button("Internal-only", 'n'), AdoptDecision::InternalOnly));
        if failure.can_amend {
            options.push((button("Amend", 'a'), AdoptDecision::Amend));
        }
        if failure.can_leave_out {
            options.push((button("Skip", 's'), AdoptDecision::Skip));
            options.push((button("Stop", 't'), AdoptDecision::Stop));
        }
        options.push((button("Abort", 'b'), AdoptDecision::Abort));
        crate::tui::choose_on_failure(failure, &options)
    })
}
