use std::sync::Arc;

use super::*;

use crate::adopt::BranchRun;
pub use crate::adopt::{
    AdoptDecider, AdoptDecision, AdoptFailure, AdoptFailureKind, AdoptResume, BranchAdopted,
    BranchAdoption,
};
use crate::prompt::Choice;
use crate::tui::TuiOutcome;

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
    /// Takes up a paused run instead of starting one.
    pub resume: Option<AdoptResume>,
    /// Says what to do with a group that fails. `None` asks with buttons in
    /// a terminal, and fails the run elsewhere.
    pub decide: Option<AdoptDecider>,
}

#[derive(Debug, Clone)]
pub struct AdoptBranchResult {
    pub branch: String,
    /// `None` when the groups were only written, or the run was aborted.
    pub adoption: Option<BranchAdoption>,
    /// The branch forked from an older upstream than uplink/upstream.
    pub behind: bool,
    /// The groups as decided, for `--groups` to replay.
    pub groups_file: PathBuf,
    pub aborted: bool,
}

/// Adopts picked commits of another branch into the queue, after the
/// patches already there. Runs locally only: every ref must already be in
/// this clone, nothing is fetched or pushed, and company main is untouched.
/// Writes one uplink/state commit.
pub fn adopt_branch(repo: &Path, opts: AdoptBranchOpts) -> Result<AdoptBranchResult> {
    crate::git::with_offline(|| adopt_branch_offline(repo, opts))
}

fn adopt_branch_offline(repo: &Path, opts: AdoptBranchOpts) -> Result<AdoptBranchResult> {
    let queue = read_queue_file(repo)?;
    let company_branch = queue.config.internal_branch.clone();
    let interactive = opts.interactive.unwrap_or_else(adopt::stdin_is_tty);
    let decide = opts
        .decide
        .clone()
        .or_else(|| interactive.then(prompt_decider));
    if let Some(resume) = opts.resume {
        return with_queue_lock(repo, || {
            let run = adopt::resume_branch(repo, resume)?;
            let mut result = AdoptBranchResult {
                branch: run.branch.clone(),
                adoption: None,
                behind: false,
                groups_file: run.groups_file.clone(),
                aborted: resume == AdoptResume::Abort,
            };
            if result.aborted {
                return Ok(result);
            }
            let analysis = check_refs(repo, &queue, &run.branch, &company_branch)?;
            result.behind = analysis.behind;
            result.adoption = Some(adopt::run_branch(
                repo,
                &analysis,
                run,
                &ScriptVerdict::Run,
                decide.as_ref(),
            )?);
            Ok(result)
        });
    }
    adopt::refuse_when_paused(repo)?;
    let analysis = check_refs(repo, &queue, &opts.branch, &company_branch)?;
    let (groups, write_only) = match opts.groups {
        Some(groups) => (groups, false),
        None if interactive => match crate::tui::run_adopt(repo, &queue, &analysis)? {
            TuiOutcome::Adopt(groups) => (groups, false),
            TuiOutcome::WriteAndExit(groups) => (groups, true),
        },
        None => {
            return Err(Error::msg(
                "pass --groups <file> or re-run git uplink adopt in a terminal to pick commits",
            ));
        }
    };
    let groups = adopt::resolve_groups(&analysis, &groups)?;
    // Absolute, so a paused run finds it from any directory.
    let groups_file = match opts.write_groups {
        Some(path) if path.is_relative() => std::env::current_dir()?.join(path),
        Some(path) => path,
        None => adopt::groups_file_path(repo, &opts.branch)?,
    };
    adopt::write_groups_file(&groups_file, &groups)?;
    let mut result = AdoptBranchResult {
        branch: opts.branch.clone(),
        adoption: None,
        behind: analysis.behind,
        groups_file: groups_file.clone(),
        aborted: false,
    };
    if write_only {
        return Ok(result);
    }
    let run = BranchRun::new(opts.branch, groups_file, groups, opts.keep_test_branches);
    result.adoption = Some(with_queue_lock(repo, || {
        adopt::run_branch(repo, &analysis, run, &ScriptVerdict::Run, decide.as_ref())
    })?);
    Ok(result)
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

/// Asks with a row of buttons in the terminal.
fn prompt_decider() -> AdoptDecider {
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
        options.push((button("Skip", 's'), AdoptDecision::Skip));
        options.push((button("Abort", 'q'), AdoptDecision::Abort));
        let what = match failure.kind {
            AdoptFailureKind::Apply => "does not apply onto public upstream",
            AdoptFailureKind::Preflight => "fails preflight.sh on public upstream",
        };
        if let Some(first) = failure.message.lines().next() {
            eprintln!("{first}");
        }
        if !failure.can_amend {
            eprintln!("(Amend needs a checkout without uncommitted changes.)");
        }
        let question = format!(
            "Group {} (\"{}\") {what}. ←/→ to choose, Enter to confirm, Esc aborts.",
            failure.group, failure.title
        );
        let choices: Vec<Choice> = options.iter().map(|(choice, _)| *choice).collect();
        Ok(match crate::prompt::choose(&question, &choices)? {
            Some(i) => options[i].1,
            None => AdoptDecision::Abort,
        })
    })
}
