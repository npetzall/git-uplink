//! Patch-queue engine for contributing to public GitHub from an EMU enterprise.
//!
//! Install the binary as `git-uplink` on `PATH` and invoke it as `git uplink`.

mod adopt;
mod assess;
mod doctor;
mod error;
mod gate;
mod git;
mod github;
mod hooks;
mod init_report;
mod inspect;
mod lock;
mod ops;
mod preflight;
mod progress;
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
    export_commit_message, extras_dir, format_approval_receipt, format_approver_packet,
    format_assess_checks_markdown, format_assess_markdown, format_contribution_packet,
    format_contribution_packet_with_extras, format_delta_approver_packet, format_incoming_packet,
    from_upstream_report_paths, load_extra_markdown, parse_depends_on, prepend_report_extras,
    report_paths, split_internal_message, store_extras, stored_commit_message, stored_extras_fresh,
    strip_html_comments,
};
pub use doctor::{DoctorReport, doctor, format_doctor_summary};
pub use error::{AssessError, ConflictError, Error, PreflightError, Result};
pub use git::{GitError, GitOpts, GitResult, git, git_ok, git_succeeds};
pub use github::{parse_github_repo, parse_issue_url, parse_pull_request_url};
pub use hooks::{HOOKS_BRANCH, HooksPushAction, TOOLCHAIN_ACTION_PATH, hooks_publish_hint};
pub use init_report::{InitReport, format_init_summary};
pub use ops::{
    AddPatchOpts, AmendMessage, AmendResult, InitOpts, InitResult, PushOpts, PushResult,
    QueueCounts, RebuildOpts, RebuildResult, RefreshResult, ResetResult, StateStatus, StatusReport,
    StatusSnapshot, SubmitResult, SyncResult, TransferResult, accept_upstream, add_patch,
    amend_patch, approve_patch, approve_patch_at, drop_patch, format_status_table, init, init_repo,
    init_repo_with_progress, mark_merged, push_queue, read_queue, rebuild, rebuild_with,
    record_gated_pr, record_pull_request, refresh_from_origin, reset_from_origin, resolve_conflict,
    state_status_at, status_report, status_snapshot, store_patch_extras, submit_patch,
    summarize_queue, sync, transfer_patch, write_queue,
};
pub use preflight::{
    IncomingPreflight, assert_export_preflight, preflight_existing_patch, preflight_incoming_change,
};
pub use progress::{ProgressMode, StepOutcome, StepProgress, format_step_line};
pub use repo::{FileRevision, commit_queue, file_history, patch_state_commit, queue_at, show_at};
pub use types::{
    AssessReport, CheckStatus, DEFAULT_CUTOFF, Forge, GateKind, MergeVia, Patch, PatchApproval,
    PatchExtras, PatchIntent, PatchLayer, PatchStatus, PendingUpstream, QUEUE_PATH, QUEUE_VERSION,
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
