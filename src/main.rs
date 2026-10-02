use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use git_uplink::{
    AddPatchOpts, AmendMessage, AmendResult, ApprovalReceipt, Error, FROM_UPSTREAM_ENVIRONMENT,
    Forge, IncomingPreflight, InitOpts, MergeVia, PreflightError, ProgressMode, PushOpts,
    RebuildOpts, STATE_BRANCH, TO_UPSTREAM_ENVIRONMENT, accept_upstream, add_patch,
    adopted_next_steps, amend_patch, approve_patch_at, assess_from_message, commit_queue, doctor,
    drop_patch, format_approval_receipt, format_assess_markdown,
    format_contribution_packet_with_extras, format_doctor_summary, format_init_summary,
    format_status_table, from_upstream_report_paths, git_ok, init, load_groups_file, mark_merged,
    parse_github_repo, parse_pull_request_url, preflight_existing_patch, preflight_incoming_change,
    push_queue, read_queue, rebuild_with, record_gated_pr, record_pull_request,
    refresh_from_origin, report_paths, reset_from_origin, resolve_conflict, status_report,
    status_snapshot, store_patch_extras, stored_commit_message, submit_patch, sync, transfer_patch,
};
use git_uplink::{
    Patch, PatchIntent, PatchStatus, QueueState, SubmitResult, SyncResult, TransferDirection,
    TransferResult,
};

const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("GIT_UPLINK_COMMIT"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "git-uplink",
    bin_name = "git uplink",
    version = VERSION,
    about = "Carry internal patches on upstream, contribute once, drop when merged.",
    long_about = "Developers open PRs and merge them; they never push main.\n\
add records a merged PR as a queued patch on uplink/state. assess uses the PR\n\
title and body as the single commit message, adds a co-author trailer, strips\n\
the internal section before contrib export, and scans for company affiliation.\n\
On GitHub Enterprise Cloud, contribution approval is the to-upstream Environment;\n\
approve/submit run after that review. git uplink talks to git only; workflows\n\
use gh for GitHub and follow-up commands (submitted, gated) to record results."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long)]
        contrib: Option<String>,
        #[arg(long = "upstream-remote-name")]
        upstream_remote_name: Option<String>,
        #[arg(long = "upstream-branch")]
        upstream_branch: Option<String>,
        #[arg(long = "contrib-remote-name")]
        contrib_remote_name: Option<String>,
        #[arg(long = "internal-branch")]
        internal_branch: Option<String>,
        #[arg(long, value_enum)]
        forge: Option<Forge>,
        #[arg(long)]
        upgrade: bool,
        #[arg(
            long = "adopt-groups",
            help = "JSON file of commit groups when internal is ahead of upstream"
        )]
        adopt_groups: Option<PathBuf>,
        #[arg(long, help = "Print queue config as JSON")]
        json: bool,
    },
    Add {
        #[arg(long)]
        title: String,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long, help = "Base revision (fetched from origin if missing)")]
        from: Option<String>,
        #[arg(long, help = "Head revision (fetched from origin if missing)")]
        head: Option<String>,
        #[arg(long)]
        internal_only: bool,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long)]
        pr_url: Option<String>,
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        #[arg(
            long = "extra-dir",
            help = "Directory of company assessment-hook *.md extras to store with the patch"
        )]
        extra_dir: Option<PathBuf>,
        #[arg(
            long = "extra-source",
            requires = "extra_dir",
            help = "Where the extras came from, such as the hook run URL"
        )]
        extra_source: Option<String>,
    },
    /// Publish local uplink/state, restacking unique patches if origin moved.
    Push {
        #[arg(long = "push-remote")]
        push_remote: Option<String>,
    },
    /// Fetch origin tracking refs without moving local branches.
    Refresh,
    /// Fetch origin and hard-reset company main, uplink/state, and uplink/upstream.
    Reset,
    Preflight {
        id: Option<String>,
        #[arg(long, help = "Base revision (fetched from origin if missing)")]
        from: Option<String>,
        #[arg(long, help = "Head revision (fetched from origin if missing)")]
        head: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        #[arg(long)]
        internal_only: bool,
    },
    Assess {
        #[arg(long, help = "Base revision (fetched from origin if missing)")]
        from: Option<String>,
        #[arg(long, help = "Head revision (fetched from origin if missing)")]
        head: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long)]
        internal_only: bool,
        #[arg(
            long,
            conflicts_with = "internal_only",
            help = "Assess a queued patch in its layer, with its stored title and message unless --title or --message[-file] is given (for example a conflict resolution or amend)"
        )]
        patch: Option<String>,
    },
    Report {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(
            long = "extra-dir",
            help = "Directory of *.md files prepended to the packet. Without it, extras stored for the unchanged patch are used"
        )]
        extra_dir: Option<PathBuf>,
        #[arg(
            long = "store-extras",
            requires = "extra_dir",
            help = "Also store --extra-dir as the patch's extras for later packets"
        )]
        store_extras: bool,
        #[arg(
            long = "extra-source",
            requires = "store_extras",
            help = "Where the stored extras came from, such as the hook run URL"
        )]
        extra_source: Option<String>,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Doctor {
        #[arg(long, help = "Print doctor report as JSON")]
        json: bool,
    },
    Approve {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Build the export commit on uplink/upstream. The forge creates the
    /// signed contrib commit from the printed `contribCommit`.
    Submit {
        id: String,
        /// Force-push the local, unsigned export commit to contrib instead.
        #[arg(long)]
        push: bool,
    },
    Submitted {
        id: String,
        #[arg(long = "pr-url")]
        pr_url: String,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long = "push-remote", default_value = "origin")]
        push_remote: String,
    },
    Sync,
    /// Promote a pending public main after from-upstream environment approval.
    #[command(name = "accept-upstream")]
    AcceptUpstream,
    /// Record the company PR that gates a conflict.
    #[command(alias = "conflicted")]
    Gated {
        id: String,
        #[arg(long = "pr-url")]
        pr_url: String,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long = "push-remote", default_value = "origin")]
        push_remote: String,
    },
    Merged {
        id: String,
        #[arg(long, value_enum, default_value_t = MergeVia::Manual)]
        via: MergeVia,
        #[arg(long)]
        sha: Option<String>,
    },
    Drop {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Rebuild {
        #[arg(
            long,
            help = "Rebuild onto uplink/preview/<name> instead of company main (preview; does not mutate the queue or push)"
        )]
        branch: Option<String>,
        #[arg(long, help = "Push uplink/state and the rebuilt branch after rebuild")]
        push: bool,
        #[arg(long = "push-remote")]
        push_remote: Option<String>,
    },
    Resolve {
        id: String,
    },
    /// Move a patch between the internal and upstream queues.
    #[command(group(
        clap::ArgGroup::new("direction")
            .required(true)
            .args(["to_upstream", "to_internal"])
    ))]
    Transfer {
        id: String,
        #[arg(long = "to-upstream")]
        to_upstream: bool,
        #[arg(long = "to-internal")]
        to_internal: bool,
        #[arg(long, help = "Finish a gated transfer after the work PR is merged")]
        complete: bool,
    },
    /// Revise a patch through a gated PR from uplink/amend/<id>-work.
    Amend {
        id: String,
        #[arg(long, help = "Fold the merged amend PR into the patch")]
        complete: bool,
        #[arg(long, requires = "complete", help = "New patch title")]
        title: Option<String>,
        #[arg(long, requires = "title", conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(
            long = "message-file",
            requires = "title",
            help = "New commit message, as the PR title and body (- reads stdin)"
        )]
        message_file: Option<PathBuf>,
    },
    /// Start the embedded operator dashboard on 127.0.0.1 and open a browser.
    #[command(name = "web-ui")]
    WebUi {
        #[arg(long, default_value_t = 43721)]
        port: u16,
        /// Do not launch a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Print the git uplink version and the commit it was built from.
    Version,
}

fn read_commit_message(
    message: Option<String>,
    message_file: Option<PathBuf>,
    fallback: &str,
) -> Result<String, Error> {
    if let Some(path) = message_file {
        let text = if path.as_os_str() == "-" {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        } else {
            fs::read_to_string(&path).map_err(|err| {
                Error::msg(format!(
                    "could not read --message-file {}: {err}",
                    path.display()
                ))
            })?
        };
        return Ok(text);
    }
    Ok(message.unwrap_or_else(|| fallback.to_string()))
}

fn write_markdown_file(repo: &Path, file: &Path, markdown: &str) -> Result<(), Error> {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        repo.join(file)
    };
    let mut body = markdown.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    abs.parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&abs, body))
        .map_err(|err| Error::msg(format!("could not write {}: {err}", abs.display())))
}

fn github_run_url() -> String {
    match (
        env::var("GITHUB_SERVER_URL"),
        env::var("GITHUB_REPOSITORY"),
        env::var("GITHUB_RUN_ID"),
    ) {
        (Ok(server), Ok(repository), Ok(run_id)) => {
            format!("{server}/{repository}/actions/runs/{run_id}")
        }
        _ => "not a GitHub Actions run".into(),
    }
}

/// Commit the approval is recorded against: uplink/state, else `GITHUB_SHA`, else HEAD.
fn state_sha(repo: &Path) -> String {
    git_ok(repo, &["rev-parse", STATE_BRANCH]).unwrap_or_else(|_| {
        env::var("GITHUB_SHA")
            .unwrap_or_else(|_| git_ok(repo, &["rev-parse", "HEAD"]).unwrap_or_default())
    })
}

struct Receipt {
    sha: String,
    run_url: String,
    text: String,
}

/// Approval receipt for `subject`; the environment name comes from `env_var`
/// or `default_env`.
fn build_receipt(repo: &Path, subject: &str, env_var: &str, default_env: &str) -> Receipt {
    let sha = state_sha(repo);
    let run_url = github_run_url();
    let environment = env::var(env_var).ok();
    let actor = env::var("GITHUB_ACTOR").ok();
    let text = format_approval_receipt(ApprovalReceipt {
        patch_id: subject,
        environment: environment.as_deref().unwrap_or(default_env),
        actor: actor.as_deref().unwrap_or("local operator"),
        run_url: &run_url,
        sha: &sha,
        at: None,
    });
    Receipt { sha, run_url, text }
}

/// `--pr` when given, otherwise the number parsed from `--pr-url`.
fn pr_number(pr: Option<u64>, pr_url: &str) -> Result<u64, Error> {
    pr.or_else(|| parse_pull_request_url(pr_url))
        .ok_or_else(|| Error::msg(format!("could not parse pull request number from {pr_url}")))
}

fn remote_url(repo: &Path, name: &str) -> Option<String> {
    git_ok(repo, &["remote", "get-url", name]).ok()
}

fn compare_url(onto: Option<&str>, branch: &str) -> Option<String> {
    let onto = onto.filter(|value| !value.is_empty())?;
    let server = env::var("GITHUB_SERVER_URL").ok()?;
    let repository = env::var("GITHUB_REPOSITORY").ok()?;
    Some(format!("{server}/{repository}/compare/{onto}...{branch}"))
}

fn preflight_comment(error: &PreflightError) -> String {
    let lines = if error.suggested_depends_on.is_empty() {
        "Uplink-Depends-On: upl_…".into()
    } else {
        error
            .suggested_depends_on
            .iter()
            .map(|id| format!("Uplink-Depends-On: {id}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Uplink export preflight failed ({}). This change is not ready to import or to open an upstream PR.\n\n\
Company `main` already includes other queued patches. Branching from it is not enough — record the patches this source actually needs, then retry.\n\n\
```\n{}\n```\n\n\
Add to the PR body (one per line) and import again:\n\n\
```\n{lines}\n```\n",
        error.stage, error
    )
}

fn print_failure_comment(err: &Error) {
    match err {
        Error::Preflight(pre) => print!("{}", preflight_comment(pre)),
        Error::Assess(pre) => print!("{}", format_assess_markdown(&pre.report)),
        _ => {}
    }
}

fn conflict_body(
    id: &str,
    work_branch: &str,
    onto: Option<&str>,
    resolved_from: Option<&str>,
) -> String {
    let intro = if let Some(from) = resolved_from {
        format!(
            "Rebuild after resolving `{from}` stopped on `{id}`. Checkout `{work_branch}`, remove the conflict markers, and open or update the PR into the protected base. Merge runs `git uplink resolve {id}`."
        )
    } else {
        format!(
            "Sync stopped on `{id}`. Checkout `{work_branch}`, remove the conflict markers, and push that work branch. Merge the PR into the protected base to run `git uplink resolve {id}`."
        )
    };
    let mut body = format!(
        "Company `main` is bot-owned. Resolve this conflict through the gated PR from `{work_branch}` into the protected base.\n\n\
{intro}\n\n\
Remaining patches wait until this id is resolved.\n"
    );
    if let Some(compare) = compare_url(onto, work_branch) {
        body.push_str(&format!(
            "\nCompare the failed apply (not frozen main): {compare}\n"
        ));
    }
    body
}

fn conflict_pr_create_artifact(
    repo: &Path,
    patch: &Patch,
    resolved_from: Option<&str>,
) -> Result<serde_json::Value, Error> {
    let conflict = patch.conflict.as_ref();
    let base = conflict.map(|c| c.branch.as_str()).unwrap_or("");
    let work = conflict
        .and_then(|c| c.work_branch.as_deref())
        .filter(|s| !s.is_empty())
        .unwrap_or(base);
    let onto = conflict.and_then(|c| c.onto.as_deref());
    let body = conflict_body(&patch.id, work, onto, resolved_from);
    let body_file = match report_paths(&patch.id) {
        Ok((dir, _, _)) => format!("{dir}/conflict.md"),
        Err(_) => return Ok(serde_json::json!({"error": "invalid patch id"})),
    };
    write_markdown_file(repo, Path::new(&body_file), &body)?;
    Ok(serde_json::json!({
        "title": format!("Uplink conflict: {}", patch.id),
        "bodyFile": body_file,
        "head": work,
        "base": base,
        "labels": ["uplink:conflict"]
    }))
}

/// The patch the queue is currently stopped on, if any.
fn find_conflict(queue: &QueueState) -> Option<&Patch> {
    queue
        .all_patches()
        .find(|p| p.status == PatchStatus::Conflict)
}

/// The `conflict` object in sync and resolve JSON.
fn conflict_json(patch: &Patch) -> serde_json::Value {
    let conflict = patch.conflict.as_ref();
    serde_json::json!({
        "id": patch.id,
        "branch": conflict.map(|c| &c.branch),
        "workBranch": conflict.and_then(|c| c.work_branch.clone()),
        "onto": conflict.and_then(|c| c.onto.clone()),
    })
}

fn eprint_conflict(patch: &Patch) {
    eprintln!(
        "CONFLICT {} on {}",
        patch.id,
        patch
            .conflict
            .as_ref()
            .map(|c| c.branch.as_str())
            .unwrap_or("")
    );
}

fn print_sync_artifact(
    repo: &Path,
    result: &SyncResult,
    summary: Option<&str>,
) -> Result<(), Error> {
    let queue = &result.queue;
    let conflict = find_conflict(queue);
    let summary = summary
        .filter(|text| !text.is_empty())
        .map(|text| serde_json::Value::String(text.to_string()))
        .unwrap_or(serde_json::Value::Null);
    let mut value = serde_json::json!({
        "lastSync": queue.last_sync,
        "needsApproval": result.needs_approval,
        "pendingSha": result.pending_sha,
        "flowedBack": result.flowed_back,
        "foreignCommits": result.foreign_commits,
        "reportPath": result.report_path,
        "summary": summary,
    });
    if let Some(patch) = conflict {
        value["conflict"] = conflict_json(patch);
        value["gh"] = serde_json::json!({
            "prCreate": conflict_pr_create_artifact(repo, patch, None)?,
        });
    }
    println!("{value}");
    Ok(())
}

fn print_transfer_artifact(repo: &Path, result: &TransferResult) -> Result<(), Error> {
    let mut value = serde_json::json!({
        "id": result.id,
        "direction": result.direction.as_str(),
        "transferred": result.transferred,
        "gated": result.gated,
    });
    if result.gated {
        value["base"] = serde_json::Value::String(result.base_branch.clone().unwrap_or_default());
        value["work"] = serde_json::Value::String(result.work_branch.clone().unwrap_or_default());
        value["onto"] = serde_json::Value::String(result.onto.clone().unwrap_or_default());
        let kind = result.direction.gate_kind();
        let body = format!(
            "Transfer of `{}` {} needs product changes (git conflict, assess, or preflight).\n\n\
Checkout `{}`, fix the tree, and merge this PR into the protected base. Closing the PR without merging aborts; the queue is unchanged.\n",
            result.id,
            result.direction.as_str(),
            result.work_branch.as_deref().unwrap_or("")
        );
        let body_file = match report_paths(&result.id) {
            Ok((dir, _, _)) => format!("{dir}/transfer.md"),
            Err(_) => "transfer.md".into(),
        };
        write_markdown_file(repo, Path::new(&body_file), &body)?;
        value["gh"] = serde_json::json!({
            "prCreate": {
                "title": format!("Uplink transfer {}: {}", result.direction.as_str(), result.id),
                "bodyFile": body_file,
                "head": result.work_branch,
                "base": result.base_branch,
                "labels": [kind.label()]
            }
        });
    } else if result.pr_close_url.is_some() || result.pr_close_branch.is_some() {
        let mut pr_close = serde_json::Map::new();
        if let Some(number) = result.pr_close_number {
            pr_close.insert("number".into(), serde_json::json!(number));
        }
        if let Some(url) = &result.pr_close_url {
            pr_close.insert("url".into(), serde_json::Value::String(url.clone()));
        }
        pr_close.insert(
            "comment".into(),
            serde_json::Value::String(format!(
                "Abandoned public PR; {} moved to internal.",
                result.id
            )),
        );
        if let Some(branch) = &result.pr_close_branch {
            pr_close.insert(
                "contribBranch".into(),
                serde_json::Value::String(branch.clone()),
            );
        }
        value["gh"] = serde_json::json!({ "prClose": pr_close });
    }
    println!("{value}");
    Ok(())
}

fn finish_sync(repo: &Path, result: SyncResult, summary: Option<&str>) -> Result<(), Error> {
    print_sync_artifact(repo, &result, summary)?;
    if let Some(conflict) = find_conflict(&result.queue) {
        eprint_conflict(conflict);
    }
    Ok(())
}

fn print_resolve_artifact(
    repo: &Path,
    resolved_id: &str,
    queue: &QueueState,
    follow_on_conflict: bool,
) -> Result<(), Error> {
    println!(
        "{}",
        resolve_artifact(repo, resolved_id, queue, follow_on_conflict)?
    );
    Ok(())
}

/// `{id, status}` plus the follow-on conflict and its PR, if rebuild stopped.
fn resolve_artifact(
    repo: &Path,
    resolved_id: &str,
    queue: &QueueState,
    follow_on_conflict: bool,
) -> Result<serde_json::Value, Error> {
    let mut gh = serde_json::Map::new();
    let conflict = find_conflict(queue);
    if follow_on_conflict && let Some(patch) = conflict {
        gh.insert(
            "prCreate".into(),
            conflict_pr_create_artifact(repo, patch, Some(resolved_id))?,
        );
    }
    let status = queue
        .all_patches()
        .find(|p| p.id == resolved_id)
        .map(|p| p.status.as_str())
        .unwrap_or("");
    let mut value = serde_json::json!({
        "id": resolved_id,
        "status": status,
    });
    if let Some(patch) = conflict.filter(|_| follow_on_conflict) {
        value["conflict"] = conflict_json(patch);
    }
    if !gh.is_empty() {
        value["gh"] = serde_json::Value::Object(gh);
    }
    Ok(value)
}

/// The amend PR description: instructions in a comment (stripped on merge),
/// then the stored message body, which the author may edit.
fn amend_body(patch: &Patch, work: &str, base: &str) -> String {
    let stored = stored_commit_message(patch);
    let body = match stored.split_once('\n') {
        Some((subject, rest)) if subject.trim() == patch.title.trim() => rest.trim(),
        None if stored.trim() == patch.title.trim() => "",
        _ => stored.trim(),
    };
    format!(
        "<!--\n\
Uplink amend for `{id}`. Push the change to `{work}`, mark this PR ready, and merge it into `{base}`.\n\
On merge, the PR title and this description become the patch title and commit message. Text below the internal cutoff line stays internal.\n\
Closing without merging leaves the patch unchanged.\n\
-->\n\n{body}\n",
        id = patch.id,
    )
}

fn print_amend_artifact(repo: &Path, result: &AmendResult) -> Result<(), Error> {
    if result.completed {
        let mut value = resolve_artifact(repo, &result.id, &result.queue, false)?;
        value["completed"] = serde_json::Value::Bool(true);
        value["changed"] = serde_json::Value::Bool(result.changed);
        println!("{value}");
        return Ok(());
    }
    let patch = result
        .queue
        .all_patches()
        .find(|p| p.id == result.id)
        .ok_or_else(|| Error::msg(format!("unknown patch {}", result.id)))?;
    let base = result.base_branch.clone().unwrap_or_default();
    let work = result.work_branch.clone().unwrap_or_default();
    let body_file = match report_paths(&result.id) {
        Ok((dir, _, _)) => format!("{dir}/amend.md"),
        Err(_) => "amend.md".into(),
    };
    write_markdown_file(
        repo,
        Path::new(&body_file),
        &amend_body(patch, &work, &base),
    )?;
    let value = serde_json::json!({
        "id": result.id,
        "completed": false,
        "gated": true,
        "base": base,
        "work": work,
        "onto": result.onto,
        "gh": {
            "prCreate": {
                "title": patch.title,
                "bodyFile": body_file,
                "head": work,
                "base": base,
                "labels": [git_uplink::GateKind::Amend.label()],
                "draft": true
            }
        }
    });
    println!("{value}");
    Ok(())
}

fn submit_artifact(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    exported: &SubmitResult,
) -> Result<(), Error> {
    let branch = exported.branch.as_str();
    let body = format!(
        "Company contribution exported by Uplink.\n\nUplink-Patch-Id: {}\n",
        patch.id
    );
    let Ok((report_dir, _, _)) = report_paths(&patch.id) else {
        return Ok(());
    };
    let body_file = format!("{report_dir}/pr.md");
    write_markdown_file(repo, Path::new(&body_file), &body)?;
    let existing = patch.upstream.as_ref().and_then(|u| {
        u.pr_url.as_ref().map(|url| {
            serde_json::json!({
                "number": u.pr_number,
                "url": url,
            })
        })
    });
    let mut gh = serde_json::Map::new();
    if existing.is_none() {
        let upstream_url = queue
            .config
            .upstream_url
            .clone()
            .or_else(|| remote_url(repo, &queue.config.upstream_remote));
        let contrib_url = queue
            .config
            .contrib_url
            .clone()
            .or_else(|| remote_url(repo, &queue.config.contrib_remote));
        if let (Some((uo, ur)), Some((co, _))) = (
            upstream_url.as_deref().and_then(parse_github_repo),
            contrib_url.as_deref().and_then(parse_github_repo),
        ) {
            gh.insert(
                "prCreate".into(),
                serde_json::json!({
                    "repo": format!("{uo}/{ur}"),
                    "head": format!("{co}:{branch}"),
                    "base": queue.config.upstream_branch,
                    "title": patch.title,
                    "bodyFile": body_file,
                }),
            );
        }
    }
    let mut value = serde_json::json!({
        "id": patch.id,
        "branch": branch,
        "sha": exported.sha,
        "pushed": exported.pushed,
        "existingPr": existing,
    });
    if !exported.pushed {
        let message_file = format!("{report_dir}/commit-message.txt");
        write_markdown_file(repo, Path::new(&message_file), &exported.message)?;
        value["contribCommit"] = serde_json::json!({
            "branch": branch,
            "baseSha": exported.base,
            "localSha": exported.sha,
            "treeSha": exported.tree,
            "messageFile": message_file,
        });
    }
    if !gh.is_empty() {
        value["gh"] = serde_json::Value::Object(gh);
    }
    println!("{value}");
    Ok(())
}

fn upgrade_next_steps(branch: &str) -> String {
    format!(
        "tooling has been updated\n\
Company main was rebuilt locally. Nothing was pushed.\n\
Inspect with: git diff origin/{branch} {branch}\n\
Publish state: git uplink push\n\
Publish main: git uplink rebuild --push"
    )
}

struct InitArgs {
    upstream: Option<String>,
    contrib: Option<String>,
    upstream_remote_name: Option<String>,
    upstream_branch: Option<String>,
    contrib_remote_name: Option<String>,
    internal_branch: Option<String>,
    forge: Option<Forge>,
    upgrade: bool,
    adopt_groups: Option<PathBuf>,
    json: bool,
}

fn cmd_init(repo: &Path, args: InitArgs) -> Result<(), Error> {
    let adopt_groups = match args.adopt_groups {
        Some(path) => Some(load_groups_file(&path)?),
        None => None,
    };
    let hydrate = args.upstream.is_none()
        && args.contrib.is_none()
        && args.upstream_remote_name.is_none()
        && args.upstream_branch.is_none()
        && args.contrib_remote_name.is_none()
        && args.internal_branch.is_none()
        && adopt_groups.is_none()
        && args.forge.is_none()
        && !args.upgrade;
    let opts = InitOpts {
        upstream_url: args.upstream,
        contrib_url: args.contrib,
        upstream_remote_name: args.upstream_remote_name,
        upstream_branch: args.upstream_branch,
        contrib_remote_name: args.contrib_remote_name,
        internal_branch: args.internal_branch,
        forge: args.forge,
        upgrade: args.upgrade,
        adopt_groups,
        interactive: None,
        progress: if hydrate {
            ProgressMode::Disabled
        } else {
            ProgressMode::Auto
        },
    };
    let result = init(repo, opts)?;
    let queue = &result.queue;
    if queue.all_patches().any(|p| {
        p.source
            .note
            .as_deref()
            .is_some_and(|n| n.starts_with("adopted from "))
    }) {
        eprintln!("{}", adopted_next_steps());
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&queue.config)?);
    } else if !hydrate {
        println!("{}", format_init_summary(&result.report));
        if args.upgrade {
            if result.tooling_changed {
                println!("{}", upgrade_next_steps(&queue.config.internal_branch));
            } else {
                println!("already up-to-date");
            }
        }
    }
    if !result.report.ok {
        return Err(Error::msg(format_init_summary(&result.report)));
    }
    Ok(())
}

fn cmd_doctor(repo: &Path, json: bool) -> Result<(), Error> {
    let report = doctor(repo, ProgressMode::Auto)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", format_doctor_summary(&report));
    }
    if !report.ok {
        return Err(Error::msg(format_doctor_summary(&report)));
    }
    Ok(())
}

fn cmd_add(repo: &Path, opts: AddPatchOpts) -> Result<(), Error> {
    let internal_only = opts.internal_only;
    match add_patch(repo, opts) {
        Ok(patch) => {
            println!(
                "{}  {}  {}  {}",
                patch.id,
                if internal_only {
                    "internal"
                } else {
                    "upstream"
                },
                patch.status,
                patch.title
            );
            Ok(())
        }
        Err(err) => {
            print_failure_comment(&err);
            Err(err)
        }
    }
}

fn cmd_push(repo: &Path, push_remote: Option<String>) -> Result<(), Error> {
    let result = push_queue(
        repo,
        PushOpts {
            push_remote: Some(push_remote.unwrap_or_else(|| "origin".into())),
        },
    )?;
    println!(
        "{} {} to {} at {}",
        result.action, result.branch, result.remote, result.sha
    );
    Ok(())
}

fn cmd_refresh(repo: &Path) -> Result<(), Error> {
    let result = refresh_from_origin(repo)?;
    println!("origin/{} {}", result.internal_branch, result.internal_sha);
    println!("origin/{} {}", STATE_BRANCH, result.state_sha);
    println!("origin/uplink/upstream {}", result.upstream_sha);
    Ok(())
}

fn cmd_reset(repo: &Path) -> Result<(), Error> {
    let result = reset_from_origin(repo)?;
    println!("{} {}", result.internal_branch, result.internal_sha);
    println!("{} {}", STATE_BRANCH, result.state_sha);
    println!("uplink/upstream {}", result.upstream_sha);
    Ok(())
}

fn cmd_assess(
    repo: &Path,
    from: Option<String>,
    head: Option<String>,
    title: String,
    message: String,
    internal_only: bool,
) -> Result<(), Error> {
    let queue = read_queue(repo)?;
    let report = assess_from_message(
        repo,
        &queue,
        from.as_deref().unwrap_or("main"),
        head.as_deref().unwrap_or("HEAD"),
        &message,
        Some(&title),
        PatchIntent::from_internal_only(internal_only),
    )?;
    let markdown = format_assess_markdown(&report);
    println!("{markdown}");
    if !report.ok {
        return Err(Error::msg("assess failed"));
    }
    Ok(())
}

fn cmd_report(
    repo: &Path,
    id: &str,
    out: Option<PathBuf>,
    extra_dir: Option<PathBuf>,
    store_extras: bool,
    extra_source: Option<String>,
) -> Result<(), Error> {
    if store_extras && let Some(dir) = extra_dir.as_deref() {
        store_patch_extras(repo, id, dir, extra_source)?;
    }
    let queue = read_queue(repo)?;
    let patch = queue
        .all_patches()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
    let packet = format_contribution_packet_with_extras(repo, patch, extra_dir.as_deref())?;
    let default_out = report_paths(id)?.1;
    let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
    write_markdown_file(repo, &dest, &packet)?;
    commit_queue(repo, &format!("uplink: contribution packet {id}"))?;
    println!("{packet}");
    eprintln!("Wrote {}", dest.display());
    Ok(())
}

fn cmd_preflight(
    repo: &Path,
    id: Option<String>,
    incoming: IncomingPreflight,
) -> Result<(), Error> {
    let result = if let Some(id) = id {
        let queue = read_queue(repo)?;
        preflight_existing_patch(repo, &queue, &id)
    } else {
        preflight_incoming_change(repo, incoming)
    };
    match result {
        Ok(()) => {
            println!("export preflight passed");
            Ok(())
        }
        Err(err) => {
            print_failure_comment(&err);
            Err(err)
        }
    }
}

fn cmd_status(repo: &Path, json: bool) -> Result<(), Error> {
    let snapshot = status_snapshot(repo)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status_report(&snapshot))?
        );
    } else {
        print!("{}", format_status_table(&snapshot));
    }
    Ok(())
}

fn cmd_approve(repo: &Path, id: &str, out: Option<PathBuf>) -> Result<(), Error> {
    let queue = read_queue(repo)?;
    if !queue.all_patches().any(|p| p.id == id) {
        return Err(Error::msg(format!("unknown patch {id}")));
    }
    let receipt = build_receipt(
        repo,
        id,
        "UPLINK_TO_UPSTREAM_ENVIRONMENT",
        TO_UPSTREAM_ENVIRONMENT,
    );
    let default_out = report_paths(id)?.2;
    let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
    write_markdown_file(repo, &dest, &receipt.text)?;
    let patch = approve_patch_at(repo, id, Some(&receipt.sha), Some(&receipt.run_url))?;
    commit_queue(repo, &format!("uplink: to-upstream approval receipt {id}"))?;
    println!("{}", receipt.text);
    eprintln!("{} approved", patch.id);
    eprintln!("Wrote {}", dest.display());
    Ok(())
}

fn cmd_submit(repo: &Path, id: &str, push: bool) -> Result<(), Error> {
    let queue = read_queue(repo)?;
    let patch = queue
        .all_patches()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
    let exported = match submit_patch(repo, id, push) {
        Ok(v) => v,
        Err(err) => {
            print_failure_comment(&err);
            return Err(err);
        }
    };
    submit_artifact(repo, &queue, &patch, &exported)
}

fn cmd_submitted(
    repo: &Path,
    id: &str,
    pr_url: &str,
    pr: Option<u64>,
    push_remote: &str,
) -> Result<(), Error> {
    let number = pr_number(pr, pr_url)?;
    let branch = format!("uplink/{id}");
    let patch = record_pull_request(repo, id, number, pr_url, &branch, Some(push_remote))?;
    println!("{} submitted as {pr_url}", patch.id);
    Ok(())
}

fn cmd_sync(repo: &Path) -> Result<(), Error> {
    let result = sync(repo)?;
    let summary = result.report.clone();
    finish_sync(repo, result, summary.as_deref())
}

fn cmd_accept_upstream(repo: &Path) -> Result<(), Error> {
    let queue = read_queue(repo)?;
    if queue.pending_upstream.is_none() {
        return Err(Error::msg(
            "No pending upstream to accept. Run `git uplink sync` first.",
        ));
    }
    let receipt = build_receipt(
        repo,
        "incoming",
        "UPLINK_FROM_UPSTREAM_ENVIRONMENT",
        FROM_UPSTREAM_ENVIRONMENT,
    );
    let dest = PathBuf::from(from_upstream_report_paths().2);
    write_markdown_file(repo, &dest, &receipt.text)?;
    finish_sync(repo, accept_upstream(repo)?, Some(&receipt.text))
}

fn cmd_gated(
    repo: &Path,
    id: &str,
    pr_url: &str,
    pr: Option<u64>,
    push_remote: &str,
) -> Result<(), Error> {
    let number = pr_number(pr, pr_url)?;
    let patch = record_gated_pr(repo, id, number, pr_url, Some(push_remote))?;
    println!("{} conflict PR {pr_url}", patch.id);
    Ok(())
}

fn cmd_merged(repo: &Path, id: &str, via: MergeVia, sha: Option<&str>) -> Result<(), Error> {
    mark_merged(repo, id, via, sha)?;
    rebuild_with(repo, RebuildOpts::default())?;
    println!("{id} marked merged via {}", via.as_str());
    Ok(())
}

fn cmd_drop(repo: &Path, id: &str, reason: Option<&str>) -> Result<(), Error> {
    drop_patch(repo, id, reason.unwrap_or("dropped by operator"))?;
    println!("{id} dropped");
    Ok(())
}

fn cmd_rebuild(
    repo: &Path,
    branch: Option<String>,
    push: bool,
    push_remote: Option<String>,
) -> Result<(), Error> {
    let result = rebuild_with(
        repo,
        RebuildOpts {
            branch,
            push,
            push_remote: if push {
                Some(push_remote.unwrap_or_else(|| "origin".into()))
            } else {
                push_remote
            },
        },
    )?;
    if result.preview {
        println!("rebuild preview at {}", result.branch);
        eprintln!(
            "Inspect with: git diff {} {}",
            result.queue.config.internal_branch, result.branch
        );
    } else {
        println!("rebuild complete");
    }
    Ok(())
}

fn cmd_resolve(repo: &Path, id: &str) -> Result<(), Error> {
    match resolve_conflict(repo, id) {
        Ok(queue) => print_resolve_artifact(repo, id, &queue, false),
        Err(Error::Conflict(_)) => {
            let queue = read_queue(repo)?;
            print_resolve_artifact(repo, id, &queue, true)?;
            if let Some(conflict) = find_conflict(&queue) {
                eprint_conflict(conflict);
            }
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn cmd_amend(
    repo: &Path,
    id: &str,
    complete: bool,
    change: Option<AmendMessage>,
) -> Result<(), Error> {
    match amend_patch(repo, id, complete, change) {
        Ok(result) => {
            print_amend_artifact(repo, &result)?;
            if !result.completed {
                eprintln!(
                    "AMEND {} on {}",
                    result.id,
                    result.work_branch.as_deref().unwrap_or("")
                );
            }
            Ok(())
        }
        Err(Error::Conflict(_)) if complete => {
            let queue = read_queue(repo)?;
            let mut value = resolve_artifact(repo, id, &queue, true)?;
            value["completed"] = serde_json::Value::Bool(true);
            value["changed"] = serde_json::Value::Bool(true);
            println!("{value}");
            if let Some(conflict) = find_conflict(&queue) {
                eprint_conflict(conflict);
            }
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn cmd_transfer(
    repo: &Path,
    id: &str,
    direction: TransferDirection,
    complete: bool,
) -> Result<(), Error> {
    let result = transfer_patch(repo, id, direction, complete)?;
    print_transfer_artifact(repo, &result)?;
    if result.gated {
        eprintln!(
            "TRANSFER {} needs work on {}",
            result.id,
            result.work_branch.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn cmd_web_ui(repo: PathBuf, port: u16, no_open: bool) -> Result<(), Error> {
    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::msg(format!("tokio runtime: {err}")))?;
    runtime
        .block_on(git_uplink::webui::serve(repo, addr, !no_open))
        .map_err(|err| Error::msg(format!("web-ui server: {err}")))
}

fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    let repo = env::current_dir()?;
    match cli.command {
        Commands::Init {
            upstream,
            contrib,
            upstream_remote_name,
            upstream_branch,
            contrib_remote_name,
            internal_branch,
            forge,
            upgrade,
            adopt_groups,
            json,
        } => cmd_init(
            &repo,
            InitArgs {
                upstream,
                contrib,
                upstream_remote_name,
                upstream_branch,
                contrib_remote_name,
                internal_branch,
                forge,
                upgrade,
                adopt_groups,
                json,
            },
        ),
        Commands::Doctor { json } => cmd_doctor(&repo, json),
        Commands::Add {
            title,
            message,
            message_file,
            from,
            head,
            internal_only,
            pr,
            pr_url,
            depends_on,
            extra_dir,
            extra_source,
        } => {
            let message = read_commit_message(message, message_file, &title)?;
            cmd_add(
                &repo,
                AddPatchOpts {
                    title,
                    message: Some(message),
                    internal_only,
                    from_ref: from,
                    head_ref: head,
                    depends_on,
                    author: env::var("GIT_AUTHOR_NAME").ok(),
                    internal_pr_number: pr,
                    internal_pr_url: pr_url,
                    extra_dir,
                    extra_source,
                    ..Default::default()
                },
            )
        }
        Commands::Push { push_remote } => cmd_push(&repo, push_remote),
        Commands::Refresh => cmd_refresh(&repo),
        Commands::Reset => cmd_reset(&repo),
        Commands::Assess {
            from,
            head,
            title,
            message,
            message_file,
            internal_only,
            patch,
        } => {
            if let Some(id) = patch {
                let queue = read_queue(&repo)?;
                let stored = queue
                    .all_patches()
                    .find(|p| p.id == id)
                    .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
                let title = title.unwrap_or_else(|| stored.title.clone());
                let message = if message.is_some() || message_file.is_some() {
                    read_commit_message(message, message_file, &title)?
                } else {
                    stored_commit_message(stored)
                };
                let internal_only = !queue.is_upstream(&id);
                return cmd_assess(&repo, from, head, title, message, internal_only);
            }
            let title = title.unwrap_or_else(|| "candidate change".into());
            let message = read_commit_message(message, message_file, &title)?;
            cmd_assess(&repo, from, head, title, message, internal_only)
        }
        Commands::Report {
            id,
            out,
            extra_dir,
            store_extras,
            extra_source,
        } => cmd_report(&repo, &id, out, extra_dir, store_extras, extra_source),
        Commands::Preflight {
            id,
            from,
            head,
            title,
            message,
            message_file,
            depends_on,
            internal_only,
        } => {
            let title = title.unwrap_or_else(|| "candidate change".into());
            let message = read_commit_message(message, message_file, &title)?;
            cmd_preflight(
                &repo,
                id,
                IncomingPreflight {
                    title,
                    from_ref: from.unwrap_or_else(|| "main".into()),
                    head_ref: head.unwrap_or_else(|| "HEAD".into()),
                    depends_on,
                    message: Some(message),
                    preflight_command: None,
                    internal_only,
                },
            )
        }
        Commands::Status { json } => cmd_status(&repo, json),
        Commands::Approve { id, out } => cmd_approve(&repo, &id, out),
        Commands::Submit { id, push } => cmd_submit(&repo, &id, push),
        Commands::Submitted {
            id,
            pr_url,
            pr,
            push_remote,
        } => cmd_submitted(&repo, &id, &pr_url, pr, &push_remote),
        Commands::Sync => cmd_sync(&repo),
        Commands::AcceptUpstream => cmd_accept_upstream(&repo),
        Commands::Gated {
            id,
            pr_url,
            pr,
            push_remote,
        } => cmd_gated(&repo, &id, &pr_url, pr, &push_remote),
        Commands::Merged { id, via, sha } => cmd_merged(&repo, &id, via, sha.as_deref()),
        Commands::Drop { id, reason } => cmd_drop(&repo, &id, reason.as_deref()),
        Commands::Rebuild {
            branch,
            push,
            push_remote,
        } => cmd_rebuild(&repo, branch, push, push_remote),
        Commands::Resolve { id } => cmd_resolve(&repo, &id),
        Commands::Transfer {
            id,
            to_upstream,
            to_internal: _,
            complete,
        } => {
            // clap requires exactly one of --to-upstream / --to-internal.
            let direction = if to_upstream {
                TransferDirection::ToUpstream
            } else {
                TransferDirection::ToInternal
            };
            cmd_transfer(&repo, &id, direction, complete)
        }
        Commands::Amend {
            id,
            complete,
            title,
            message,
            message_file,
        } => {
            let change = match title {
                Some(title) => {
                    let message = if message.is_some() || message_file.is_some() {
                        Some(read_commit_message(message, message_file, &title)?)
                    } else {
                        None
                    };
                    Some(AmendMessage { title, message })
                }
                None => None,
            };
            cmd_amend(&repo, &id, complete, change)
        }
        Commands::WebUi { port, no_open } => cmd_web_ui(repo, port, no_open),
        Commands::Version => {
            println!("git-uplink {VERSION}");
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_ui_has_no_bind_option() {
        assert!(Cli::try_parse_from(["git-uplink", "web-ui", "--bind", "0.0.0.0"]).is_err());
        assert!(Cli::try_parse_from(["git-uplink", "web-ui", "--port", "1", "--no-open"]).is_ok());
    }

    fn merged_via(args: &[&str]) -> Option<MergeVia> {
        let argv = ["git-uplink", "merged", "upl_x"].iter().chain(args);
        match Cli::try_parse_from(argv).ok()?.command {
            Commands::Merged { via, .. } => Some(via),
            _ => None,
        }
    }

    #[test]
    fn write_markdown_file_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        write_markdown_file(repo, Path::new("out/report.md"), "# hi").unwrap();
        assert_eq!(
            fs::read_to_string(repo.join("out/report.md")).unwrap(),
            "# hi\n"
        );
        // A regular file where a directory is needed cannot be written through.
        let err = write_markdown_file(repo, Path::new("out/report.md/x.md"), "x").unwrap_err();
        assert!(err.to_string().contains("could not write"), "{err}");
    }

    #[test]
    fn conflict_json_keeps_camel_case_keys() {
        let patch: Patch = serde_json::from_value(serde_json::json!({
            "id": "upl_x",
            "title": "x",
            "status": "conflict",
            "dependsOn": [],
            "createdAt": "t",
            "updatedAt": "t",
            "source": {},
            "events": [],
            "conflict": {
                "branch": "uplink/conflict/upl_x",
                "workBranch": "uplink/conflict/upl_x-work",
                "files": [],
                "message": "m",
                "onto": "abc"
            }
        }))
        .unwrap();
        assert_eq!(
            conflict_json(&patch),
            serde_json::json!({
                "id": "upl_x",
                "branch": "uplink/conflict/upl_x",
                "workBranch": "uplink/conflict/upl_x-work",
                "onto": "abc",
            })
        );
    }

    #[test]
    fn pr_number_prefers_flag_then_url() {
        let url = "https://github.com/acme/app/pull/42";
        assert_eq!(pr_number(Some(7), url).unwrap(), 7);
        assert_eq!(pr_number(None, url).unwrap(), 42);
        let err = pr_number(None, "https://github.com/acme/app").unwrap_err();
        assert!(
            err.to_string()
                .contains("could not parse pull request number")
        );
    }

    #[test]
    fn transfer_requires_exactly_one_direction() {
        let parse = |args: &[&str]| {
            let argv = ["git-uplink", "transfer", "upl_x"].iter().chain(args);
            Cli::try_parse_from(argv).is_ok()
        };
        assert!(!parse(&[]));
        assert!(!parse(&["--complete"]));
        assert!(!parse(&["--to-upstream", "--to-internal"]));
        assert!(parse(&["--to-upstream"]));
        assert!(parse(&["--to-internal", "--complete"]));
    }

    #[test]
    fn amend_message_flags_need_complete_and_title() {
        let parse = |args: &[&str]| {
            let argv = ["git-uplink", "amend", "upl_x"].iter().chain(args);
            Cli::try_parse_from(argv).is_ok()
        };
        assert!(parse(&[]));
        assert!(parse(&["--complete"]));
        assert!(parse(&[
            "--complete",
            "--title",
            "T",
            "--message-file",
            "m.txt"
        ]));
        assert!(!parse(&["--title", "T"]));
        assert!(!parse(&["--complete", "--message-file", "m.txt"]));
        assert!(!parse(&[
            "--complete",
            "--title",
            "T",
            "--message",
            "m",
            "--message-file",
            "f"
        ]));
    }

    #[test]
    fn assess_patch_accepts_a_message_override() {
        let parse = |args: &[&str]| {
            let argv = ["git-uplink", "assess", "--patch", "upl_x"]
                .iter()
                .chain(args);
            Cli::try_parse_from(argv).is_ok()
        };
        assert!(parse(&["--title", "T", "--message-file", "m.txt"]));
        assert!(!parse(&["--internal-only"]));
    }

    #[test]
    fn merged_via_is_parsed_by_clap() {
        assert_eq!(merged_via(&["--via", "patch-id"]), Some(MergeVia::PatchId));
        assert_eq!(
            merged_via(&["--via", "empty-rebase"]),
            Some(MergeVia::EmptyRebase)
        );
        assert_eq!(merged_via(&[]), Some(MergeVia::Manual));
        assert_eq!(merged_via(&["--via", "bogus"]), None);
    }
}
