use super::*;
use crate::repo::{file_history, show_at};

/// How a branch stands against company main.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RebaseState {
    /// Company main is in the branch.
    Current,
    /// The branch started from a commit company main still has.
    Behind,
    /// The branch started from a company main a rebuild has replaced.
    Replaced,
    /// Where the branch left company main cannot be told.
    Unknown,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebasePlan {
    pub state: RebaseState,
    /// The company branch.
    pub branch: String,
    pub head: String,
    /// Tip of the company branch on origin.
    pub target: String,
    /// The newest commit of a company main in the branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork_point: Option<String>,
    /// Commits the rebase replays.
    pub commits: u32,
    /// The rebase as a plain git command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Why the state is `unknown`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RebaseResult {
    pub plan: RebasePlan,
    pub rebased: bool,
}

/// Revisions of `previous-main.json` a plan reads before giving up.
const PREVIOUS_MAIN_REVISIONS: usize = 50;

/// Origin's tip of `branch`: fetched, or as last fetched when `fetch` is off.
fn origin_sha(repo: &Path, branch: &str, fetch: bool) -> Result<String> {
    if fetch {
        return fetch_tracking_sha(repo, COMPANY_REMOTE, branch);
    }
    let tracking = format!("{COMPANY_REMOTE}/{branch}");
    if !has_ref(repo, &tracking)? {
        return Err(Error::msg(format!(
            "{tracking} is not in this clone; run `git fetch {COMPANY_REMOTE}` first."
        )));
    }
    rev_parse(repo, &tracking)
}

/// What a rebase of `head` onto company main would do. With `fetch` it
/// fetches origin's company branch, `uplink/state` and `uplink/upstream`;
/// it changes nothing else.
pub fn rebase_plan(repo: &Path, head: &str, fetch: bool) -> Result<RebasePlan> {
    let state_sha = origin_sha(repo, STATE_BRANCH, fetch)?;
    let branch = queue_at(repo, &state_sha)?.config.internal_branch;
    let target = origin_sha(repo, &branch, fetch)?;
    origin_sha(repo, UPSTREAM_REF, fetch)?;
    let upstream = format!("{COMPANY_REMOTE}/{UPSTREAM_REF}");
    let head = rev_parse(repo, &format!("{head}^{{commit}}"))?;

    let mut plan = RebasePlan {
        state: RebaseState::Current,
        branch,
        head,
        target,
        fork_point: None,
        commits: 0,
        command: None,
        reason: None,
    };
    if is_ancestor(repo, &plan.target, &plan.head)? {
        return Ok(plan);
    }
    match fork_point(repo, &plan.head, &upstream, &state_sha)? {
        Ok(fork) => {
            let onto = format!("{COMPANY_REMOTE}/{}", plan.branch);
            (plan.state, plan.command) = if is_ancestor(repo, &fork, &plan.target)? {
                (RebaseState::Behind, Some(format!("git rebase {onto}")))
            } else {
                (
                    RebaseState::Replaced,
                    Some(format!("git rebase --onto {onto} {fork}")),
                )
            };
            let range = format!("{fork}..{}", plan.head);
            let count = git_ok(repo, &["rev-list", "--count", "--no-merges", &range])?;
            plan.commits = count.trim().parse().unwrap_or(0);
            plan.fork_point = Some(fork);
        }
        Err(reason) => {
            plan.state = RebaseState::Unknown;
            plan.reason = Some(reason);
        }
    }
    Ok(plan)
}

/// The newest commit of a company main in `head`, or why it cannot be told.
///
/// A commit of a company main is one a rebuild wrote, which carries an
/// `Uplink-Patch-Id` trailer, or one `previous-main.json` lists: a merge
/// that landed on a main a rebuild has since replaced.
fn fork_point(
    repo: &Path,
    head: &str,
    upstream: &str,
    state_sha: &str,
) -> Result<std::result::Result<String, String>> {
    let above_upstream = commits_with_patch_id(repo, &["--topo-order", head, "--not", upstream])?;
    let listed = listed_commits(repo, state_sha, &above_upstream)?;
    let of_main =
        |(sha, has_patch_id): &(String, bool)| *has_patch_id || listed.contains(sha.as_str());
    let Some((fork, _)) = above_upstream.iter().find(|commit| of_main(commit)) else {
        return Ok(Err(
            "the branch has no commit of company main above public upstream".into(),
        ));
    };

    // A branch that merged two different mains has no single commit to
    // rebase from.
    let below_fork: HashSet<String> = git_ok(repo, &["rev-list", fork, "--not", upstream])?
        .lines()
        .map(str::to_string)
        .collect();
    if above_upstream
        .iter()
        .any(|commit| of_main(commit) && !below_fork.contains(&commit.0))
    {
        return Ok(Err(
            "the branch holds commits of more than one company main".into(),
        ));
    }

    // Everything under the fork point is left behind by the rebase, so all
    // of it has to be company main. A patch commit cherry-picked on top of
    // the branch's own work is not a fork point.
    let trunk = commits_with_patch_id(repo, &["--first-parent", fork, "--not", upstream])?;
    if let Some((own, _)) = trunk.iter().find(|commit| !of_main(commit)) {
        return Ok(Err(format!(
            "{} is under {} but is not a commit of company main",
            short(own),
            short(fork)
        )));
    }
    Ok(Ok(fork.clone()))
}

/// The commits of `candidates` that the newest revision of
/// `previous-main.json` naming any of them lists.
fn listed_commits(
    repo: &Path,
    state_sha: &str,
    candidates: &[(String, bool)],
) -> Result<HashSet<String>> {
    let candidates: HashSet<&str> = candidates.iter().map(|(sha, _)| sha.as_str()).collect();
    let revisions = file_history(repo, state_sha, PREVIOUS_MAIN_PATH)?;
    for revision in revisions.iter().take(PREVIOUS_MAIN_REVISIONS) {
        let Some(previous) = previous_main_at(repo, &revision.sha) else {
            continue;
        };
        let hits: HashSet<String> = previous
            .commits
            .into_iter()
            .filter(|sha| candidates.contains(sha.as_str()))
            .collect();
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    Ok(HashSet::new())
}

/// `previous-main.json` as committed at `sha`; `None` when that commit has
/// none or it does not parse.
fn previous_main_at(repo: &Path, sha: &str) -> Option<PreviousMain> {
    let raw = show_at(repo, sha, PREVIOUS_MAIN_PATH).ok()?;
    serde_json::from_str(&raw).ok()
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Rebases the checked-out branch onto company main, from the commit of the
/// main it started on. A conflict leaves the rebase in progress, as
/// `git rebase` does.
///
/// The rebase runs as plain `git`, not through [`git`]: the commits are the
/// developer's, so their identity, signing and hooks apply, not the bot's.
pub fn rebase_onto_main(repo: &Path, fetch: bool) -> Result<RebaseResult> {
    let plan = rebase_plan(repo, "HEAD", fetch)?;
    let checked_out = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if checked_out == plan.branch || checked_out.starts_with("uplink/") {
        return Err(Error::msg(format!(
            "{checked_out} is not a branch to rebase; check out your own branch first."
        )));
    }
    let onto_fork = match plan.state {
        RebaseState::Current => {
            return Ok(RebaseResult {
                plan,
                rebased: false,
            });
        }
        RebaseState::Unknown => return Err(unknown_error(repo, &plan)),
        RebaseState::Behind => None,
        RebaseState::Replaced => plan.fork_point.clone(),
    };
    ensure_clean_worktree(repo, "a rebase")?;
    let mut args = vec!["rebase"];
    if let Some(fork) = onto_fork.as_deref() {
        args.extend_from_slice(&["--onto", plan.target.as_str(), fork]);
    } else {
        args.push(plan.target.as_str());
    }
    let output = std::process::Command::new("git")
        .current_dir(repo)
        .args(&args)
        .output()?;
    if !output.status.success() {
        return Err(Error::msg(format!(
            "{}\n{}\n\ngit rebase stopped. Resolve the conflicts and run `git rebase --continue`, \
or give up with `git rebase --abort`.",
            String::from_utf8_lossy(&output.stdout).trim_end(),
            String::from_utf8_lossy(&output.stderr).trim_end()
        )));
    }
    Ok(RebaseResult {
        plan,
        rebased: true,
    })
}

fn unknown_error(repo: &Path, plan: &RebasePlan) -> Error {
    let mut message = format!(
        "Cannot tell where this branch left {branch}: {reason}.\n\
Rebase by hand, naming the commit of the old {branch} the branch started from:\n\n    \
git rebase --onto {COMPANY_REMOTE}/{branch} <that commit>\n",
        branch = plan.branch,
        reason = plan.reason.as_deref().unwrap_or("no fork point found"),
    );
    let state = format!("{COMPANY_REMOTE}/{STATE_BRANCH}");
    if let Some(previous) = previous_main_at(repo, &state) {
        let _ = write!(
            message,
            "\nThe last rebuild replaced {} at {} ({}).\n",
            plan.branch, previous.tip, previous.at
        );
    }
    Error::msg(message)
}
