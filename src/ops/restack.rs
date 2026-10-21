use super::*;

use crate::adopt::{AdoptDecider, AdoptResume, BranchRun, QueuedItem};
use crate::lock::with_run_lock;

/// The branch a restack builds origin's queue on, with each local patch
/// applied on top as it is added again.
pub const RESTACK_BRANCH: &str = "uplink/restack";

/// Keeps local uplink/state as it was before a restack, so nothing it left
/// out is lost.
pub const RESTACK_FROM_REF_PREFIX: &str = "refs/uplink/restack-from/";

#[derive(Clone, Default)]
pub struct RestackOpts {
    /// Takes up the run in progress instead of starting one.
    pub resume: Option<AdoptResume>,
    /// `None` detects a terminal.
    pub interactive: Option<bool>,
    /// Says what to do with a patch that fails. `None` asks in a terminal
    /// window, and fails the run elsewhere.
    pub decide: Option<AdoptDecider>,
    /// Do not run `preflight.sh` on upstream-bound patches.
    pub no_preflight: bool,
    /// Do not run a failing `preflight.sh` again with queued upstream
    /// patches to find the ones a patch needs.
    pub no_preflight_deps: bool,
    /// Do not build the preview branch when the run ends.
    pub no_preview: bool,
    /// Leave [`RESTACK_BRANCH`] when the run ends.
    pub keep_branch: bool,
    pub progress: ProgressMode,
}

/// Where local uplink/state stood against origin's, as last fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestackAction {
    /// The same commit: nothing to do.
    UpToDate,
    /// Local is ahead of origin's: nothing to restack, push it.
    Ahead,
    /// Local was behind and was moved to origin's.
    FastForwarded,
    /// Local patches were added again on top of origin's.
    Restacked,
}

#[derive(Debug, Clone)]
pub struct RestackResult {
    pub action: RestackAction,
    /// The run that added the local patches again (or the one resumed).
    pub run: Option<AdoptBranchResult>,
    /// The ref that keeps local uplink/state as it was before.
    pub backup: Option<String>,
}

impl RestackOpts {
    fn adopt_opts(&self) -> AdoptBranchOpts {
        AdoptBranchOpts {
            interactive: self.interactive,
            decide: self.decide.clone(),
            no_preview: self.no_preview,
            progress: self.progress,
            ..AdoptBranchOpts::default()
        }
    }
}

/// Adds the local queue items origin's `uplink/state` lacks again on top of
/// it, as last fetched: nothing is fetched or pushed. Origin's queue is
/// rebuilt onto upstream on [`RESTACK_BRANCH`], and each local patch is
/// checked as `adopt` checks a group (it must apply on that branch, and an
/// upstream-bound one onto upstream and its dependencies, and pass
/// `preflight.sh`) and committed when it passes. A patch that fails is put
/// to the operator the way adopt does. `git uplink push` publishes the result.
pub fn restack(repo: &Path, opts: RestackOpts) -> Result<RestackResult> {
    crate::git::with_offline(|| restack_offline(repo, opts))
}

fn restack_offline(repo: &Path, opts: RestackOpts) -> Result<RestackResult> {
    if let Some(resume) = opts.resume {
        let run = resume_run(repo, resume, &opts.adopt_opts())?;
        return Ok(RestackResult {
            action: RestackAction::Restacked,
            run: Some(run),
            backup: None,
        });
    }
    crate::run::refuse_while_running(repo)?;
    read_queue_file(repo)?;
    let tracking = format!("{COMPANY_REMOTE}/{STATE_BRANCH}");
    if !has_ref(repo, &format!("refs/remotes/{tracking}"))? {
        return Err(Error::msg(format!(
            "{tracking} is not in this clone, and restack makes no network calls; \
fetch it first: git uplink refresh"
        )));
    }
    let local = rev_parse(repo, STATE_BRANCH)?;
    let remote = rev_parse(repo, &tracking)?;
    let settled = |action| {
        Ok(RestackResult {
            action,
            run: None,
            backup: None,
        })
    };
    if local == remote {
        return settled(RestackAction::UpToDate);
    }
    if is_ancestor(repo, &remote, &local)? {
        return settled(RestackAction::Ahead);
    }
    if is_ancestor(repo, &local, &remote)? {
        with_queue_lock(repo, || set_state_branch(repo, STATE_BRANCH, &remote))?;
        return settled(RestackAction::FastForwarded);
    }
    if merge_base(repo, &local, &remote)?.is_none() {
        return Err(Error::msg(format!(
            "{STATE_BRANCH} has unrelated histories with {tracking}; cannot restack"
        )));
    }
    ensure_clean_worktree(repo, "a restack")?;

    let backup = format!("{RESTACK_FROM_REF_PREFIX}{local}");
    git(repo, &["update-ref", &backup, &local], GitOpts::default())?;
    let decide = opts.decide.clone().or_else(|| {
        opts.interactive
            .unwrap_or_else(adopt::stdin_is_tty)
            .then(prompt_decider)
    });
    let adoption = with_run_lock(repo, || {
        let items = match start_on_origin(repo, &local, &remote) {
            Ok(items) => items,
            Err(err) => {
                set_state_branch(repo, STATE_BRANCH, &local)?;
                let _ = git(
                    repo,
                    &["branch", "-D", RESTACK_BRANCH],
                    GitOpts::allow_fail(),
                );
                return Err(err);
            }
        };
        let run = BranchRun::restack(
            local.clone(),
            items,
            RESTACK_BRANCH.to_string(),
            opts.keep_branch,
            !opts.no_preflight,
            !opts.no_preflight_deps,
        );
        adopt::run_branch(
            repo,
            None,
            run,
            &ScriptVerdict::Run,
            decide.as_ref(),
            opts.progress,
        )
    })?;
    let preview = preview_after(repo, &adoption, opts.no_preview);
    Ok(RestackResult {
        action: RestackAction::Restacked,
        run: Some(AdoptBranchResult {
            kind: RunKind::Restack,
            branch: COMPANY_REMOTE.to_string(),
            adoption: Some(adoption),
            behind: false,
            groups_file: PathBuf::new(),
            aborted: None,
            preview,
        }),
        backup: Some(backup),
    })
}

/// Moves local uplink/state to origin's, carrying over what needs no check
/// (a tooling patch origin lacks, and local changes to patches both sides
/// have, as push does), builds origin's queue on [`RESTACK_BRANCH`], and
/// returns the local patches to add again, in queue order.
fn start_on_origin(repo: &Path, local: &str, remote: &str) -> Result<Vec<QueuedItem>> {
    let local_queue = queue_at(repo, local)?;
    let remote_queue = queue_at(repo, remote)?;
    let carry = LocalOnly::of(&local_queue, &remote_queue);
    let replay = locally_changed_shared_patches(repo, &local_queue, local, &remote_queue, remote)?;
    set_state_branch(repo, STATE_BRANCH, remote)?;
    if carry.tooling.is_some() || !replay.is_empty() {
        let mut paths = Vec::new();
        if let Some(tooling) = &carry.tooling {
            paths.push(patch_path(&tooling.id)?.to_string_lossy().into_owned());
        }
        for (patch, _) in &replay {
            paths.extend(patch_state_paths(repo, local, &patch.id)?);
        }
        restore_paths_from(repo, local, &paths)?;
        let mut queue = read_queue_file(repo)?;
        for (patch, layer) in &replay {
            replace_patch(&mut queue, patch.clone(), *layer);
        }
        if queue.tooling.is_none() {
            queue.tooling = carry.tooling.clone();
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: restack local changes onto origin")?;
    }
    rebuild_preview(repo, RESTACK_BRANCH)?;
    let item = |patch: &Patch, internal_only| QueuedItem {
        patch: patch.clone(),
        internal_only,
        source: local.to_string(),
        ignore_preflight: false,
        amended: None,
    };
    Ok(carry
        .upstream
        .iter()
        .map(|patch| item(patch, false))
        .chain(carry.internal.iter().map(|patch| item(patch, true)))
        .collect())
}
