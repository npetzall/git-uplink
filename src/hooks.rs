//! Company hooks on the orphan branch `uplink/hooks`.
//!
//! `init` creates the branch locally from the embedded hooks pack and never
//! overwrites a file on it; `init --upgrade` adds files the pack gained.
//! `git uplink push` publishes it.

use std::path::Path;

use uuid::Uuid;

use crate::error::Result;
use crate::git::{GitOpts, git, git_ok};
use crate::progress::StepOutcome;
use crate::repo::{
    COMPANY_REMOTE, fetch_state_tracking, has_ref, is_ancestor, path_exists_at, point_branch_at,
    push_state_branch,
};
use crate::tooling::hooks_files;
use crate::types::{Forge, ForgeFamily, QueueState};

pub const HOOKS_BRANCH: &str = "uplink/hooks";
pub const TOOLCHAIN_ACTION_PATH: &str = ".github/actions/uplink-toolchain-hook/action.yml";

const HOOKS_REF: &str = "refs/heads/uplink/hooks";
const HOOKS_TRACKING_REF: &str = "refs/remotes/origin/uplink/hooks";
const PUBLISH: &str = "git uplink push";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HooksMode {
    /// Take origin's branch when there is no local one; never create.
    FetchOnly,
    /// Also create the branch when neither this clone nor origin has it.
    Create,
    /// Also add pack files missing from an existing branch.
    Upgrade,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HooksOutcome {
    Existing,
    FromOrigin,
    Created,
    Missing,
    /// Pack files added on top of the existing branch.
    Completed(Vec<String>),
    /// Upgrade could not add files; the reason says why.
    Skipped(String),
}

/// Make `uplink/hooks` available locally: keep an existing branch, else take
/// origin's, else (by `mode`) commit the embedded hooks pack as an orphan.
/// Never pushes, never changes an existing file, never touches the working tree.
pub fn ensure_hooks_branch(repo: &Path, forge: Forge, mode: HooksMode) -> Result<HooksOutcome> {
    if has_ref(repo, HOOKS_REF)? {
        if mode == HooksMode::Upgrade {
            return complete_hooks_branch(repo, forge);
        }
        return Ok(HooksOutcome::Existing);
    }
    if let Some(sha) = fetch_state_tracking(repo, COMPANY_REMOTE, HOOKS_BRANCH)? {
        point_branch_at(repo, HOOKS_BRANCH, &sha)?;
        if mode == HooksMode::Upgrade {
            return match complete_hooks_branch(repo, forge)? {
                HooksOutcome::Existing => Ok(HooksOutcome::FromOrigin),
                other => Ok(other),
            };
        }
        return Ok(HooksOutcome::FromOrigin);
    }
    if mode == HooksMode::FetchOnly {
        return Ok(HooksOutcome::Missing);
    }
    let tree = build_tree(repo, None, &hooks_files(forge))?;
    let commit = git_ok(
        repo,
        &["commit-tree", &tree, "-m", "uplink: create uplink/hooks"],
    )?;
    // An empty old value refuses to overwrite a branch created meanwhile.
    git(
        repo,
        &["update-ref", HOOKS_REF, &commit, ""],
        GitOpts::default(),
    )?;
    Ok(HooksOutcome::Created)
}

/// Add pack files missing from `uplink/hooks` in one commit on top of the
/// newest of the local branch and origin's. Existing files are kept as is.
fn complete_hooks_branch(repo: &Path, forge: Forge) -> Result<HooksOutcome> {
    let local = git_ok(repo, &["rev-parse", HOOKS_REF])?;
    let base = match fetch_state_tracking(repo, COMPANY_REMOTE, HOOKS_BRANCH)? {
        Some(remote) if remote == local || is_ancestor(repo, &remote, &local)? => local.clone(),
        Some(remote) if is_ancestor(repo, &local, &remote)? => remote,
        Some(_) => {
            return Ok(HooksOutcome::Skipped(format!(
                "uplink/hooks differs from origin; run git fetch {COMPANY_REMOTE} {HOOKS_BRANCH}, \
reconcile, and re-run git uplink init --upgrade"
            )));
        }
        None => local.clone(),
    };
    let missing: Vec<(String, Vec<u8>)> = hooks_files(forge)
        .into_iter()
        .filter(|(rel, _)| !path_exists_at(repo, &base, rel).unwrap_or(false))
        .collect();
    if missing.is_empty() && base == local {
        return Ok(HooksOutcome::Existing);
    }
    if hooks_checked_out(repo) {
        return Ok(HooksOutcome::Skipped(
            "uplink/hooks is checked out; switch to another branch and re-run git uplink init --upgrade"
                .into(),
        ));
    }
    let tip = if missing.is_empty() {
        base
    } else {
        let tree = build_tree(repo, Some(&base), &missing)?;
        git_ok(
            repo,
            &[
                "commit-tree",
                &tree,
                "-p",
                &base,
                "-m",
                "uplink: add missing hook files",
            ],
        )?
    };
    // Compare-and-swap against the branch we read.
    git(
        repo,
        &["update-ref", HOOKS_REF, &tip, &local],
        GitOpts::default(),
    )?;
    if missing.is_empty() {
        return Ok(HooksOutcome::FromOrigin);
    }
    Ok(HooksOutcome::Completed(
        missing.into_iter().map(|(rel, _)| rel).collect(),
    ))
}

fn hooks_checked_out(repo: &Path) -> bool {
    git_ok(repo, &["symbolic-ref", "--quiet", "HEAD"]).is_ok_and(|head| head == HOOKS_REF)
}

/// Write `files` into the tree of `base` (or an empty tree) and return the new tree.
fn build_tree(repo: &Path, base: Option<&str>, files: &[(String, Vec<u8>)]) -> Result<String> {
    let index = repo.join(format!(".git/uplink-hooks-index-{}", Uuid::new_v4()));
    let index_opts = GitOpts {
        extra_env: vec![(
            "GIT_INDEX_FILE".into(),
            index.to_string_lossy().into_owned(),
        )],
        ..GitOpts::default()
    };
    let outcome = (|| -> Result<String> {
        match base {
            Some(base) => git(repo, &["read-tree", base], index_opts.clone())?,
            None => git(repo, &["read-tree", "--empty"], index_opts.clone())?,
        };
        for (rel, bytes) in files {
            let blob = git(
                repo,
                &["hash-object", "-w", "--stdin"],
                GitOpts {
                    input: Some(bytes),
                    ..GitOpts::default()
                },
            )?
            .stdout;
            let info = format!("100644,{blob},{rel}");
            git(
                repo,
                &["update-index", "--add", "--cacheinfo", &info],
                index_opts.clone(),
            )?;
        }
        Ok(git(repo, &["write-tree"], index_opts.clone())?.stdout)
    })();
    let _ = std::fs::remove_file(&index);
    outcome
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HooksPushAction {
    /// No local `uplink/hooks`; nothing to publish.
    Absent,
    UpToDate,
    Pushed,
    /// Origin has commits the local branch lacks; nothing pushed.
    Behind,
    Diverged,
    /// The push was refused, for example by a ruleset or a token without workflows scope.
    Rejected(String),
}

/// Publish a local `uplink/hooks` that is new or ahead of `remote`. Never
/// force-pushes and never moves the local branch. A refused push is reported,
/// not raised, so it cannot fail the `uplink/state` push that ran before it.
pub fn push_hooks_branch(repo: &Path, remote: &str) -> Result<HooksPushAction> {
    if !has_ref(repo, HOOKS_REF)? {
        return Ok(HooksPushAction::Absent);
    }
    let local = git_ok(repo, &["rev-parse", HOOKS_REF])?;
    let action = match fetch_state_tracking(repo, remote, HOOKS_BRANCH)? {
        Some(remote_sha) if remote_sha == local => return Ok(HooksPushAction::UpToDate),
        None => HooksPushAction::Pushed,
        Some(remote_sha) if is_ancestor(repo, &remote_sha, &local)? => HooksPushAction::Pushed,
        Some(remote_sha) if is_ancestor(repo, &local, &remote_sha)? => {
            return Ok(HooksPushAction::Behind);
        }
        Some(_) => return Ok(HooksPushAction::Diverged),
    };
    match push_state_branch(repo, remote, HOOKS_BRANCH) {
        Ok(()) => Ok(action),
        Err(err) => Ok(HooksPushAction::Rejected(err.to_string())),
    }
}

/// Next step for `init` output when the local branch is not on origin yet.
/// Uses the remote-tracking ref only, so it never touches the network.
pub fn hooks_publish_hint(repo: &Path) -> Option<String> {
    let local = git_ok(repo, &["rev-parse", "--verify", "--quiet", HOOKS_REF]).ok()?;
    let tracking = git_ok(
        repo,
        &["rev-parse", "--verify", "--quiet", HOOKS_TRACKING_REF],
    )
    .ok();
    if tracking.as_deref() == Some(local.as_str()) {
        return None;
    }
    Some(format!("Publish hooks: {PUBLISH}"))
}

pub fn hooks_step_outcome(outcome: HooksOutcome) -> StepOutcome {
    match outcome {
        HooksOutcome::Existing => StepOutcome::pass("uplink/hooks present; left unchanged"),
        HooksOutcome::FromOrigin => StepOutcome::pass("uplink/hooks fetched from origin"),
        HooksOutcome::Created => StepOutcome::pass(format!(
            "uplink/hooks created locally; publish with: {PUBLISH}"
        )),
        HooksOutcome::Missing => StepOutcome::skip("uplink/hooks not on origin"),
        HooksOutcome::Completed(paths) => StepOutcome::pass(format!(
            "added {} to uplink/hooks; publish with: {PUBLISH}",
            paths.join(", ")
        )),
        HooksOutcome::Skipped(reason) => StepOutcome::warn(reason),
    }
}

/// Doctor check: the branch exists locally, carries the toolchain hook, and is
/// published on origin.
pub fn check_hooks_branch(repo: &Path, queue: &QueueState) -> StepOutcome {
    let Some(forge) = queue.config.forge else {
        return StepOutcome::skip("no forge recorded");
    };
    match forge.family() {
        ForgeFamily::Github => check_github_hooks_branch(repo),
    }
}

fn check_github_hooks_branch(repo: &Path) -> StepOutcome {
    let push = PUBLISH;
    let local = match has_ref(repo, HOOKS_REF) {
        Ok(true) => git_ok(repo, &["rev-parse", HOOKS_REF]).ok(),
        Ok(false) => None,
        Err(err) => return StepOutcome::fail(err.to_string()),
    };
    let remote = remote_hooks_sha(repo);
    let Some(local) = local else {
        return if matches!(remote, Ok(Some(_))) {
            StepOutcome::warn(
                "uplink/hooks is on origin but not local; run git uplink init to fetch it",
            )
        } else {
            StepOutcome::fail(format!(
                "uplink/hooks is missing; run git uplink init --upgrade, then {push}"
            ))
        };
    };
    if !path_exists_at(repo, &local, TOOLCHAIN_ACTION_PATH).unwrap_or(false) {
        return StepOutcome::fail(format!(
            "uplink/hooks has no {TOOLCHAIN_ACTION_PATH}; preflight jobs call it. \
Add it back (see toolchain-hook.md on uplink/hooks)"
        ));
    }
    let short = &local[..local.len().min(7)];
    match remote {
        // Like origin/uplink/state: unverifiable is a warning, not a failure.
        Err(detail) => StepOutcome::warn(format!(
            "uplink/hooks at {short}; cannot check origin ({detail}). Once origin exists: {push}"
        )),
        Ok(None) => StepOutcome::fail(format!(
            "uplink/hooks at {short} is not on origin; push it with: {push}"
        )),
        Ok(Some(remote)) if remote == local => {
            StepOutcome::pass(format!("uplink/hooks at {short}, pushed"))
        }
        Ok(Some(remote)) if is_ancestor(repo, &remote, &local).unwrap_or(false) => {
            StepOutcome::warn(format!(
                "uplink/hooks has local commits not on origin; push them with: {push}"
            ))
        }
        Ok(Some(_)) => StepOutcome::warn(format!(
            "uplink/hooks differs from origin; run git fetch {COMPANY_REMOTE} {HOOKS_BRANCH} and reconcile"
        )),
    }
}

fn remote_hooks_sha(repo: &Path) -> std::result::Result<Option<String>, String> {
    let result = git(
        repo,
        &["ls-remote", "--heads", COMPANY_REMOTE, HOOKS_REF],
        GitOpts::allow_fail(),
    )
    .map_err(|err| err.to_string())?;
    if result.code != 0 {
        return Err(format!(
            "could not read {COMPANY_REMOTE}: {}",
            result.stderr.trim()
        ));
    }
    Ok(result.stdout.split_whitespace().next().map(str::to_string))
}
