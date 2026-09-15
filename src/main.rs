use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use git_uplink::{
    AddPatchOpts, ApprovalReceipt, Error, GitOpts, GithubConfig, IncomingPreflight, MergeVia,
    OSS_ENVIRONMENT, PreflightError, QueueConfig, add_patch, approve_patch, comment_on_issue,
    commit_queue, create_upstream_pull_request, drop_patch, format_approval_receipt,
    format_approver_packet, format_prepare_markdown, get_pull_request, git, git_ok, init_repo,
    mark_merged, parse_github_repo, preflight_existing_patch, preflight_incoming_change,
    prepare_from_range, read_queue, rebuild, record_pull_request, report_paths, resolve_conflict,
    status_snapshot, submit_patch, summarize_queue, sync,
};

#[derive(Parser)]
#[command(
    name = "git-uplink",
    bin_name = "git uplink",
    about = "Carry internal patches on upstream, contribute once, drop when merged.",
    long_about = "Company main is bot-owned. Developers open PRs; they never push main.\n\
add is the internal product gate (status: queued). prepare rewrites the\n\
export author, strips the internal commit-message section, and scans for\n\
company affiliation. On GitHub Enterprise Cloud, contribution approval is\n\
the oss Environment; approve/submit run after that review."
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
    },
    Add {
        #[arg(long)]
        title: String,
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
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        #[arg(long)]
        pr: Option<u64>,
    },
    Prepare {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        head: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
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
    Sync,
    Merged {
        id: String,
        #[arg(long, default_value = "manual")]
        via: String,
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

fn depends_from_env() -> Vec<String> {
    env::var("UPLINK_DEPENDS_ON")
        .unwrap_or_default()
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|id| id.starts_with("upl_"))
        .map(str::to_string)
        .collect()
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

fn notify_internal_pr(pr_number: Option<u64>, body: &str) {
    let (Some(pr_number), Ok(token), Ok(repository)) = (
        pr_number,
        env::var("GITHUB_TOKEN"),
        env::var("GITHUB_REPOSITORY"),
    ) else {
        return;
    };
    let mut parts = repository.split('/');
    let (Some(owner), Some(name)) = (parts.next(), parts.next()) else {
        return;
    };
    if let Err(err) = comment_on_issue(
        &token,
        env::var("GITHUB_API_URL").ok().as_deref(),
        owner,
        name,
        pr_number,
        body,
    ) {
        eprintln!("Could not comment on internal PR #{pr_number}: {err}");
    }
}

fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    let repo = env::current_dir()?;
    match cli.command {
        Commands::Init { upstream, contrib } => {
            let mut config = QueueConfig::default();
            if upstream.is_some() {
                config.upstream_remote = "upstream".into();
            }
            let queue = init_repo(&repo, config)?;
            if let Some(url) = upstream {
                let _ = git(
                    &repo,
                    &["remote", "remove", "upstream"],
                    GitOpts {
                        allow_fail: true,
                        ..GitOpts::default()
                    },
                );
                git(
                    &repo,
                    &["remote", "add", "upstream", &url],
                    GitOpts::default(),
                )?;
            }
            if let Some(url) = contrib {
                let _ = git(
                    &repo,
                    &["remote", "remove", "contrib"],
                    GitOpts {
                        allow_fail: true,
                        ..GitOpts::default()
                    },
                );
                git(
                    &repo,
                    &["remote", "add", "contrib", &url],
                    GitOpts::default(),
                )?;
            }
            println!("{}", serde_json::to_string_pretty(&queue)?);
        }
        Commands::Add {
            title,
            from,
            head,
            internal_only,
            pr,
            pr_url,
            mut depends_on,
            push,
            refresh,
            push_remote,
        } => {
            depends_on.extend(depends_from_env());
            let opts = AddPatchOpts {
                title,
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
                    if let Error::Preflight(pre) = &err {
                        notify_internal_pr(pr, &preflight_comment(pre));
                    }
                    if let Error::Prepare(pre) = &err {
                        notify_internal_pr(pr, &format_prepare_markdown(&pre.report));
                    }
                    return Err(err);
                }
            }
        }
        Commands::Prepare {
            from,
            head,
            title,
            pr,
            internal_only,
        } => {
            let queue = read_queue(&repo)?;
            let report = prepare_from_range(
                &repo,
                &queue,
                from.as_deref().unwrap_or("main"),
                head.as_deref().unwrap_or("HEAD"),
                Some(title.as_deref().unwrap_or("candidate change")),
                if internal_only {
                    "internal-only"
                } else {
                    "upstream"
                },
            )?;
            let markdown = format_prepare_markdown(&report);
            println!("{markdown}");
            append_step_summary(&markdown);
            notify_internal_pr(pr, &markdown);
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
            let packet = format_approver_packet(patch);
            let default_out = report_paths(&id).1;
            let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
            write_markdown_file(&repo, &dest, &packet);
            append_step_summary(&packet);
            println!("{packet}");
            eprintln!("Wrote {}", dest.display());
        }
        Commands::Preflight {
            id,
            from,
            head,
            title,
            mut depends_on,
            pr,
        } => {
            depends_on.extend(depends_from_env());
            let result = if let Some(id) = id {
                let queue = read_queue(&repo)?;
                preflight_existing_patch(&repo, &queue, &id)
            } else {
                preflight_incoming_change(
                    &repo,
                    IncomingPreflight {
                        title: title.unwrap_or_else(|| "candidate change".into()),
                        from_ref: from.unwrap_or_else(|| "main".into()),
                        head_ref: head.unwrap_or_else(|| "HEAD".into()),
                        depends_on,
                        preflight_command: None,
                    },
                )
            };
            match result {
                Ok(()) => println!("export preflight passed"),
                Err(err) => {
                    if let Error::Preflight(pre) = &err {
                        notify_internal_pr(pr, &preflight_comment(pre));
                    }
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
            let sha = env::var("GITHUB_SHA")
                .unwrap_or_else(|_| git_ok(&repo, &["rev-parse", "HEAD"]).unwrap_or_default());
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
                run_url: &github_run_url(),
                sha: &sha,
                at: None,
            });
            let default_out = report_paths(&id).2;
            let dest = out.unwrap_or_else(|| PathBuf::from(&default_out));
            write_markdown_file(&repo, &dest, &receipt);
            append_step_summary(&receipt);
            let patch = approve_patch(&repo, &id)?;
            commit_queue(&repo, &format!("uplink: OSS approval receipt {id}"))?;
            println!("{} approved", patch.id);
            eprintln!("Wrote {}", dest.display());
        }
        Commands::Submit { id } => {
            let token = env::var("UPLINK_GITHUB_TOKEN").ok();
            let queue = read_queue(&repo)?;
            let patch = queue
                .patches
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
            let exported = match submit_patch(&repo, &id, None) {
                Ok(v) => v,
                Err(err) => {
                    if let Error::Preflight(pre) = &err {
                        notify_internal_pr(
                            patch.source.internal_pr_number,
                            &preflight_comment(pre),
                        );
                    }
                    return Err(err);
                }
            };
            if let Some(token) = token {
                let upstream_url = remote_url(&repo, &queue.config.upstream_remote);
                let contrib_url = remote_url(&repo, &queue.config.contrib_remote);
                if let (Some((uo, ur)), Some((co, cr))) = (
                    upstream_url.as_deref().and_then(parse_github_repo),
                    contrib_url.as_deref().and_then(parse_github_repo),
                ) {
                    let pr = create_upstream_pull_request(
                        &GithubConfig {
                            token,
                            api_url: env::var("UPLINK_GITHUB_API").ok(),
                            upstream_owner: uo,
                            upstream_repo: ur,
                            contrib_owner: co,
                            contrib_repo: cr,
                        },
                        &patch.title,
                        &format!(
                            "Company contribution exported by Uplink.\n\nUplink-Patch-Id: {}\nUplink-Intent: upstream\n",
                            patch.id
                        ),
                        &exported.branch,
                        &queue.config.upstream_branch,
                    )?;
                    record_pull_request(&repo, &id, pr.number, &pr.url, &exported.branch)?;
                    println!("{}", pr.url);
                    return Ok(());
                }
            }
            println!("exported {} at {}", exported.branch, exported.sha);
            println!("Set UPLINK_GITHUB_TOKEN to open the upstream pull request automatically.");
        }
        Commands::Sync => {
            let queue = sync(&repo)?;
            println!("{}", serde_json::to_string(&queue.last_sync)?);
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
                return Err(Error::msg("sync conflict"));
            }
        }
        Commands::Merged { id, via } => {
            let via = MergeVia::parse(&via).ok_or_else(|| Error::msg("invalid --via"))?;
            let token = env::var("UPLINK_GITHUB_TOKEN").ok();
            let queue = read_queue(&repo)?;
            let patch = queue.patches.iter().find(|p| p.id == id);
            if matches!(via, MergeVia::Pr) {
                if let (Some(token), Some(pr_number)) = (
                    token.as_ref(),
                    patch.and_then(|p| p.upstream.as_ref().and_then(|u| u.pr_number)),
                ) {
                    let upstream_url = remote_url(&repo, &queue.config.upstream_remote);
                    let contrib_url = remote_url(&repo, &queue.config.contrib_remote);
                    if let (Some((uo, ur)), Some((co, cr))) = (
                        upstream_url.as_deref().and_then(parse_github_repo),
                        contrib_url.as_deref().and_then(parse_github_repo),
                    ) {
                        let pr = get_pull_request(
                            &GithubConfig {
                                token: token.clone(),
                                api_url: env::var("UPLINK_GITHUB_API").ok(),
                                upstream_owner: uo,
                                upstream_repo: ur,
                                contrib_owner: co,
                                contrib_repo: cr,
                            },
                            pr_number,
                        )?;
                        if !pr.merged {
                            return Err(Error::msg(format!("PR {} is not merged yet", pr.url)));
                        }
                        mark_merged(&repo, &id, MergeVia::Pr, pr.merge_commit_sha.as_deref())?;
                        rebuild(&repo)?;
                        println!("{id} merged via PR {}", pr.number);
                        return Ok(());
                    }
                }
            }
            mark_merged(&repo, &id, via.clone(), None)?;
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
        Commands::Resolve { id } => match resolve_conflict(&repo, &id) {
            Ok(_) => println!("{id} resolved and queue rebuilt"),
            Err(Error::Conflict(err)) => {
                let queue = read_queue(&repo)?;
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
        },
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
