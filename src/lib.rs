//! Patch-queue engine for contributing to public GitHub from an EMU enterprise.
//!
//! Install the binary as `git-uplink` on `PATH` and invoke it as `git uplink`.

mod error;
mod git;
mod github;
mod lock;
mod ops;
mod preflight;
mod prepare;
mod queue;
mod repo;
mod types;
pub mod webui;

pub use error::{ConflictError, Error, PreflightError, PrepareError, Result};
pub use git::{GitError, GitOpts, GitResult, configure_repo, git, git_ok};
pub use github::{
    GithubConfig, GithubPr, comment_on_issue, create_upstream_pull_request, get_pull_request,
    parse_github_repo,
};
pub use ops::{
    AddPatchOpts, QueueCounts, StatusSnapshot, SubmitResult, add_patch, approve_patch,
    approve_patch_at, drop_patch, init_repo, mark_merged, read_queue, rebuild, record_pull_request,
    resolve_conflict, status_snapshot, submit_patch, summarize_queue, sync, write_queue,
};
pub use preflight::{
    IncomingPreflight, assert_export_preflight, preflight_existing_patch, preflight_incoming_change,
};
pub use prepare::{
    ApprovalReceipt, OSS_ENVIRONMENT, assert_prepare_ok, company_commit_message,
    depends_on_from_message, export_commit_message, format_approval_receipt,
    format_approver_packet, format_contribution_packet, format_delta_approver_packet,
    format_prepare_markdown, parse_depends_on, prepare_from_message, report_paths,
    split_internal_message, strip_html_comments,
};
pub use repo::{commit_queue, patch_state_commit};
pub use types::{
    DEFAULT_CUTOFF, DEFAULT_EXPORT_AUTHOR, MergeVia, Patch, PatchApproval, PatchIntent,
    PatchStatus, PrepareReport, QUEUE_PATH, QueueConfig, QueueState, STATE_BRANCH,
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
