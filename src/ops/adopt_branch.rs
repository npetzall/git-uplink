use super::*;

pub use crate::adopt::{BranchAdopted, BranchAdoption};

#[derive(Clone, Default)]
pub struct AdoptBranchOpts {
    /// Local branch (or any local rev) to adopt commits from.
    pub branch: String,
    /// Groups to adopt; commits left out of every group are skipped.
    pub groups: Option<Vec<AdoptGroup>>,
    /// `None` detects a terminal.
    pub interactive: Option<bool>,
    /// Leave `uplink/adopt-test/*` at upstream + dependencies + each patch.
    pub keep_test_branches: bool,
}

#[derive(Debug, Clone)]
pub struct AdoptBranchResult {
    pub adoption: BranchAdoption,
    /// The branch forked from an older upstream than uplink/upstream.
    pub behind: bool,
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
    require_local(repo, &opts.branch)?;
    require_local(repo, UPSTREAM_REF)?;
    require_local(repo, &company_branch)?;
    if queue.config.forge.is_some() && crate::hooks::hooks_source(repo)?.is_none() {
        return Err(Error::msg(format!(
            "{HOOKS_BRANCH} is not available locally, so preflight has nothing to run; \
fetch it first: git fetch origin {HOOKS_BRANCH}:{HOOKS_BRANCH}"
        )));
    }
    let analysis = adopt::analyze_branch(repo, &opts.branch, &company_branch)?;
    if analysis.commits.is_empty() {
        return Err(Error::msg(format!(
            "nothing to adopt from {}: it has no commits since it left uplink/upstream",
            opts.branch
        )));
    }
    let groups = match opts.groups {
        Some(groups) => groups,
        None if opts.interactive.unwrap_or_else(adopt::stdin_is_tty) => {
            crate::tui::run_adopt(repo, &queue, &analysis)?
        }
        None => {
            return Err(Error::msg(
                "pass --groups <file> or re-run git uplink adopt in a terminal to pick commits",
            ));
        }
    };
    let adoption = adopt::apply_branch_groups(
        repo,
        &analysis,
        &groups,
        &opts.branch,
        &ScriptVerdict::Run,
        opts.keep_test_branches,
    )?;
    Ok(AdoptBranchResult {
        adoption,
        behind: analysis.behind,
    })
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
