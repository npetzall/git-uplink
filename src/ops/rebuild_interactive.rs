//! `rebuild -i`: a rebuild that stops on a patch the way `git rebase` stops
//! on a commit, in place of the conflict gate. The checkout is left on the
//! patch: the failed 3-way apply, or the patch `preflight.sh` is blamed on
//! with the fix to go on top. `--continue` takes what the checkout holds as
//! the patch and replays again; `--abort` puts the checkout back.
//!
//! Nothing is written to uplink/state until the rebuild is published: the
//! resolutions wait in `<git-common-dir>/uplink-rebuild.json`, which locks
//! uplink/state against every other command while it exists.

use serde::{Deserialize, Serialize};

use super::*;
use crate::lock::with_run_lock;

const PAUSE_FILE: &str = "uplink-rebuild.json";

/// Keeps the commit a pause squashes onto from garbage collection.
const ONTO_REF: &str = "refs/uplink/rebuild/onto";

#[derive(Debug, Clone, Default)]
pub struct InlineRebuildOpts {
    pub push: bool,
    pub push_remote: Option<String>,
    pub verify: bool,
}

/// How `rebuild --continue` or `--abort` takes up a paused rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildResume {
    /// What the checkout holds becomes the patch; the rebuild goes on.
    Continue,
    /// Put the checkout back; uplink/state was never changed.
    Abort,
}

/// A published inline rebuild.
#[derive(Debug, Clone)]
pub struct InlineRebuild {
    pub result: RebuildResult,
    /// The queue before the rebuild, for what became ready to submit.
    pub before: QueueState,
    /// The patches it amended, in the order they were resolved.
    pub amended: Vec<String>,
    /// Where the rebuild was pushed.
    pub pushed: Option<String>,
}

/// One resolution, waiting for the rebuild to be published.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedPatch {
    id: String,
    contents: String,
    assess: AssessReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "cause")]
enum StopCause {
    /// It does not apply. `files` are in conflict; none when `git apply
    /// --3way` could not merge it at all.
    Apply { files: Vec<String> },
    /// It applies, but `preflight.sh` fails from it on.
    Preflight { output: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stop {
    id: String,
    title: String,
    /// The queue before the patch: what the fix is squashed onto.
    onto: String,
    #[serde(flatten)]
    cause: StopCause,
}

/// A paused rebuild, in the pause file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PausedRebuild {
    /// The checkout the rebuild started from, as [`checkout_identity`] gives it.
    original: String,
    original_sha: String,
    push: bool,
    push_remote: Option<String>,
    verify: bool,
    resolved: Vec<ResolvedPatch>,
    /// None when the rebuild was interrupted after a resolution:
    /// `--continue` replays again.
    stop: Option<Stop>,
}

impl PausedRebuild {
    fn instructions(&self) -> String {
        let Some(stop) = &self.stop else {
            return format!(
                "a rebuild is paused after resolving {}.\n\
Replay again:              git uplink rebuild --continue\n\
Stop, changing nothing:    git uplink rebuild --abort",
                self.resolved_ids().join(", ")
            );
        };
        let id = &stop.id;
        let mut out = match &stop.cause {
            StopCause::Apply { files } if files.is_empty() => format!(
                "rebuild paused on {id} (\"{}\"): it does not apply on the patches before it, \
and git apply --3way could not merge it, so nothing of it is applied.\n\
The checkout holds the queue before {id}. Make its change by hand, from \
.uplink/patches/{id}.patch (git apply --reject writes what does not fit to .rej files).\n",
                stop.title
            ),
            StopCause::Apply { files } => format!(
                "rebuild paused on {id} (\"{}\"): it does not apply on the patches before it.\n\
The checkout holds the queue before {id} with the failed apply. In conflict:\n  {}\n",
                stop.title,
                files.join("\n  ")
            ),
            StopCause::Preflight { output } => {
                let mut out = format!(
                    "rebuild paused on {id} (\"{}\"): it applies, but preflight.sh fails once it is applied.\n\
The checkout holds the queue up to and including {id}. Fix it on top \
(run preflight.sh to check).\n",
                    stop.title
                );
                if let Some(output) = output.as_deref().filter(|o| !o.trim().is_empty()) {
                    out.push_str(&format!(
                        "preflight.sh output (tail):\n{}\n",
                        output.trim_end()
                    ));
                }
                out
            }
        };
        let resolved = self.resolved_ids();
        if !resolved.is_empty() {
            out.push_str(&format!("Resolved so far: {}\n", resolved.join(", ")));
        }
        out.push_str(&format!(
            "git add the files, then:  git uplink rebuild --continue\n\
Stop, changing nothing:   git uplink rebuild --abort\n\
To leave {id} out, abort and run git uplink drop {id}."
        ));
        out
    }

    fn resolved_ids(&self) -> Vec<String> {
        self.resolved.iter().map(|r| r.id.clone()).collect()
    }
}

fn pause_path(repo: &Path) -> Result<PathBuf> {
    Ok(repo
        .join(git_ok(repo, &["rev-parse", "--git-common-dir"])?)
        .join(PAUSE_FILE))
}

fn read_paused(repo: &Path) -> Result<Option<PausedRebuild>> {
    match fs::read_to_string(pause_path(repo)?) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn save_paused(repo: &Path, paused: &PausedRebuild) -> Result<()> {
    fs::write(
        pause_path(repo)?,
        format!("{}\n", serde_json::to_string_pretty(paused)?),
    )?;
    Ok(())
}

fn end_pause(repo: &Path) -> Result<()> {
    match fs::remove_file(pause_path(repo)?) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
        _ => {}
    }
    git(repo, &["update-ref", "-d", ONTO_REF], GitOpts::allow_fail())?;
    Ok(())
}

/// Why uplink/state is locked by a paused rebuild, or None. Never fails.
pub fn paused_rebuild_message(repo: &Path) -> Option<String> {
    let path = pause_path(repo).ok()?;
    if !path.is_file() {
        return None;
    }
    let message = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<PausedRebuild>(&raw).ok())
        .map(|paused| {
            format!(
                "a rebuild is paused, so uplink/state is locked. Finish it first.\n{}",
                paused.instructions()
            )
        })
        .unwrap_or_else(|| {
            format!(
                "a rebuild is paused, so uplink/state is locked; see {}",
                path.display()
            )
        });
    Some(message)
}

/// `rebuild -i`: rebuilds company main, stopping on a patch to fix inline
/// in place of gating it.
pub fn rebuild_inline(repo: &Path, opts: InlineRebuildOpts) -> Result<InlineRebuild> {
    // Refuses while a rebuild is paused or a queue run holds uplink/state.
    with_queue_lock(repo, || {
        ensure_clean_worktree(repo, "a rebuild")?;
        let (original, original_sha) = checkout_identity(repo)?;
        let paused = PausedRebuild {
            original,
            original_sha,
            push: opts.push,
            push_remote: opts.push_remote.clone(),
            verify: opts.verify,
            resolved: Vec::new(),
            stop: None,
        };
        // The pause file does not exist yet, so the plain lock lets the
        // rebuild write; once it pauses, only the resume does.
        replay(repo, paused)
    })
}

/// `rebuild --continue` or `--abort`. `None` after an abort.
pub fn resume_rebuild(repo: &Path, resume: RebuildResume) -> Result<Option<InlineRebuild>> {
    if let Some(message) = crate::run::blocking_queue_run(repo) {
        return Err(Error::msg(message));
    }
    let Some(mut paused) = read_paused(repo)? else {
        return Err(Error::msg(
            "no rebuild is paused; start one with git uplink rebuild -i",
        ));
    };
    with_run_lock(repo, || match resume {
        RebuildResume::Abort => {
            restore_checkout(repo, &paused.original, &paused.original_sha)?;
            restore_uplink_from_state(repo)?;
            ensure_state_worktree(repo)?;
            end_pause(repo)?;
            Ok(None)
        }
        RebuildResume::Continue => {
            if let Some(stop) = paused.stop.clone() {
                let resolved = resolve_stop(repo, &stop)?;
                paused.resolved.retain(|r| r.id != resolved.id);
                paused.resolved.push(resolved);
                paused.stop = None;
                // An interruption from here on keeps the resolution.
                save_paused(repo, &paused)?;
            }
            replay(repo, paused).map(Some)
        }
    })
}

/// Takes what the checkout holds as the patch `stop` is on, squashed onto
/// the queue before it. An upstream-bound resolution must pass the
/// assessment; otherwise the checkout is put back as it was.
fn resolve_stop(repo: &Path, stop: &Stop) -> Result<ResolvedPatch> {
    let queue = read_queue_file(repo)?;
    let patch = get_patch(&queue, &stop.id)?.clone();
    assert_resolution_clean(repo)?;
    let before = rev_parse(repo, "HEAD")?;
    commit_resolution(repo, &stop.onto, &company_commit_message(&patch))?;
    if rev_parse(repo, "HEAD")? == stop.onto {
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
        return Err(Error::msg(format!(
            "{} is resolved to no change. Make the change, or leave the patch out: \
git uplink rebuild --abort, then git uplink drop {} (or git uplink merged {} when upstream has it).",
            stop.id, stop.id, stop.id
        )));
    }
    let intent = PatchIntent::from_internal_only(!queue.is_upstream(&stop.id));
    let report = assess_from_message(
        repo,
        &queue,
        &stop.onto,
        "HEAD",
        &stored_commit_message(&patch),
        Some(&patch.title),
        intent,
    )?;
    if !intent.is_internal_only()
        && let Err(err) = assert_assess_ok(&report, &patch.title)
    {
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
        return Err(err);
    }
    Ok(ResolvedPatch {
        id: stop.id.clone(),
        contents: format_patch_at_head(repo)?,
        assess: report,
    })
}

/// Replays the queue with the resolutions so far. Publishes the rebuild,
/// or pauses on the next patch to fix.
fn replay(repo: &Path, mut paused: PausedRebuild) -> Result<InlineRebuild> {
    let before = read_queue_file(repo)?;
    let company_branch = before.config.internal_branch.clone();
    let base = match ChangeBase::of_company(repo, &company_branch)? {
        // The push moves origin's branch, so name the commit.
        Some(base) if paused.push => Some(base.pinned(repo)?),
        base => base,
    };
    let tree_before = if has_ref(repo, &company_branch)? {
        Some(tree_of(repo, &company_branch)?)
    } else {
        None
    };
    let replace: Vec<(String, String)> = paused
        .resolved
        .iter()
        .map(|r| (r.id.clone(), r.contents.clone()))
        .collect();
    let resolved = paused.resolved.clone();
    let mut record = |queue: &mut QueueState| record_resolved(repo, queue, &resolved);
    let outcome = rebuild_replay(
        repo,
        Some(&ScriptVerdict::Run),
        paused.verify,
        Some(Inline {
            replace: &replace,
            before_publish: &mut record,
        }),
    );
    let queue = match outcome {
        Ok(Rebuilt::Published(queue)) => *queue,
        Ok(Rebuilt::Stopped(stop)) => {
            git(
                repo,
                &["update-ref", ONTO_REF, &stop.onto],
                GitOpts::default(),
            )?;
            paused.stop = Some(Stop {
                id: stop.patch.id.clone(),
                title: stop.patch.title.clone(),
                onto: stop.onto,
                cause: match stop.cause {
                    Stopped::Apply { files } => StopCause::Apply { files },
                    Stopped::Preflight { output, .. } => StopCause::Preflight {
                        output: output.as_deref().map(output_tail),
                    },
                },
            });
            save_paused(repo, &paused)?;
            return Err(Error::msg(paused.instructions()));
        }
        Err(err) => {
            if paused.resolved.is_empty() {
                end_pause(repo)?;
                return Err(match err {
                    Error::Conflict(conflict) => Error::msg(format!(
                        "{conflict}. It is gated already: fix it on uplink/conflict/{id}-work and run git uplink resolve {id}.",
                        id = conflict.patch_id
                    )),
                    err => err,
                });
            }
            paused.stop = None;
            save_paused(repo, &paused)?;
            return Err(Error::msg(format!("{err}\n{}", paused.instructions())));
        }
    };
    end_pause(repo)?;
    let tree_changed = tree_before != Some(tree_of(repo, &company_branch)?);
    let changes = match &base {
        Some(base) => Some(main_changes(repo, &queue, base, &company_branch)?),
        None => None,
    };
    let pushed = if paused.push {
        let remote = paused.push_remote.as_deref().unwrap_or("origin");
        // State first: a rejected state push leaves origin main alone.
        push_state_branch(repo, remote, STATE_BRANCH)?;
        push_branch_force(repo, remote, &company_branch)?;
        Some(remote.to_string())
    } else {
        None
    };
    if paused.original != company_branch {
        restore_checkout(repo, &paused.original, &paused.original_sha)?;
        ensure_state_worktree(repo)?;
    }
    Ok(InlineRebuild {
        result: RebuildResult {
            queue,
            branch: company_branch,
            preview: false,
            tree_changed,
            changes,
        },
        before,
        amended: paused.resolved_ids(),
        pushed,
    })
}

/// Records the resolutions on the queue as amends of their patches, in one
/// uplink/state commit. `.uplink/patches` holds them as replayed.
fn record_resolved(repo: &Path, queue: &mut QueueState, resolved: &[ResolvedPatch]) -> Result<()> {
    let mut ids = Vec::new();
    for resolution in resolved {
        let patch = get_patch_mut(queue, &resolution.id)?;
        // It applied empty on this replay: already in upstream.
        if patch.status == PatchStatus::Merged {
            continue;
        }
        patch.assess = Some(resolution.assess.clone());
        patch.conflict = None;
        let rel = patch_path(&resolution.id)?.to_string_lossy().into_owned();
        patch.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
        patch.status = status_after_rewrite(repo, patch);
        let detail = rewrite_event_detail(patch.status, "Resolved in an interactive rebuild");
        add_event(patch, "amended", detail);
        ids.push(resolution.id.clone());
    }
    if ids.is_empty() {
        return Ok(());
    }
    write_queue_file(repo, queue)?;
    commit_queue(
        repo,
        &format!("uplink: amend {} in rebuild", ids.join(", ")),
    )
}
