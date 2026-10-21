//! A run paused to amend an item, the way `git rebase` stops on a commit.
//! The checkout is left on what the item must apply on, with the item's
//! commits cherry-picked one by one up to the first that conflicts, so the
//! operator sees which are in and which are still to pick.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    ADOPT_AMENDED_REF_PREFIX, AdoptFailure, AdoptFailureKind, BranchRun, RunItem, RunKind,
};
use crate::error::{Error, Result};
use crate::gate::{assert_resolution_clean, commit_resolution};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::ops::{checkout_identity, restore_checkout};
use crate::preflight::apply_abs;
use crate::queue::{get_patch, patch_path};
use crate::repo::{UPSTREAM_REF, conflicted_files, ensure_state_worktree, rev_parse};
use crate::types::{ApplyOutcome, PatchStatus, QueueState};

/// How `--continue`, `--skip`, `--abort` or `--abort-keep` takes up a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptResume {
    /// The amended files are added: what they hold becomes the item's change.
    /// A run that was interrupted (not paused) goes on where it was.
    Continue,
    /// Leave out the item the run is on.
    Skip,
    /// Put the checkout back and reset uplink/state to where the run started.
    Abort,
    /// Put the checkout back, keeping the patches the run added.
    AbortKeep,
}

/// What the checkout of a pause is built on.
pub(super) enum PauseAt {
    /// uplink/upstream with these queued patches applied.
    Upstream(Vec<String>),
    /// The restack branch: origin's queue with the items added so far.
    Stack(String),
}

/// Where one commit of the item stands in the paused checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
pub(super) enum CommitState {
    /// Cherry-picked.
    Applied,
    /// What it changes is in the checkout already.
    AlreadyApplied,
    /// The cherry-pick stopped on it with these files in conflict.
    Conflict { files: Vec<String> },
    /// It could not be applied at all; make the change by hand.
    NotApplied { reason: String },
    /// Not picked yet, because an earlier one stopped.
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PausedCommit {
    /// None for an item that is one patch file.
    sha: Option<String>,
    subject: String,
    #[serde(default)]
    merge: bool,
    #[serde(flatten)]
    state: CommitState,
}

/// A run paused on an item to amend, kept in the run file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Paused {
    /// The checkout the run started from, as `checkout_identity` gives it.
    original: String,
    original_sha: String,
    /// What the item's change is applied on, before it.
    onto: String,
    /// `onto` in words, for the instructions.
    on: String,
    reason: String,
    commits: Vec<PausedCommit>,
}

impl Paused {
    pub(super) fn instructions(&self, run: &BranchRun) -> String {
        let cmd = run.kind.command();
        let mut out = format!(
            "{} paused on {} (\"{}\"): {}.\nThe checkout holds {} with the change on top:\n",
            run.noun(),
            run.item_label(run.next),
            run.items[run.next].title(),
            self.reason,
            self.on,
        );
        for commit in &self.commits {
            let name = match &commit.sha {
                Some(sha) => format!("{} {}", short(sha), commit.subject),
                None => commit.subject.clone(),
            };
            let (mark, state) = match &commit.state {
                CommitState::Applied => ("✓", "applied".to_string()),
                CommitState::AlreadyApplied => ("=", "already applied".to_string()),
                CommitState::Conflict { files } if files.is_empty() => {
                    ("✗", "in conflict".to_string())
                }
                CommitState::Conflict { files } => {
                    ("✗", format!("in conflict: {}", files.join(", ")))
                }
                CommitState::NotApplied { reason } => (
                    "✗",
                    format!("not applied, make the change by hand: {reason}"),
                ),
                CommitState::Pending => ("·", format!("to pick: {}", pick_command(commit))),
            };
            out.push_str(&format!("  {mark} {name}  ({state})\n"));
        }
        let picking = self.commits.iter().any(|c| c.sha.is_some());
        let conflict = self
            .commits
            .iter()
            .any(|c| matches!(c.state, CommitState::Conflict { .. }));
        if picking && conflict {
            out.push_str(&format!(
                "Resolve the conflict and git add the files. git uplink {cmd} --continue finishes \
the cherry-pick and picks the rest, stopping again on the next conflict; or pick them yourself \
with the commands above, in order.\n"
            ));
        }
        out.push_str(&format!(
            "Amend the change (run preflight.sh to check), git add the files, then:  git uplink {cmd} --continue\n"
        ));
        if run.kind.can_leave_out() {
            out.push_str(&format!("Leave the item out:  git uplink {cmd} --skip\n"));
        }
        out.push_str(&run.ways_out());
        if let Some(path) = &run.groups_file {
            out.push_str(&format!("\nGroups file: {}", path.display()));
        }
        out
    }

    /// Puts the checkout back to where the run started.
    pub(super) fn restore(&self, repo: &Path) -> Result<()> {
        if cherry_pick_in_progress(repo)? {
            let _ = git(repo, &["cherry-pick", "--abort"], GitOpts::allow_fail());
        }
        restore_checkout(repo, &self.original, &self.original_sha)?;
        ensure_state_worktree(repo)
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn pick_command(commit: &PausedCommit) -> String {
    let sha = commit.sha.as_deref().unwrap_or_default();
    if commit.merge {
        format!("git cherry-pick -m 1 {}", short(sha))
    } else {
        format!("git cherry-pick {}", short(sha))
    }
}

fn cherry_pick_in_progress(repo: &Path) -> Result<bool> {
    git_succeeds(
        repo,
        &["rev-parse", "--quiet", "--verify", "CHERRY_PICK_HEAD"],
    )
}

/// No staged or unstaged changes to tracked files.
pub(super) fn checkout_clean(repo: &Path) -> Result<bool> {
    Ok(git_succeeds(repo, &["diff", "--cached", "--quiet"])?
        && git_succeeds(repo, &["diff", "--quiet"])?)
}

/// Leaves the checkout on `at` with the item's change on top, for the
/// operator to amend. Returns the pause to keep in the run file.
pub(super) fn pause(
    repo: &Path,
    queue: &QueueState,
    run: &BranchRun,
    info: &AdoptFailure,
    candidate_abs: &Path,
    at: &PauseAt,
) -> Result<Paused> {
    let (original, original_sha) = checkout_identity(repo)?;
    let item = &run.items[run.next];
    let placed = (|| -> Result<(String, String, Vec<PausedCommit>)> {
        let on = match at {
            PauseAt::Upstream(deps) => {
                checkout_detached(repo, UPSTREAM_REF)?;
                apply_deps(repo, queue, deps, info)?;
                if deps.is_empty() {
                    "uplink/upstream".to_string()
                } else {
                    format!("uplink/upstream + {}", deps.join(", "))
                }
            }
            PauseAt::Stack(stack) => {
                checkout_detached(repo, stack)?;
                format!("{stack} ({})", run.stack_words())
            }
        };
        let onto = rev_parse(repo, "HEAD")?;
        let commits = if item.commits().is_empty() {
            vec![apply_whole(repo, candidate_abs, item)?]
        } else {
            pick_each(repo, item.commits())?
        };
        Ok((onto, on, commits))
    })();
    let (onto, on, commits) = match placed {
        Ok(placed) => placed,
        Err(err) => {
            if cherry_pick_in_progress(repo).unwrap_or(false) {
                let _ = git(repo, &["cherry-pick", "--abort"], GitOpts::allow_fail());
            }
            restore_checkout(repo, &original, &original_sha)?;
            ensure_state_worktree(repo)?;
            return Err(err);
        }
    };
    Ok(Paused {
        original,
        original_sha,
        onto,
        on,
        reason: match info.kind {
            AdoptFailureKind::Apply => "it does not apply onto public upstream".into(),
            AdoptFailureKind::Preflight => "preflight.sh fails on it".into(),
            AdoptFailureKind::Stack if run.kind == RunKind::Restack => {
                "it does not apply on top of origin's queue".into()
            }
            AdoptFailureKind::Stack => "it does not apply on company main".into(),
            AdoptFailureKind::CompanyPreflight => "preflight.sh fails on it on company main".into(),
        },
        commits,
    })
}

fn checkout_detached(repo: &Path, rev: &str) -> Result<()> {
    git(
        repo,
        &["checkout", "-f", "--quiet", "--detach", rev],
        GitOpts::default(),
    )?;
    Ok(())
}

fn apply_deps(repo: &Path, queue: &QueueState, deps: &[String], info: &AdoptFailure) -> Result<()> {
    for id in deps {
        let dep = get_patch(queue, id)?;
        // Already in upstream, possibly in another form than its patch file.
        if dep.status == PatchStatus::Merged {
            continue;
        }
        if apply_abs(repo, &repo.join(patch_path(id)?), &dep.title)? == ApplyOutcome::Conflict {
            return Err(Error::msg(format!(
                "{id} (\"{}\") does not apply onto public upstream, so {} cannot be amended on it",
                dep.title, info.item
            )));
        }
    }
    Ok(())
}

/// Applies an item that is one patch file with a 3-way merge, staged.
fn apply_whole(repo: &Path, candidate_abs: &Path, item: &RunItem) -> Result<PausedCommit> {
    let applied = git(
        repo,
        &[
            "apply",
            "--3way",
            "--index",
            candidate_abs.to_str().unwrap_or(""),
        ],
        GitOpts::allow_fail(),
    )?;
    let files = conflicted_files(repo)?;
    let state = if !files.is_empty() {
        CommitState::Conflict { files }
    } else if applied.code != 0 {
        CommitState::NotApplied {
            reason: applied.stderr.lines().next().unwrap_or("").to_string(),
        }
    } else {
        CommitState::Applied
    };
    let subject = match item {
        RunItem::Queued(queued) => format!("patch {} \"{}\"", queued.patch.id, item.title()),
        RunItem::Group(_) => format!("the amended change of \"{}\"", item.title()),
    };
    Ok(PausedCommit {
        sha: None,
        subject,
        merge: false,
        state,
    })
}

/// The commits of an item, none picked yet.
fn pick_each(repo: &Path, commits: &[String]) -> Result<Vec<PausedCommit>> {
    let mut out = Vec::new();
    for sha in commits {
        let subject = git_ok(repo, &["log", "-1", "--format=%s", sha])?;
        let parents = git_ok(repo, &["rev-list", "--parents", "-n1", sha])?
            .split_whitespace()
            .count()
            - 1;
        out.push(PausedCommit {
            sha: Some(sha.clone()),
            subject,
            merge: parents > 1,
            state: CommitState::Pending,
        });
    }
    pick_pending(repo, &mut out)?;
    Ok(out)
}

/// The change of `sha` is in HEAD already, as `git cherry` tells by patch id.
fn already_in_head(repo: &Path, sha: &str) -> bool {
    git_ok(repo, &["cherry", "HEAD", sha, &format!("{sha}^")])
        .is_ok_and(|out| out.lines().any(|line| line.starts_with('-')))
}

/// Cherry-picks the pending commits onto HEAD in order, until one does not
/// apply; true when it stopped on one. One the operator picked by hand
/// already is marked so.
fn pick_pending(repo: &Path, commits: &mut [PausedCommit]) -> Result<bool> {
    for commit in commits.iter_mut() {
        if commit.state != CommitState::Pending {
            continue;
        }
        let sha = commit.sha.clone().unwrap_or_default();
        if !commit.merge && already_in_head(repo, &sha) {
            commit.state = CommitState::AlreadyApplied;
            continue;
        }
        let before = rev_parse(repo, "HEAD^{tree}")?;
        let mut args = vec!["cherry-pick", "--allow-empty", "--keep-redundant-commits"];
        if commit.merge {
            args.extend(["-m", "1"]);
        }
        args.push(&sha);
        let picked = git(repo, &args, GitOpts::allow_fail())?;
        if picked.code == 0 {
            commit.state = if rev_parse(repo, "HEAD^{tree}")? == before {
                CommitState::AlreadyApplied
            } else {
                CommitState::Applied
            };
            continue;
        }
        let files = conflicted_files(repo)?;
        commit.state = if files.is_empty() {
            let _ = git(repo, &["cherry-pick", "--abort"], GitOpts::allow_fail());
            CommitState::NotApplied {
                reason: picked
                    .stderr
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("")
                    .to_string(),
            }
        } else {
            CommitState::Conflict { files }
        };
        return Ok(true);
    }
    Ok(false)
}

/// Finishes the cherry-pick the operator resolved, as `git cherry-pick
/// --continue` would, or drops it when the resolution left it empty.
fn finish_cherry_pick(repo: &Path, commits: &mut [PausedCommit]) -> Result<()> {
    assert_resolution_clean(repo)?;
    let empty = git_succeeds(repo, &["diff", "--cached", "--quiet"])?;
    if empty {
        git(repo, &["cherry-pick", "--skip"], GitOpts::default())?;
    } else {
        git(
            repo,
            &["-c", "core.editor=true", "cherry-pick", "--continue"],
            GitOpts::default(),
        )?;
    }
    if let Some(commit) = commits
        .iter_mut()
        .find(|c| matches!(c.state, CommitState::Conflict { .. }))
    {
        commit.state = if empty {
            CommitState::AlreadyApplied
        } else {
            CommitState::Applied
        };
    }
    Ok(())
}

/// `--continue` on a paused run, like `git rebase --continue`: finishes a
/// resolved cherry-pick and picks the commits still to pick. When one of
/// them does not apply, returns the pause on it. Otherwise the checkout's
/// change since `onto` becomes the item's, kept by a ref as an amended
/// commit.
pub(super) fn continue_paused(
    repo: &Path,
    run: &mut BranchRun,
    paused: &Paused,
) -> Result<Option<Paused>> {
    let mut commits = paused.commits.clone();
    if cherry_pick_in_progress(repo)? {
        finish_cherry_pick(repo, &mut commits)?;
    }
    if commits.iter().any(|c| c.sha.is_some()) && pick_pending(repo, &mut commits)? {
        return Ok(Some(Paused {
            commits,
            ..paused.clone()
        }));
    }
    assert_resolution_clean(repo)?;
    let item = &run.items[run.next];
    let title = item.title().to_string();
    commit_resolution(repo, &paused.onto, &title)?;
    if rev_parse(repo, "HEAD")? == paused.onto {
        if !run.kind.can_leave_out() {
            return Err(Error::msg(format!(
                "{} (\"{title}\") is left with no change, and init adopts every commit of company main. \
Make the change, or give up with git uplink {} --abort.",
                run.item_label(run.next),
                run.kind.command()
            )));
        }
        eprintln!(
            "{} (\"{title}\") is left with no change; leaving it out",
            run.item_label(run.next)
        );
        run.items.remove(run.next);
        return Ok(None);
    }
    // The change stays its author's.
    if let RunItem::Group(group) = item
        && let Some(last) = group.commits.last()
    {
        let author = git_ok(repo, &["log", "-1", "--format=%an <%ae>", last])?;
        git(
            repo,
            &[
                "commit",
                "--amend",
                "--quiet",
                "--no-edit",
                "--author",
                &author,
            ],
            GitOpts::default(),
        )?;
    }
    let sha = rev_parse(repo, "HEAD")?;
    git(
        repo,
        &[
            "update-ref",
            &format!("{ADOPT_AMENDED_REF_PREFIX}{sha}"),
            &sha,
        ],
        GitOpts::default(),
    )?;
    run.items[run.next].set_amended(sha);
    Ok(None)
}
