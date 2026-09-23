//! Patch-queue engine for contributing to public GitHub from an EMU enterprise.
//!
//! Install the binary as `git-uplink` on `PATH` and invoke it as `git uplink`.

mod adopt;
mod assess;
mod error;
mod gate;
mod git;
mod github;
mod lock;
mod ops;
mod preflight;
mod queue;
mod repo;
mod tooling;
mod tui;
mod types;
pub mod webui;

pub use adopt::{AdoptGroup, adopted_next_steps, load_groups_file};
pub use assess::{
    ApprovalReceipt, FROM_UPSTREAM_ENVIRONMENT, IncomingFlowedBack, TO_UPSTREAM_ENVIRONMENT,
    assert_assess_ok, assess_from_message, company_commit_message, depends_on_from_message,
    export_commit_message, format_approval_receipt, format_approver_packet, format_assess_markdown,
    format_contribution_packet, format_contribution_packet_with_extras,
    format_delta_approver_packet, format_incoming_packet, from_upstream_report_paths,
    load_extra_markdown, parse_depends_on, prepend_report_extras, report_paths,
    split_internal_message, strip_html_comments,
};
pub use error::{AssessError, ConflictError, Error, PreflightError, Result};
pub use git::{GitError, GitOpts, GitResult, configure_repo, git, git_ok};
pub use github::{parse_github_repo, parse_issue_url, parse_pull_request_url};
pub use ops::{
    AddPatchOpts, InitOpts, InitResult, PushOpts, PushResult, QueueCounts, RebuildOpts,
    RebuildResult, RefreshResult, ResetResult, StateStatus, StatusReport, StatusSnapshot,
    SubmitResult, SyncResult, TransferResult, accept_upstream, add_patch, approve_patch,
    approve_patch_at, drop_patch, format_status_table, init, init_repo, mark_merged, push_queue,
    read_queue, rebuild, rebuild_with, record_gated_pr, record_pull_request, refresh_from_origin,
    reset_from_origin, resolve_conflict, state_status_at, status_report, status_snapshot,
    submit_patch, summarize_queue, sync, transfer_patch, write_queue,
};
pub use preflight::{
    IncomingPreflight, assert_export_preflight, preflight_existing_patch, preflight_incoming_change,
};
pub use repo::{FileRevision, commit_queue, file_history, patch_state_commit, queue_at, show_at};
pub use types::{
    AssessReport, DEFAULT_CUTOFF, DEFAULT_EXPORT_AUTHOR, Forge, GateKind, MergeVia, Patch,
    PatchApproval, PatchLayer, PatchStatus, PendingUpstream, QUEUE_PATH, QUEUE_VERSION,
    QueueConfig, QueueState, STATE_BRANCH, TOOLING_PATCH_KIND, TOOLING_PATCH_TITLE,
    TransferDirection,
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
