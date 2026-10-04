use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::adopt::{self, AdoptGroup};
use crate::assess::{
    IncomingCommitRow, IncomingMergeRow, IncomingPacket, assert_assess_ok, assess_from_message,
    company_commit_message, depends_on_from_message, format_incoming_packet,
    from_upstream_report_paths, report_paths, review_token, store_extras, stored_commit_message,
};
use crate::error::{ConflictError, Error, Result};
use crate::gate::{
    assert_resolution_clean, commit_resolution, cut_gated_work, ensure_clean_worktree,
    format_patch_at_head, recover_onto,
};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::hooks::{
    HooksMode, HooksOutcome, HooksPushAction, ensure_hooks_branch, hooks_questions,
    hooks_step_outcome, push_hooks_branch,
};
use crate::init_report::InitReport;
use crate::inspect::{
    check_forge_tooling, check_remotes_configured, check_upstream_ref, check_urls_recorded,
};
use crate::lock::{is_push_lease_rejected, with_queue_lock};
use crate::preflight::{
    apply_abs, assert_export_preflight, assert_upstream_layer_applies, run_preflight_command_in,
};
use crate::progress::{ProgressMode, StepOutcome, StepProgress};
use crate::queue::{
    add_event, apply_order_active, apply_order_upstream_layer, cannot_depend_on, get_patch,
    get_patch_mut, is_active, move_patch, patch_path, read_queue as read_queue_file,
    write_queue as write_queue_file,
};
use crate::repo::{
    COMPANY_REMOTE, TempWorktree, UPSTREAM_REF, ahead_behind, apply_patch_file, apply_state_sha,
    commit_queue, conflicted_files, copy_dir, ensure_company_branch_ref, ensure_configured_remotes,
    ensure_revs, ensure_state_worktree, ensure_upstream_ref, fetch_state_tracking,
    fetch_tracking_sha, fetch_upstream, fetch_upstream_remote, has_ref, is_ancestor, merge_base,
    new_patch_id, patch_already_applied_on, path_exists_at, point_branch_at, promote_upstream,
    push_branch_force, push_state_branch, queue_at, refresh_company_branch, refresh_upstream_ref,
    replace_state_from_origin, restore_paths_from, rev_parse, set_state_branch, stable_patch_id,
    stable_patch_id_from_contents, stamp, state_exists, try_replace_state_from_origin,
    uplink_uncommitted_paths, write_product_patch,
};
use crate::settings::{SETTINGS_PATH, Settings, SettingsFlags, answer_settings};
use crate::types::{
    ApplyOutcome, AssessReport, Forge, GateKind, LastSync, MergeVia, Patch, PatchApproval,
    PatchConflict, PatchEvent, PatchIntent, PatchLayer, PatchMerged, PatchSource, PatchStatus,
    PatchUpstream, PendingMerge, PendingUpstream, QueueConfig, QueueState, STATE_BRANCH,
    TransferDirection,
};

mod add;
mod amend;
mod init;
mod lifecycle;
mod push;
mod rebuild;
mod status;
mod submit;
mod sync;
mod transfer;

pub use add::*;
pub use amend::*;
pub use init::*;
pub use lifecycle::*;
pub use push::*;
pub use rebuild::*;
pub use status::*;
pub use submit::*;
pub use sync::*;
pub use transfer::*;

pub fn read_queue(repo: &Path) -> Result<QueueState> {
    read_queue_file(repo)
}

pub fn write_queue(repo: &Path, queue: &QueueState) -> Result<()> {
    write_queue_file(repo, queue)
}

pub(super) fn snapshot_uplink(repo: &Path) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("uplink-{}", uuid::Uuid::new_v4()));
    copy_dir(&repo.join(".uplink"), &dir.join(".uplink"))?;
    Ok(dir)
}

/// Resets the `.uplink` worktree to the committed uplink/state, when it exists.
pub(super) fn restore_uplink_from_state(repo: &Path) -> Result<()> {
    let queue_ref = STATE_BRANCH;
    if has_ref(repo, queue_ref)? {
        git(
            repo,
            &[
                "restore",
                "--source",
                queue_ref,
                "--worktree",
                "--",
                ".uplink",
            ],
            GitOpts::default(),
        )?;
    }
    Ok(())
}
