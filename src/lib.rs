//! Patch-queue engine for contributing to public GitHub from an EMU enterprise.
//!
//! Install the binary as `git-uplink` on `PATH` and invoke it as `git uplink`.

mod adopt;
mod error;
mod git;
mod github;
mod lock;
mod ops;
mod preflight;
mod prepare;
mod queue;
mod repo;
mod tooling;
mod tui;
mod types;
pub mod webui;

pub use adopt::{AdoptGroup, adopted_next_steps, load_groups_file};
pub use error::{ConflictError, Error, PreflightError, PrepareError, Result};
pub use git::{GitError, GitOpts, GitResult, configure_repo, git, git_ok};
pub use github::{parse_github_repo, parse_issue_url, parse_pull_request_url};
pub use ops::{
    AddPatchOpts, InitOpts, PushOpts, PushResult, QueueCounts, RebuildOpts, RebuildResult,
    ResetResult, StatusSnapshot, SubmitResult, SyncResult, accept_upstream, add_patch,
    approve_patch, approve_patch_at, drop_patch, init, init_repo, mark_merged, push_queue,
    read_queue, rebuild, rebuild_with, record_conflict_issue, record_pull_request,
    reset_from_origin, resolve_conflict, status_snapshot, submit_patch, summarize_queue, sync,
    write_queue,
};
pub use preflight::{
    IncomingPreflight, assert_export_preflight, preflight_existing_patch, preflight_incoming_change,
};
pub use prepare::{
    ApprovalReceipt, FROM_UPSTREAM_ENVIRONMENT, IncomingFlowedBack, TO_UPSTREAM_ENVIRONMENT,
    assert_prepare_ok, company_commit_message, depends_on_from_message, export_commit_message,
    format_approval_receipt, format_approver_packet, format_contribution_packet,
    format_delta_approver_packet, format_incoming_packet, format_prepare_markdown,
    from_upstream_report_paths, parse_depends_on, prepare_from_message, report_paths,
    split_internal_message, strip_html_comments,
};
pub use repo::{commit_queue, patch_state_commit};
pub use types::{
    DEFAULT_CUTOFF, DEFAULT_EXPORT_AUTHOR, Forge, MergeVia, Patch, PatchApproval, PatchIntent,
    PatchStatus, PendingUpstream, PrepareReport, QUEUE_PATH, QueueConfig, QueueState, STATE_BRANCH,
    TOOLING_PATCH_KIND, TOOLING_PATCH_TITLE,
};

#[cfg(test)]
mod embed_tests {
    #[test]
    fn web_ui_index_is_embedded() {
        assert!(
            crate::webui::has_embedded_index(),
            "web/dist/index.html must be produced by build.rs and embedded"
        );
    }
}
