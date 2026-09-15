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

pub use error::{ConflictError, Error, PreflightError, PrepareError, Result};
pub use git::{configure_repo, git, git_ok, GitError, GitOpts, GitResult};
pub use github::{
    comment_on_issue, create_upstream_pull_request, get_pull_request, parse_github_repo,
    GithubConfig, GithubPr,
};
pub use ops::{
    add_patch, approve_patch, drop_patch, init_repo, mark_merged, read_queue, rebuild,
    record_pull_request, resolve_conflict, status_snapshot, submit_patch, summarize_queue, sync,
    write_queue, AddPatchOpts, QueueCounts, StatusSnapshot, SubmitResult,
};
pub use preflight::{
    assert_export_preflight, preflight_existing_patch, preflight_incoming_change, IncomingPreflight,
};
pub use prepare::{
    assert_prepare_ok, export_commit_message, format_approval_receipt, format_approver_packet,
    format_prepare_markdown, install_commit_template, prepare_from_range, report_paths,
    split_internal_message, ApprovalReceipt, OSS_ENVIRONMENT,
};
pub use repo::commit_queue;
pub use types::{
    MergeVia, Patch, PatchIntent, PatchStatus, PrepareReport, QueueConfig, QueueState,
    DEFAULT_CUTOFF, DEFAULT_EXPORT_AUTHOR, QUEUE_PATH,
};
