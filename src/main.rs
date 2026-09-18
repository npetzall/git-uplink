use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use git_uplink::{
    AddPatchOpts, ApprovalReceipt, Error, IncomingPreflight, InitOpts, MergeVia, OSS_ENVIRONMENT,
    PreflightError, STATE_BRANCH, add_patch, approve_patch_at, commit_queue, drop_patch,
    format_approval_receipt, format_contribution_packet, format_prepare_markdown, git_ok, init,
    mark_merged, parse_github_repo, parse_issue_url, parse_pull_request_url,
    preflight_existing_patch, preflight_incoming_change, prepare_from_message, read_queue, rebuild,
    record_conflict_issue, record_pull_request, report_paths, resolve_conflict, status_snapshot,
    submit_patch, summarize_queue, sync,
};
use git_uplink::{Patch, QueueState};

#[derive(Parser)]
#[command(
    name = "git-uplink",
    bin_name = "git uplink",
    about = "Carry internal patches on upstream, contribute once, drop when merged.",
    long_about = "Developers open PRs and merge them; they never push main.\n\
add records a merged PR as a queued patch on uplink/state. prepare uses the PR\n\
title and body as the single commit message, rewrites the export author, strips\n\
the internal section before contrib export, and scans for company affiliation.\n\
On GitHub Enterprise Cloud, contribution approval is the oss Environment;\n\
approve/submit run after that review. git uplink talks to git only; workflows\n\
use gh for GitHub and follow-up commands (submitted, conflicted) to record results."
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
    },
    Add {
        #[arg(long)]
        title: String,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        head: Option<String>,
        #[arg(long)]
        internal_only: bool,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long)]
        pr_url: Option<String>,
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        #[arg(long)]
        push: bool,
        #[arg(long)]
        refresh: Option<String>,
        #[arg(long = "push-remote")]
        push_remote: Option<String>,
    },
    Preflight {
        id: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        head: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
    },
    Prepare {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
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
    },
    Status,
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
    Conflicted {
        id: String,
        #[arg(long = "issue-url")]
        issue_url: String,
        #[arg(long)]
        issue: Option<u64>,
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
    Rebuild,
    Resolve {
        id: String,
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
        Error::Prepare(pre) => print!("{}", format_prepare_markdown(&pre.report)),
        _ => {}
    }
}

fn conflict_body(
    id: &str,
    branch: &str,
    onto: Option<&str>,
    resolved_from: Option<&str>,
) -> String {
    let intro = if let Some(from) = resolved_from {
        format!(
            "Rebuild after resolving `{from}` stopped on `{id}`. Checkout `{branch}`, remove the conflict markers, and push. Then run `git uplink resolve {id}` (or let **Uplink resolve** run on that push)."
        )
    } else {
        format!(
            "Sync stopped on `{id}`. Checkout `{branch}`, remove the conflict markers, and push. Then run `git uplink resolve {id}` (or let **Uplink resolve** run on that push)."
        )
    };
    let mut body = format!(
        "Company `main` is bot-owned. **Do not open or merge a pull request** for this conflict.\n\n\
{intro}\n\n\
Remaining patches wait until this id is resolved.\n"
    );
    if let Some(compare) = compare_url(onto, branch) {
        body.push_str(&format!(
            "\nCompare the failed apply (not frozen main): {compare}\n"
        ));
    }
    body
}

fn issue_create_artifact(
    repo: &Path,
    patch: &Patch,
    resolved_from: Option<&str>,
) -> serde_json::Value {
    let conflict = patch.conflict.as_ref();
    let branch = conflict.map(|c| c.branch.as_str()).unwrap_or("");
    let onto = conflict.and_then(|c| c.onto.as_deref());
    let body = conflict_body(&patch.id, branch, onto, resolved_from);
    let body_file = format!(".uplink/reports/{}/conflict.md", patch.id);
    write_markdown_file(repo, Path::new(&body_file), &body);
    serde_json::json!({
        "title": format!("Uplink conflict: {}", patch.id),
        "bodyFile": body_file,
        "label": "uplink:conflict"
    })
}

fn issue_close_artifact(patch: &Patch) -> Option<serde_json::Value> {
    let conflict = patch.conflict.as_ref()?;
    let url = conflict.issue_url.as_ref()?;
    Some(serde_json::json!({
        "number": conflict.issue_number,
        "url": url,
        "comment": format!(
            "Resolved {}; company main rebuilt. Do not open a conflict PR.",
            patch.id
        )
    }))
}

fn print_sync_artifact(repo: &Path, queue: &QueueState) {
    let conflict = queue.patches.iter().find(|p| p.status == "conflict");
    let mut value = serde_json::json!({
        "lastSync": queue.last_sync,
    });
    if let Some(patch) = conflict {
        value["conflict"] = serde_json::json!({
            "id": patch.id,
            "branch": patch.conflict.as_ref().map(|c| &c.branch),
            "onto": patch.conflict.as_ref().and_then(|c| c.onto.clone()),
        });
        value["gh"] = serde_json::json!({
            "issueCreate": issue_create_artifact(repo, patch, None),
        });
    }
    println!("{value}");
}

fn print_resolve_artifact(
    repo: &Path,
    resolved_id: &str,
    prior: &Patch,
    queue: &QueueState,
    follow_on_conflict: bool,
) {
    let mut gh = serde_json::Map::new();
    if let Some(close) = issue_close_artifact(prior) {
        gh.insert("issueClose".into(), close);
    }
    let conflict = queue.patches.iter().find(|p| p.status == "conflict");
    if follow_on_conflict {
        if let Some(patch) = conflict {
            gh.insert(
                "issueCreate".into(),
                issue_create_artifact(repo, patch, Some(resolved_id)),
            );
        }
    }
    let status = queue
        .patches
        .iter()
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
    let body_file = format!(".uplink/reports/{}/pr.md", patch.id);
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
        } => {
            let queue = init(
                &repo,
                InitOpts {
                    upstream_url: upstream,
                    contrib_url: contrib,
                    upstream_remote_name,
                    upstream_branch,
                    contrib_remote_name,
                    internal_branch,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&queue)?);
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
            push,
            refresh,
            push_remote,
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
                refresh_remote: refresh.or_else(|| push.then(|| "origin".into())),
                push_remote: if push {
                    Some(push_remote.unwrap_or_else(|| "origin".into()))
                } else {
                    None
                },
                ..Default::default()
            };
            match add_patch(&repo, opts) {
                Ok(patch) => {
                    println!(
                        "{}  {}  {}  {}",
                        patch.id, patch.intent, patch.status, patch.title
                    );
                }
                Err(err) => {
                    print_failure_comment(&err);
                    return Err(err);
                }
            }
        }
        Commands::Prepare {
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
            let report = prepare_from_message(
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
            let markdown = format_prepare_markdown(&report);
            println!("{markdown}");
            append_step_summary(&markdown);
            if !report.ok {
                return Err(Error::msg("prepare failed"));
            }
        }
        Commands::Report { id, out } => {
            let queue = read_queue(&repo)?;
            let patch = queue
                .patches
                .iter()
                .find(|p| p.id == id)
                .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
            let packet = format_contribution_packet(&repo, patch)?;
            let default_out = report_paths(&id).1;
            let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
            write_markdown_file(&repo, &dest, &packet);
            append_step_summary(&packet);
            commit_queue(&repo, &format!("uplink: OSS packet {id}"))?;
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
        Commands::Status => {
            let snapshot = status_snapshot(&repo)?;
            println!(
                "{}",
                serde_json::to_string(&summarize_queue(&snapshot.queue))?
            );
            for patch in &snapshot.queue.patches {
                let link = patch
                    .upstream
                    .as_ref()
                    .and_then(|u| u.pr_url.clone())
                    .unwrap_or_else(|| patch.intent.clone());
                println!(
                    "{}  {:<10}  {:<14}  {}  {link}",
                    patch.id, patch.status, patch.intent, patch.title
                );
            }
            if let Some(sync) = &snapshot.queue.last_sync {
                println!("last sync: {} @ {}", sync.result, sync.at);
                if let Some(msg) = &sync.message {
                    println!("{msg}");
                }
            }
        }
        Commands::Approve { id, out } => {
            let queue = read_queue(&repo)?;
            if !queue.patches.iter().any(|p| p.id == id) {
                return Err(Error::msg(format!("unknown patch {id}")));
            }
            let sha = git_ok(&repo, &["rev-parse", STATE_BRANCH]).unwrap_or_else(|_| {
                env::var("GITHUB_SHA")
                    .unwrap_or_else(|_| git_ok(&repo, &["rev-parse", "HEAD"]).unwrap_or_default())
            });
            let run_url = github_run_url();
            let receipt = format_approval_receipt(ApprovalReceipt {
                patch_id: &id,
                environment: env::var("UPLINK_OSS_ENVIRONMENT")
                    .ok()
                    .as_deref()
                    .unwrap_or(OSS_ENVIRONMENT),
                actor: env::var("GITHUB_ACTOR")
                    .ok()
                    .as_deref()
                    .unwrap_or("local operator"),
                run_url: &run_url,
                sha: &sha,
                at: None,
            });
            let default_out = report_paths(&id).2;
            let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
            write_markdown_file(&repo, &dest, &receipt);
            append_step_summary(&receipt);
            let patch = approve_patch_at(&repo, &id, Some(&sha), Some(&run_url))?;
            commit_queue(&repo, &format!("uplink: OSS approval receipt {id}"))?;
            println!("{} approved", patch.id);
            eprintln!("Wrote {}", dest.display());
        }
        Commands::Submit { id } => {
            let queue = read_queue(&repo)?;
            let patch = queue
                .patches
                .iter()
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
            let queue = sync(&repo)?;
            print_sync_artifact(&repo, &queue);
            if queue.patches.iter().any(|p| p.status == "conflict") {
                if let Some(conflict) = queue.patches.iter().find(|p| p.status == "conflict") {
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
                return Err(Error::msg("sync conflict"));
            }
        }
        Commands::Conflicted {
            id,
            issue_url,
            issue,
            push_remote,
        } => {
            let number = issue
                .or_else(|| parse_issue_url(&issue_url))
                .ok_or_else(|| {
                    Error::msg(format!("could not parse issue number from {issue_url}"))
                })?;
            let patch =
                record_conflict_issue(&repo, &id, number, &issue_url, Some(push_remote.as_str()))?;
            println!("{} conflict issue {issue_url}", patch.id);
        }
        Commands::Merged { id, via, sha } => {
            let via = MergeVia::parse(&via).ok_or_else(|| Error::msg("invalid --via"))?;
            mark_merged(&repo, &id, via.clone(), sha.as_deref())?;
            rebuild(&repo)?;
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
        Commands::Rebuild => {
            rebuild(&repo)?;
            println!("rebuild complete");
        }
        Commands::Resolve { id } => {
            let prior = read_queue(&repo)?;
            let prior_patch = prior
                .patches
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
            match resolve_conflict(&repo, &id) {
                Ok(queue) => {
                    print_resolve_artifact(&repo, &id, &prior_patch, &queue, false);
                }
                Err(Error::Conflict(err)) => {
                    let queue = read_queue(&repo)?;
                    print_resolve_artifact(&repo, &id, &prior_patch, &queue, true);
                    if let Some(conflict) = queue.patches.iter().find(|p| p.status == "conflict") {
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
                    return Err(Error::Conflict(err));
                }
                Err(err) => return Err(err),
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
            // 1 = this command did not complete. 2 = this command persisted a
            // follow-on apply conflict (sync, or resolve after a successful amend).
            let code = match &err {
                Error::Conflict(_) => 2,
                Error::Message(m) if m == "prepare failed" || m == "sync conflict" => 2,
                _ => 1,
            };
            if code == 1 || matches!(err, Error::Conflict(_)) {
                eprintln!("{err}");
            }
            ExitCode::from(code)
        }
    }
}
