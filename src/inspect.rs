use std::path::Path;

use crate::adopt;
use crate::error::Result;
use crate::git::{
    GitOpts, contrib_auth_help, git, git_ok, has_contrib_credentials, has_internal_credentials,
    has_upstream_credentials, internal_auth_help, is_https_url, is_local_transport, remote_get_url,
    upstream_auth_help, urls_match,
};
use crate::progress::StepOutcome;
use crate::queue::{patch_path, read_queue as read_queue_file};
use crate::repo::{COMPANY_REMOTE, UPSTREAM_REF, has_ref, path_exists_at, state_exists};
use crate::types::{QUEUE_PATH, QueueState, STATE_BRANCH, TOOLING_PATCH_KIND};

pub fn check_initialized(repo: &Path) -> StepOutcome {
    if repo.join(QUEUE_PATH).is_file() || state_exists(repo).unwrap_or(false) {
        StepOutcome::pass("uplink/state and queue.json present")
    } else {
        StepOutcome::fail(
            "not initialized; run git uplink init --upstream <url> --contrib <url> --forge <forge>",
        )
    }
}

pub fn check_urls_recorded(queue: &QueueState) -> StepOutcome {
    let missing_upstream = queue
        .config
        .upstream_url
        .as_deref()
        .is_none_or(|s| s.is_empty());
    let missing_contrib = queue
        .config
        .contrib_url
        .as_deref()
        .is_none_or(|s| s.is_empty());
    if missing_upstream || missing_contrib {
        StepOutcome::fail(
            "queue.json is missing upstreamUrl or contribUrl; re-run git uplink init with both URLs",
        )
    } else {
        StepOutcome::pass("upstream and contrib URLs recorded")
    }
}

pub fn check_remotes_configured(repo: &Path, queue: &QueueState) -> StepOutcome {
    let opts = GitOpts::default();
    let mut problems = Vec::new();
    if let Some(url) = queue
        .config
        .upstream_url
        .as_deref()
        .filter(|u| !u.is_empty())
    {
        match remote_get_url(repo, &queue.config.upstream_remote, &opts) {
            Some(remote_url) if urls_match(&remote_url, url) => {}
            Some(remote_url) => problems.push(format!(
                "{} remote URL mismatch (stored {url}, git has {remote_url})",
                queue.config.upstream_remote
            )),
            None => problems.push(format!("{} remote missing", queue.config.upstream_remote)),
        }
    }
    if let Some(url) = queue
        .config
        .contrib_url
        .as_deref()
        .filter(|u| !u.is_empty())
    {
        match remote_get_url(repo, &queue.config.contrib_remote, &opts) {
            Some(remote_url) if urls_match(&remote_url, url) => {}
            Some(remote_url) => problems.push(format!(
                "{} remote URL mismatch (stored {url}, git has {remote_url})",
                queue.config.contrib_remote
            )),
            None => problems.push(format!("{} remote missing", queue.config.contrib_remote)),
        }
    }
    if problems.is_empty() {
        StepOutcome::pass("upstream and contrib remotes match queue.json")
    } else {
        StepOutcome::fail(problems.join("; "))
    }
}

pub fn check_upstream_ref(repo: &Path) -> Result<StepOutcome> {
    if has_ref(repo, UPSTREAM_REF)? {
        let sha = git_ok(repo, &["rev-parse", "--short", UPSTREAM_REF])?;
        Ok(StepOutcome::pass(format!("uplink/upstream at {sha}")))
    } else {
        Ok(StepOutcome::fail("uplink/upstream ref is missing"))
    }
}

pub fn check_remote_reachable(
    repo: &Path,
    remote: &str,
    branch: &str,
    role_label: &str,
) -> StepOutcome {
    let spec = format!("refs/heads/{branch}");
    match git(
        repo,
        &["ls-remote", "--heads", remote, &spec],
        GitOpts::default(),
    ) {
        Ok(result) if result.code == 0 && !result.stdout.trim().is_empty() => {
            StepOutcome::pass(format!("{role_label} reachable at {branch}"))
        }
        Ok(result) => StepOutcome::fail(format!(
            "{role_label} ls-remote failed: {}",
            result.stderr.trim()
        )),
        Err(err) => StepOutcome::fail(err.to_string()),
    }
}

pub fn check_origin_state(repo: &Path) -> Result<StepOutcome> {
    let tracking = format!("{COMPANY_REMOTE}/{STATE_BRANCH}");
    if has_ref(repo, &tracking)? {
        Ok(StepOutcome::pass(format!("{tracking} exists")))
    } else {
        Ok(StepOutcome::warn(format!(
            "{tracking} missing locally; run git uplink push after init"
        )))
    }
}

pub fn check_forge_recorded(queue: &QueueState) -> StepOutcome {
    if let Some(forge) = queue.config.forge {
        StepOutcome::pass(format!("forge {forge} recorded"))
    } else {
        StepOutcome::fail(
            "queue.json has no forge; re-run with --forge ghec or --forge example-github",
        )
    }
}

pub fn check_forge_tooling(repo: &Path, queue: &QueueState) -> StepOutcome {
    let patch = queue.tooling.as_ref().or_else(|| {
        queue
            .all_patches()
            .find(|p| p.kind.as_deref() == Some(TOOLING_PATCH_KIND))
    });
    match patch {
        Some(patch) => {
            let rel = patch_path(&patch.id).unwrap_or_default();
            if repo.join(rel).is_file() {
                StepOutcome::pass(format!("tooling patch {} on disk", patch.id))
            } else {
                StepOutcome::fail(format!(
                    "tooling patch {} missing from .uplink/patches",
                    patch.id
                ))
            }
        }
        None => StepOutcome::fail("no forge tooling patch in queue"),
    }
}

pub fn check_workflows_on_main(repo: &Path, queue: &QueueState) -> Result<StepOutcome> {
    let branch = &queue.config.internal_branch;
    if !has_ref(repo, branch)? {
        return Ok(StepOutcome::warn(format!("{branch} not found locally")));
    }
    let head = git_ok(repo, &["rev-parse", branch])?;
    let workflow = ".github/workflows/uplink-pr.yml";
    if path_exists_at(repo, &head, workflow)? {
        Ok(StepOutcome::pass(format!("{workflow} on {branch}")))
    } else if queue.tooling.is_some() {
        Ok(StepOutcome::warn(format!(
            "{workflow} not on {branch}; run git uplink rebuild"
        )))
    } else {
        Ok(StepOutcome::skip("forge tooling not installed yet"))
    }
}

pub fn check_credentials_internal(repo: &Path) -> StepOutcome {
    if remote_get_url(repo, COMPANY_REMOTE, &GitOpts::default()).is_none() {
        return StepOutcome::skip("origin remote not configured yet");
    }
    if remote_is_local(repo, COMPANY_REMOTE) {
        return StepOutcome::skip("origin is a local remote; credentials not required");
    }
    let opts = GitOpts::default();
    if has_internal_credentials(&opts) {
        StepOutcome::pass("UPLINK_INTERNAL_KEY or UPLINK_INTERNAL_TOKEN set")
    } else {
        StepOutcome::fail(internal_auth_help())
    }
}

pub fn check_credentials_upstream(queue: &QueueState) -> StepOutcome {
    if queue
        .config
        .upstream_url
        .as_deref()
        .is_some_and(is_local_transport)
    {
        return StepOutcome::skip("upstream URL is local; credentials not required");
    }
    let opts = GitOpts::default();
    if has_upstream_credentials(&opts) {
        StepOutcome::pass("UPLINK_UPSTREAM_KEY or UPLINK_UPSTREAM_TOKEN set")
    } else if queue
        .config
        .upstream_url
        .as_deref()
        .is_some_and(is_https_url)
    {
        StepOutcome::skip("public https upstream; credentials not required")
    } else {
        StepOutcome::fail(upstream_auth_help())
    }
}

pub fn check_credentials_contrib(queue: &QueueState) -> StepOutcome {
    if queue
        .config
        .contrib_url
        .as_deref()
        .is_some_and(is_local_transport)
    {
        return StepOutcome::skip("contrib URL is local; credentials not required");
    }
    let opts = GitOpts::default();
    if has_contrib_credentials(&opts) {
        StepOutcome::pass("UPLINK_CONTRIB_KEY or UPLINK_CONTRIB_TOKEN set")
    } else {
        StepOutcome::fail(contrib_auth_help())
    }
}

pub fn check_adopt_pending(repo: &Path) -> Result<StepOutcome> {
    if adopt::has_adopt_from(repo)? {
        Ok(StepOutcome::warn(
            "uplink/adopt-from is set; finish adoption with git uplink init",
        ))
    } else {
        Ok(StepOutcome::skip("no pending adoption"))
    }
}

pub fn check_branch_sync(repo: &Path, queue: &QueueState) -> Result<StepOutcome> {
    let internal = &queue.config.internal_branch;
    if !has_ref(repo, internal)? {
        return Ok(StepOutcome::warn(format!("{internal} not found locally")));
    }
    if !has_ref(repo, UPSTREAM_REF)? {
        return Ok(StepOutcome::skip("uplink/upstream missing"));
    }
    let internal_sha = git_ok(repo, &["rev-parse", internal])?;
    let upstream_sha = git_ok(repo, &["rev-parse", UPSTREAM_REF])?;
    if internal_sha == upstream_sha {
        Ok(StepOutcome::pass(format!(
            "{internal} matches uplink/upstream"
        )))
    } else {
        let (ahead, behind) = crate::repo::ahead_behind(repo, &internal_sha, &upstream_sha)?;
        if behind > 0 {
            Ok(StepOutcome::warn(format!(
                "{internal} is {behind} behind uplink/upstream"
            )))
        } else {
            Ok(StepOutcome::pass(format!(
                "{internal} is {ahead} ahead of uplink/upstream"
            )))
        }
    }
}

pub fn read_queue_if_initialized(repo: &Path) -> Option<QueueState> {
    if !repo.join(QUEUE_PATH).is_file() {
        return None;
    }
    read_queue_file(repo).ok()
}

fn remote_is_local(repo: &Path, name: &str) -> bool {
    remote_get_url(repo, name, &GitOpts::default())
        .as_deref()
        .is_some_and(is_local_transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::QueueConfig;

    #[test]
    fn remotes_configured_matches_ssh_and_https_forms() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).unwrap();
        git_ok(
            repo,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/acme/app.git",
            ],
        )
        .unwrap();
        let queue = QueueState::empty(QueueConfig {
            upstream_url: Some("git@github.com:acme/app.git".into()),
            ..QueueConfig::default()
        });
        let outcome = check_remotes_configured(repo, &queue);
        assert_eq!(
            outcome.status,
            crate::types::CheckStatus::Pass,
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn relative_path_contrib_needs_no_credentials() {
        let queue = QueueState::empty(QueueConfig {
            contrib_url: Some("../contrib.git".into()),
            ..QueueConfig::default()
        });
        assert_eq!(
            check_credentials_contrib(&queue).status,
            crate::types::CheckStatus::Skip
        );
    }
}
