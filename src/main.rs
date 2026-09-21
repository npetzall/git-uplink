use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use git_uplink::{
    AddPatchOpts, ApprovalReceipt, Error, FROM_UPSTREAM_ENVIRONMENT, Forge, IncomingPreflight,
    InitOpts, MergeVia, PreflightError, PushOpts, RebuildOpts, STATE_BRANCH,
    TO_UPSTREAM_ENVIRONMENT, accept_upstream, add_patch, adopted_next_steps, approve_patch_at,
    assess_from_message, commit_queue, drop_patch, format_approval_receipt, format_assess_markdown,
    format_contribution_packet_with_extras, format_status_table, from_upstream_report_paths,
    git_ok, init, load_groups_file, mark_merged, parse_github_repo, parse_pull_request_url,
    preflight_existing_patch, preflight_incoming_change, push_queue, read_queue, rebuild_with,
    record_gated_pr, record_pull_request, refresh_from_origin, report_paths, reset_from_origin,
    resolve_conflict, status_report, status_snapshot, submit_patch, sync, transfer_patch,
};
use git_uplink::{Patch, QueueState, SyncResult, TransferDirection, TransferResult};

#[derive(Parser)]
#[command(
    name = "git-uplink",
    bin_name = "git uplink",
    about = "Carry internal patches on upstream, contribute once, drop when merged.",
    long_about = "Developers open PRs and merge them; they never push main.\n\
add records a merged PR as a queued patch on uplink/state. assess uses the PR\n\
title and body as the single commit message, rewrites the export author, strips\n\
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
    },
    Report {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(
            long = "extra-dir",
            help = "Directory of *.md files prepended to the packet"
        )]
        extra_dir: Option<PathBuf>,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Approve {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Submit {
        id: String,
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
        #[arg(long, default_value = "manual")]
        via: String,
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
            help = "Rebuild onto this branch instead of company main (preview; does not mutate the queue)"
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
    Transfer {
        id: String,
        #[arg(long = "to-upstream", conflicts_with = "to_internal")]
        to_upstream: bool,
        #[arg(long = "to-internal", conflicts_with = "to_upstream")]
        to_internal: bool,
        #[arg(long, help = "Finish a gated transfer after the work PR is merged")]
        complete: bool,
    },
    /// Start the embedded operator dashboard and open a browser.
    #[command(name = "web-ui")]
    WebUi {
        #[arg(long, default_value_t = 43721)]
        port: u16,
        /// Bind address. Use 0.0.0.0 to reach the UI from another host.
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Do not launch a browser.
        #[arg(long)]
        no_open: bool,
    },
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

fn append_step_summary(markdown: &str) {
    let Ok(path) = env::var("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let mut body = markdown.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body.push('\n');
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(body.as_bytes()));
}

fn write_markdown_file(repo: &Path, file: &Path, markdown: &str) -> PathBuf {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        repo.join(file)
    };
    if let Some(parent) = abs.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut body = markdown.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    let _ = fs::write(&abs, body);
    file.to_path_buf()
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
) -> serde_json::Value {
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
        Err(_) => return serde_json::json!({"error": "invalid patch id"}),
    };
    write_markdown_file(repo, Path::new(&body_file), &body);
    serde_json::json!({
        "title": format!("Uplink conflict: {}", patch.id),
        "bodyFile": body_file,
        "head": work,
        "base": base,
        "labels": ["uplink:conflict"]
    })
}

fn print_sync_artifact(repo: &Path, result: &SyncResult) {
    let queue = &result.queue;
    let conflict = queue.all_patches().find(|p| p.status == "conflict");
    let mut value = serde_json::json!({
        "lastSync": queue.last_sync,
        "needsApproval": result.needs_approval,
        "pendingSha": result.pending_sha,
        "flowedBack": result.flowed_back,
        "foreignCommits": result.foreign_commits,
        "reportPath": result.report_path,
    });
    if let Some(patch) = conflict {
        value["conflict"] = serde_json::json!({
            "id": patch.id,
            "branch": patch.conflict.as_ref().map(|c| &c.branch),
            "workBranch": patch.conflict.as_ref().and_then(|c| c.work_branch.clone()),
            "onto": patch.conflict.as_ref().and_then(|c| c.onto.clone()),
        });
        value["gh"] = serde_json::json!({
            "prCreate": conflict_pr_create_artifact(repo, patch, None),
        });
    }
    println!("{value}");
}

fn print_transfer_artifact(repo: &Path, result: &TransferResult) {
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
        write_markdown_file(repo, Path::new(&body_file), &body);
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
}

fn finish_sync(repo: &Path, result: SyncResult) -> Result<(), Error> {
    if let Some(report) = &result.report {
        append_step_summary(report);
    }
    print_sync_artifact(repo, &result);
    if let Some(conflict) = result.queue.all_patches().find(|p| p.status == "conflict") {
        eprintln!(
            "CONFLICT {} on {}",
            conflict.id,
            conflict
                .conflict
                .as_ref()
                .map(|c| c.branch.as_str())
                .unwrap_or("")
        );
    }
    Ok(())
}

fn print_resolve_artifact(
    repo: &Path,
    resolved_id: &str,
    queue: &QueueState,
    follow_on_conflict: bool,
) {
    let mut gh = serde_json::Map::new();
    let conflict = queue.all_patches().find(|p| p.status == "conflict");
    if follow_on_conflict && let Some(patch) = conflict {
        gh.insert(
            "prCreate".into(),
            conflict_pr_create_artifact(repo, patch, Some(resolved_id)),
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
        value["conflict"] = serde_json::json!({
            "id": patch.id,
            "branch": patch.conflict.as_ref().map(|c| &c.branch),
            "workBranch": patch.conflict.as_ref().and_then(|c| c.work_branch.clone()),
            "onto": patch.conflict.as_ref().and_then(|c| c.onto.clone()),
        });
    }
    if !gh.is_empty() {
        value["gh"] = serde_json::Value::Object(gh);
    }
    println!("{value}");
}

fn submit_artifact(repo: &Path, queue: &QueueState, patch: &Patch, branch: &str, sha: &str) {
    let body = format!(
        "Company contribution exported by Uplink.\n\nUplink-Patch-Id: {}\n",
        patch.id
    );
    let body_file = match report_paths(&patch.id) {
        Ok((dir, _, _)) => format!("{dir}/pr.md"),
        Err(_) => return,
    };
    write_markdown_file(repo, Path::new(&body_file), &body);
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
        "sha": sha,
        "existingPr": existing,
    });
    if !gh.is_empty() {
        value["gh"] = serde_json::Value::Object(gh);
    }
    println!("{value}");
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
        } => {
            let adopt_groups = match adopt_groups {
                Some(path) => Some(load_groups_file(&path)?),
                None => None,
            };
            let opts = InitOpts {
                upstream_url: upstream,
                contrib_url: contrib,
                upstream_remote_name,
                upstream_branch,
                contrib_remote_name,
                internal_branch,
                forge,
                upgrade,
                adopt_groups,
                interactive: None,
            };
            let hydrate = !opts.has_args() && opts.forge.is_none() && !opts.upgrade;
            let queue = init(&repo, opts)?;
            if queue.all_patches().any(|p| {
                p.source
                    .note
                    .as_deref()
                    .is_some_and(|n| n.starts_with("adopted from "))
            }) {
                eprintln!("{}", adopted_next_steps());
            }
            if !hydrate {
                println!("{}", serde_json::to_string_pretty(&queue)?);
            }
        }
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
        } => {
            let message = read_commit_message(message, message_file, &title)?;
            let opts = AddPatchOpts {
                title,
                message: Some(message),
                internal_only,
                from_ref: from,
                head_ref: head,
                depends_on,
                author: env::var("GIT_AUTHOR_NAME").ok(),
                internal_pr_number: pr,
                internal_pr_url: pr_url,
                ..Default::default()
            };
            match add_patch(&repo, opts) {
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
                }
                Err(err) => {
                    print_failure_comment(&err);
                    return Err(err);
                }
            }
        }
        Commands::Push { push_remote } => {
            let result = push_queue(
                &repo,
                PushOpts {
                    push_remote: Some(push_remote.unwrap_or_else(|| "origin".into())),
                },
            )?;
            println!(
                "{} {} to {} at {}",
                result.action, result.branch, result.remote, result.sha
            );
        }
        Commands::Refresh => {
            let result = refresh_from_origin(&repo)?;
            println!("origin/{} {}", result.internal_branch, result.internal_sha);
            println!("origin/{} {}", STATE_BRANCH, result.state_sha);
            println!("origin/uplink/upstream {}", result.upstream_sha);
        }
        Commands::Reset => {
            let result = reset_from_origin(&repo)?;
            println!("{} {}", result.internal_branch, result.internal_sha);
            println!("{} {}", STATE_BRANCH, result.state_sha);
            println!("uplink/upstream {}", result.upstream_sha);
        }
        Commands::Assess {
            from,
            head,
            title,
            message,
            message_file,
            internal_only,
        } => {
            let queue = read_queue(&repo)?;
            let title = title.unwrap_or_else(|| "candidate change".into());
            let message = read_commit_message(message, message_file, &title)?;
            let report = assess_from_message(
                &repo,
                &queue,
                from.as_deref().unwrap_or("main"),
                head.as_deref().unwrap_or("HEAD"),
                &message,
                Some(&title),
                if internal_only {
                    "internal-only"
                } else {
                    "upstream"
                },
            )?;
            let markdown = format_assess_markdown(&report);
            println!("{markdown}");
            append_step_summary(&markdown);
            if !report.ok {
                return Err(Error::msg("assess failed"));
            }
        }
        Commands::Report { id, out, extra_dir } => {
            let queue = read_queue(&repo)?;
            let patch = queue
                .all_patches()
                .find(|p| p.id == id)
                .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
            let packet =
                format_contribution_packet_with_extras(&repo, patch, extra_dir.as_deref())?;
            let default_out = report_paths(&id)?.1;
            let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
            write_markdown_file(&repo, &dest, &packet);
            append_step_summary(&packet);
            commit_queue(&repo, &format!("uplink: contribution packet {id}"))?;
            println!("{packet}");
            eprintln!("Wrote {}", dest.display());
        }
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
            let result = if let Some(id) = id {
                let queue = read_queue(&repo)?;
                preflight_existing_patch(&repo, &queue, &id)
            } else {
                preflight_incoming_change(
                    &repo,
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
            };
            match result {
                Ok(()) => println!("export preflight passed"),
                Err(err) => {
                    print_failure_comment(&err);
                    return Err(err);
                }
            }
        }
        Commands::Status { json } => {
            let snapshot = status_snapshot(&repo)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status_report(&snapshot))?
                );
            } else {
                print!("{}", format_status_table(&snapshot));
            }
        }
        Commands::Approve { id, out } => {
            let queue = read_queue(&repo)?;
            if !queue.all_patches().any(|p| p.id == id) {
                return Err(Error::msg(format!("unknown patch {id}")));
            }
            let sha = git_ok(&repo, &["rev-parse", STATE_BRANCH]).unwrap_or_else(|_| {
                env::var("GITHUB_SHA")
                    .unwrap_or_else(|_| git_ok(&repo, &["rev-parse", "HEAD"]).unwrap_or_default())
            });
            let run_url = github_run_url();
            let receipt = format_approval_receipt(ApprovalReceipt {
                patch_id: &id,
                environment: env::var("UPLINK_TO_UPSTREAM_ENVIRONMENT")
                    .ok()
                    .as_deref()
                    .unwrap_or(TO_UPSTREAM_ENVIRONMENT),
                actor: env::var("GITHUB_ACTOR")
                    .ok()
                    .as_deref()
                    .unwrap_or("local operator"),
                run_url: &run_url,
                sha: &sha,
                at: None,
            });
            let default_out = report_paths(&id)?.2;
            let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
            write_markdown_file(&repo, &dest, &receipt);
            append_step_summary(&receipt);
            let patch = approve_patch_at(&repo, &id, Some(&sha), Some(&run_url))?;
            commit_queue(&repo, &format!("uplink: to-upstream approval receipt {id}"))?;
            println!("{} approved", patch.id);
            eprintln!("Wrote {}", dest.display());
        }
        Commands::Submit { id } => {
            let queue = read_queue(&repo)?;
            let patch = queue
                .all_patches()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
            let exported = match submit_patch(&repo, &id) {
                Ok(v) => v,
                Err(err) => {
                    print_failure_comment(&err);
                    return Err(err);
                }
            };
            submit_artifact(&repo, &queue, &patch, &exported.branch, &exported.sha);
        }
        Commands::Submitted {
            id,
            pr_url,
            pr,
            push_remote,
        } => {
            let number = pr
                .or_else(|| parse_pull_request_url(&pr_url))
                .ok_or_else(|| {
                    Error::msg(format!("could not parse pull request number from {pr_url}"))
                })?;
            let branch = format!("uplink/{id}");
            let patch = record_pull_request(
                &repo,
                &id,
                number,
                &pr_url,
                &branch,
                Some(push_remote.as_str()),
            )?;
            println!("{} submitted as {pr_url}", patch.id);
        }
        Commands::Sync => {
            finish_sync(&repo, sync(&repo)?)?;
        }
        Commands::AcceptUpstream => {
            let queue = read_queue(&repo)?;
            if queue.pending_upstream.is_none() {
                return Err(Error::msg(
                    "No pending upstream to accept. Run `git uplink sync` first.",
                ));
            }
            let sha = git_ok(&repo, &["rev-parse", STATE_BRANCH]).unwrap_or_else(|_| {
                env::var("GITHUB_SHA")
                    .unwrap_or_else(|_| git_ok(&repo, &["rev-parse", "HEAD"]).unwrap_or_default())
            });
            let run_url = github_run_url();
            let receipt = format_approval_receipt(ApprovalReceipt {
                patch_id: "incoming",
                environment: env::var("UPLINK_FROM_UPSTREAM_ENVIRONMENT")
                    .ok()
                    .as_deref()
                    .unwrap_or(FROM_UPSTREAM_ENVIRONMENT),
                actor: env::var("GITHUB_ACTOR")
                    .ok()
                    .as_deref()
                    .unwrap_or("local operator"),
                run_url: &run_url,
                sha: &sha,
                at: None,
            });
            let dest = PathBuf::from(from_upstream_report_paths().2);
            write_markdown_file(&repo, &dest, &receipt);
            append_step_summary(&receipt);
            finish_sync(&repo, accept_upstream(&repo)?)?;
        }
        Commands::Gated {
            id,
            pr_url,
            pr,
            push_remote,
        } => {
            let number = pr
                .or_else(|| parse_pull_request_url(&pr_url))
                .ok_or_else(|| {
                    Error::msg(format!("could not parse pull request number from {pr_url}"))
                })?;
            let patch = record_gated_pr(&repo, &id, number, &pr_url, Some(push_remote.as_str()))?;
            println!("{} conflict PR {pr_url}", patch.id);
        }
        Commands::Merged { id, via, sha } => {
            let via = MergeVia::parse(&via).ok_or_else(|| Error::msg("invalid --via"))?;
            mark_merged(&repo, &id, via.clone(), sha.as_deref())?;
            rebuild_with(&repo, RebuildOpts::default())?;
            println!("{id} marked merged via {}", via.as_str());
        }
        Commands::Drop { id, reason } => {
            drop_patch(
                &repo,
                &id,
                reason.as_deref().unwrap_or("dropped by operator"),
            )?;
            println!("{id} dropped");
        }
        Commands::Rebuild {
            branch,
            push,
            push_remote,
        } => {
            let result = rebuild_with(
                &repo,
                RebuildOpts {
                    branch: branch.clone(),
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
        }
        Commands::Resolve { id } => match resolve_conflict(&repo, &id) {
            Ok(queue) => {
                print_resolve_artifact(&repo, &id, &queue, false);
            }
            Err(Error::Conflict(_)) => {
                let queue = read_queue(&repo)?;
                print_resolve_artifact(&repo, &id, &queue, true);
                if let Some(conflict) = queue.all_patches().find(|p| p.status == "conflict") {
                    eprintln!(
                        "CONFLICT {} on {}",
                        conflict.id,
                        conflict
                            .conflict
                            .as_ref()
                            .map(|c| c.branch.as_str())
                            .unwrap_or("")
                    );
                }
            }
            Err(err) => return Err(err),
        },
        Commands::Transfer {
            id,
            to_upstream,
            to_internal,
            complete,
        } => {
            let direction = if to_upstream {
                TransferDirection::ToUpstream
            } else if to_internal {
                TransferDirection::ToInternal
            } else {
                return Err(Error::msg("specify --to-upstream or --to-internal"));
            };
            let result = transfer_patch(&repo, &id, direction, complete)?;
            print_transfer_artifact(&repo, &result);
            if result.gated {
                eprintln!(
                    "TRANSFER {} needs work on {}",
                    result.id,
                    result.work_branch.as_deref().unwrap_or("")
                );
            }
        }
        Commands::WebUi {
            port,
            bind,
            no_open,
        } => {
            let addr: std::net::SocketAddr = format!("{bind}:{port}")
                .parse()
                .map_err(|err| Error::msg(format!("invalid --bind/--port: {err}")))?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|err| Error::msg(format!("tokio runtime: {err}")))?;
            runtime
                .block_on(git_uplink::webui::serve(repo, addr, !no_open))
                .map_err(|err| Error::msg(format!("web-ui server: {err}")))?;
        }
    }
    Ok(())
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
