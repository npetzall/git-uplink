use std::path::Path;

use crate::error::Result;
use crate::hooks::check_hooks_branch;
use crate::init_report::InitReport;
use crate::inspect::{
    check_adopt_pending, check_branch_sync, check_credentials_contrib, check_credentials_internal,
    check_credentials_upstream, check_forge_recorded, check_forge_tooling, check_initialized,
    check_origin_state, check_remote_reachable, check_remotes_configured, check_upstream_ref,
    check_urls_recorded, check_workflows_on_main, read_queue_if_initialized,
};
use crate::progress::{ProgressMode, StepProgress};
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<crate::types::AssessCheck>,
}

pub fn doctor(repo: &Path, mode: ProgressMode) -> Result<DoctorReport> {
    let mut progress = StepProgress::from_mode(mode);
    progress.run_check("initialized", "Queue initialized", || {
        check_initialized(repo)
    });

    let queue = match read_queue_if_initialized(repo) {
        Some(queue) => queue,
        None => {
            return Ok(DoctorReport {
                ok: false,
                checks: progress.checks().to_vec(),
            });
        }
    };

    progress.run_check("urls-recorded", "URLs recorded", || {
        check_urls_recorded(&queue)
    });
    progress.run_check("remotes-configured", "Remotes configured", || {
        check_remotes_configured(repo, &queue)
    });
    progress.run_check("upstream-ref", "uplink/upstream ref", || {
        check_upstream_ref(repo)
            .unwrap_or_else(|err| crate::progress::StepOutcome::fail(err.to_string()))
    });
    progress.run_check("upstream-fetch", "Upstream remote reachable", || {
        check_remote_reachable(
            repo,
            &queue.config.upstream_remote,
            &queue.config.upstream_branch,
            "upstream",
        )
    });
    progress.run_check("contrib-reachable", "Contrib remote reachable", || {
        check_remote_reachable(
            repo,
            &queue.config.contrib_remote,
            &queue.config.internal_branch,
            "contrib",
        )
    });
    progress.run_check("origin-state", "origin/uplink/state", || {
        check_origin_state(repo)
            .unwrap_or_else(|err| crate::progress::StepOutcome::fail(err.to_string()))
    });
    progress.run_check("forge-recorded", "Forge recorded", || {
        check_forge_recorded(&queue)
    });
    progress.run_check("forge-tooling", "Forge tooling patch", || {
        check_forge_tooling(repo, &queue)
    });
    progress.run_check("workflows-on-main", "Workflows on company branch", || {
        check_workflows_on_main(repo, &queue)
            .unwrap_or_else(|err| crate::progress::StepOutcome::fail(err.to_string()))
    });
    progress.run_check("hooks-branch", "uplink/hooks branch", || {
        check_hooks_branch(repo, &queue)
    });
    progress.run_check("credentials-internal", "Internal credentials", || {
        check_credentials_internal(repo)
    });
    progress.run_check("credentials-upstream", "Upstream credentials", || {
        check_credentials_upstream(&queue)
    });
    progress.run_check("credentials-contrib", "Contrib credentials", || {
        check_credentials_contrib(&queue)
    });
    progress.run_check("adopt-pending", "Adoption state", || {
        check_adopt_pending(repo)
            .unwrap_or_else(|err| crate::progress::StepOutcome::fail(err.to_string()))
    });
    progress.run_check("branch-sync", "Company branch vs uplink/upstream", || {
        check_branch_sync(repo, &queue)
            .unwrap_or_else(|err| crate::progress::StepOutcome::fail(err.to_string()))
    });

    let report = InitReport::from_checks(progress.checks().to_vec());
    Ok(DoctorReport {
        ok: report.ok,
        checks: report.checks,
    })
}

pub fn format_doctor_summary(report: &DoctorReport) -> String {
    if report.ok {
        "Uplink doctor: all checks passed".into()
    } else {
        let failed = report
            .checks
            .iter()
            .filter(|c| c.status == crate::types::CheckStatus::Fail)
            .count();
        format!("Uplink doctor: {failed} check(s) failed")
    }
}
