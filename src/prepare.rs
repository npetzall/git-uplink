use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;

use regex::Regex;

use crate::error::{Error, PrepareError, Result};
use crate::git::{git, git_ok, GitOpts};
use crate::queue::now_iso;
use crate::types::{
    PrepareCheck, PrepareReport, QueueState, DEFAULT_CUTOFF, DEFAULT_EXPORT_AUTHOR,
};

pub const OSS_ENVIRONMENT: &str = "oss";

pub const COMMIT_TEMPLATE: &str = concat!(
    "Use SHA-256 for tokens\n\n",
    "Explain the change the way an upstream maintainer should read it.\n",
    "Do not mention the company, internal issue trackers, or private\n",
    "hostnames above the cutoff.\n\n",
    "----- Uplink: internal below this line -----\n\n",
    "Internal (stripped before export):\n",
    "- Ticket: PROJ-1234\n",
    "- Uplink-Depends-On: upl_…\n",
    "- Uplink-Export-Author: Jane Public <jane@users.noreply.github.com>\n"
);

pub fn cutoff_marker(queue: &QueueState) -> String {
    queue
        .config
        .cutoff_marker
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_CUTOFF)
        .to_string()
}

pub fn split_internal_message(raw: &str, marker: &str) -> (String, String) {
    let stripped: String = raw
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(index) = stripped.find(marker) {
        (
            stripped[..index].trim().to_string(),
            stripped[index + marker.len()..].trim().to_string(),
        )
    } else {
        (stripped.trim().to_string(), String::new())
    }
}

pub fn parse_person(value: Option<&str>) -> Option<(String, String)> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let re = Regex::new(r"^(.*)\s+<([^>]+)>$").ok()?;
    if let Some(caps) = re.captures(value) {
        return Some((caps[1].trim().to_string(), caps[2].trim().to_string()));
    }
    if value.contains('@') && !value.contains(' ') {
        let name = value.split('@').next().unwrap_or(DEFAULT_EXPORT_AUTHOR.0);
        return Some((name.to_string(), value.to_string()));
    }
    None
}

fn keywords_for(queue: &QueueState) -> Vec<String> {
    let mut keys: BTreeSet<String> = queue.config.redact_keywords.iter().cloned().collect();
    if let Ok(from_env) = env::var("UPLINK_REDACT_KEYWORDS") {
        for item in from_env.split(',') {
            let item = item.trim();
            if !item.is_empty() {
                keys.insert(item.to_string());
            }
        }
    }
    keys.into_iter().collect()
}

fn internal_domains(queue: &QueueState) -> Vec<String> {
    let mut domains: BTreeSet<String> = queue
        .config
        .internal_email_domains
        .iter()
        .map(|d| d.to_lowercase())
        .collect();
    if let Ok(from_env) = env::var("UPLINK_INTERNAL_DOMAINS") {
        for item in from_env.split(',') {
            let item = item.trim().to_lowercase();
            if !item.is_empty() {
                domains.insert(item);
            }
        }
    }
    domains.into_iter().collect()
}

fn resolve_export_author(queue: &QueueState, internal_text: &str) -> (String, String) {
    let re = Regex::new(r"(?im)^Uplink-Export-Author:\s*(.+)$").unwrap();
    if let Some(caps) = re.captures(internal_text) {
        if let Some(person) = parse_person(Some(&caps[1])) {
            return person;
        }
    }
    if let Some(person) = parse_person(env::var("UPLINK_EXPORT_AUTHOR").ok().as_deref()) {
        return person;
    }
    (
        queue
            .config
            .export_author_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_EXPORT_AUTHOR.0)
            .to_string(),
        queue
            .config
            .export_author_email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_EXPORT_AUTHOR.1)
            .to_string(),
    )
}

fn subject_and_body(public_text: &str, fallback: &str) -> (String, String) {
    let mut lines = public_text.lines();
    let subject = lines.next().unwrap_or(fallback).trim().to_string();
    let subject = if subject.is_empty() {
        fallback.to_string()
    } else {
        subject
    };
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (subject, body)
}

fn find_keyword_hits(haystack: &str, keywords: &[String]) -> Vec<String> {
    let lower = haystack.to_lowercase();
    let mut hits = Vec::new();
    for keyword in keywords {
        if keyword.is_empty() {
            continue;
        }
        if lower.contains(&keyword.to_lowercase()) && !hits.contains(keyword) {
            hits.push(keyword.clone());
        }
    }
    hits
}

fn find_domain_hits(haystack: &str, domains: &[String]) -> Vec<String> {
    let mut hits = Vec::new();
    for domain in domains {
        let needle = format!("@{domain}");
        let re = Regex::new(&format!(r"@{}\b", regex::escape(domain))).unwrap();
        if re.is_match(haystack) && !hits.contains(&needle) {
            hits.push(needle);
        }
    }
    hits
}

pub fn export_commit_message(patch: &crate::types::Patch) -> String {
    let subject = patch
        .prepare
        .as_ref()
        .map(|p| p.public_subject.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&patch.title);
    let body = patch
        .prepare
        .as_ref()
        .map(|p| p.public_body.trim())
        .unwrap_or("");
    let mut lines = vec![subject.to_string()];
    if !body.is_empty() {
        lines.push(String::new());
        lines.push(body.to_string());
    }
    lines.push(String::new());
    lines.push(format!("Uplink-Patch-Id: {}", patch.id));
    lines.push(format!("Uplink-Intent: {}", patch.intent));
    lines.push(String::new());
    lines.join("\n")
}

pub fn install_commit_template(repo: &Path) -> Result<()> {
    fs::create_dir_all(repo.join(".uplink"))?;
    fs::write(repo.join(".uplink/commit-msg.template"), COMMIT_TEMPLATE)?;
    git(
        repo,
        &["config", "commit.template", ".uplink/commit-msg.template"],
        GitOpts::default(),
    )?;
    Ok(())
}

pub fn format_prepare_markdown(report: &PrepareReport) -> String {
    let checks = report
        .checks
        .iter()
        .map(|check| {
            let icon = match check.status.as_str() {
                "fail" => "FAIL",
                other => other,
            };
            format!("- **{}** ({icon}): {}", check.id, check.detail)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let public = if report.public_body.is_empty() {
        report.public_subject.clone()
    } else {
        format!("{}\n\n{}", report.public_subject, report.public_body)
    };
    format!(
        "## Uplink prepare-for-upstream\n\n\
This is the contribution as it would leave the enterprise. Internal lines below the cutoff are gone. Author is rewritten. Approvers can use this report instead of reconstructing the public PR by hand.\n\n\
**Ready:** {}\n\
**Public subject:** {}\n\
**Export author:** {} <{}>\n\
**Original author:** {} <{}>\n\
**Cutoff found:** {}\n\n\
### Public message\n\n\
```\n\
{public}\n\
```\n\n\
### Checks\n\n\
{checks}\n",
        if report.ok { "yes" } else { "no" },
        report.public_subject,
        report.author_name,
        report.author_email,
        report.original_author.as_deref().unwrap_or("unknown"),
        report.original_email.as_deref().unwrap_or(""),
        if report.cutoff_found {
            "yes"
        } else {
            "no — whole message treated as public"
        }
    )
}

pub fn format_approver_packet(patch: &crate::types::Patch) -> String {
    let prepare = patch
        .prepare
        .as_ref()
        .map(format_prepare_markdown)
        .unwrap_or_else(|| {
            "## Uplink prepare-for-upstream\n\nNo prepare report stored. Run `git uplink prepare` on the internal PR first.\n".into()
        });
    let pr = patch
        .source
        .internal_pr_url
        .clone()
        .or_else(|| patch.source.internal_pr_number.map(|n| format!("#{n}")))
        .unwrap_or_else(|| "not recorded".into());
    let depends = if patch.depends_on.is_empty() {
        "none".into()
    } else {
        patch
            .depends_on
            .iter()
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "# OSS contribution packet — {id}\n\n\
Review this packet (the same markdown is on the Actions job summary / `GITHUB_STEP_SUMMARY`), then approve the **{env}** GitHub Environment on the waiting Actions run. That approval is the IP gate. GitHub records it in the environment deployment history and the enterprise audit log. After you approve, the same run submits to the upstream-owned private fork.\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Patch | `{id}` |\n\
| Title | {title} |\n\
| Intent | {intent} |\n\
| Queue status | {status} |\n\
| Depends on | {depends} |\n\
| Internal PR | {pr} |\n\n\
{prepare}\n\
## What happens when you approve the oss environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/{id}/approval.md` on company main.\n\
3. `git uplink approve` then `git uplink submit` run with App credentials that exist **only** on the oss environment.\n\
4. No public PR is opened unless export preflight still passes.\n",
        id = patch.id,
        env = OSS_ENVIRONMENT,
        title = patch.title,
        intent = patch.intent,
        status = patch.status,
    )
}

pub struct ApprovalReceipt<'a> {
    pub patch_id: &'a str,
    pub environment: &'a str,
    pub actor: &'a str,
    pub run_url: &'a str,
    pub sha: &'a str,
    pub at: Option<String>,
}

pub fn format_approval_receipt(opts: ApprovalReceipt<'_>) -> String {
    let at = opts.at.unwrap_or_else(now_iso);
    format!(
        "# OSS environment approval — {id}\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Environment | `{env}` |\n\
| Workflow actor (dispatcher) | {actor} |\n\
| Environment reviewers | See the Deployments tab and the GitHub Enterprise audit log for this run |\n\
| Run | {run} |\n\
| HEAD | `{sha}` |\n\
| Recorded at | {at} |\n\n\
This file is the in-repo receipt. The authoritative approval event is the GitHub Environment review on **{env}**.\n",
        id = opts.patch_id,
        env = opts.environment,
        actor = opts.actor,
        run = opts.run_url,
        sha = opts.sha,
        at = at,
    )
}

pub fn report_paths(id: &str) -> (String, String, String) {
    let dir = format!(".uplink/reports/{id}");
    (
        dir.clone(),
        format!("{dir}/prepare.md"),
        format!("{dir}/approval.md"),
    )
}

pub fn prepare_from_range(
    repo: &Path,
    queue: &QueueState,
    from_ref: &str,
    head_ref: &str,
    title: Option<&str>,
    intent: &str,
) -> Result<PrepareReport> {
    let marker = cutoff_marker(queue);
    let log = git(
        repo,
        &[
            "log",
            "--reverse",
            "--format=%B%x1e",
            &format!("{from_ref}..{head_ref}"),
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    let messages: Vec<&str> = log
        .stdout
        .split('\u{1e}')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let fallback = title.unwrap_or("Contribution");
    let combined = if messages.is_empty() {
        fallback.to_string()
    } else {
        messages.join("\n\n")
    };
    let (public_text, internal_text) = split_internal_message(&combined, &marker);
    let (subject, body) = subject_and_body(&public_text, fallback);
    let author = resolve_export_author(queue, &internal_text);
    let original = git(
        repo,
        &["log", "-1", "--format=%an%x00%ae", head_ref],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    let mut orig = original.stdout.split('\u{0}');
    let original_author = orig.next().filter(|s| !s.is_empty()).map(str::to_string);
    let original_email = orig.next().filter(|s| !s.is_empty()).map(str::to_string);

    let diff = git_ok(
        repo,
        &[
            "diff",
            "--full-index",
            from_ref,
            head_ref,
            "--",
            ".",
            ":!.uplink",
        ],
    )?;
    let export_surface = format!("{subject}\n{body}\n{diff}");
    let keys = keywords_for(queue);
    let domains = internal_domains(queue);
    let key_hits = find_keyword_hits(&export_surface, &keys);
    let domain_hits = find_domain_hits(&export_surface, &domains);
    let ticket_re = Regex::new(r"\b[A-Z]{2,10}-\d+\b").unwrap();
    let tickets: Vec<&str> = ticket_re
        .find_iter(&public_text)
        .map(|m| m.as_str())
        .collect();

    let mut checks = vec![
        PrepareCheck {
            id: "message-scrubbed".into(),
            status: "pass".into(),
            detail: if !internal_text.is_empty() {
                "Internal section removed. Public body is what upstream will see.".into()
            } else {
                "No cutoff in the commits. The whole message is treated as public.".into()
            },
        },
        PrepareCheck {
            id: "cutoff-used".into(),
            status: if !internal_text.is_empty() {
                "pass".into()
            } else if !tickets.is_empty() {
                "warn".into()
            } else {
                "skip".into()
            },
            detail: if !internal_text.is_empty() {
                format!("Cutoff “{marker}” found.")
            } else if !tickets.is_empty() {
                format!(
                    "No cutoff, but public message contains {}. Put tickets below the cutoff.",
                    tickets.join(", ")
                )
            } else {
                format!("Optional. Add “{marker}” under the public body for issue ids and other internal notes.")
            },
        },
        PrepareCheck {
            id: "author-rewrite".into(),
            status: "pass".into(),
            detail: format!(
                "Export author {} <{}> (was {} <{}>). Company main still records the Uplink bot.",
                author.0,
                author.1,
                original_author.as_deref().unwrap_or("unknown"),
                original_email.as_deref().unwrap_or("")
            ),
        },
    ];

    if intent == "internal-only" {
        checks.push(PrepareCheck {
            id: "affiliation-leak".into(),
            status: "skip".into(),
            detail: "internal-only patches are not exported; keyword scan skipped.".into(),
        });
    } else if keys.is_empty() && domains.is_empty() {
        checks.push(PrepareCheck {
            id: "affiliation-leak".into(),
            status: "warn".into(),
            detail: "No redactKeywords / internalEmailDomains configured. Set them (or UPLINK_REDACT_KEYWORDS) so tests cannot mention the company.".into(),
        });
    } else {
        let mut hits = key_hits;
        hits.extend(domain_hits);
        checks.push(PrepareCheck {
            id: "affiliation-leak".into(),
            status: if hits.is_empty() { "pass" } else { "fail" }.into(),
            detail: if hits.is_empty() {
                "No configured company keywords or internal email domains in the export surface.".into()
            } else {
                format!(
                    "Export diff or public message contains: {}. Remove company names, internal hostnames, and staff emails from the contribution (including tests).",
                    hits.join(", ")
                )
            },
        });
    }

    let ok = checks.iter().all(|c| c.status != "fail");
    Ok(PrepareReport {
        at: now_iso(),
        ok,
        public_subject: subject,
        public_body: body,
        author_name: author.0,
        author_email: author.1,
        original_author,
        original_email,
        cutoff_found: !internal_text.is_empty() || combined.contains(&marker),
        checks,
    })
}

pub fn assert_prepare_ok(report: &PrepareReport, label: &str) -> Result<()> {
    if report.ok {
        return Ok(());
    }
    let failed = report
        .checks
        .iter()
        .filter(|c| c.status == "fail")
        .map(|c| format!("{}: {}", c.id, c.detail))
        .collect::<Vec<_>>()
        .join("\n");
    Err(Error::Prepare(PrepareError::new(
        format!("Prepare-for-upstream failed for {label}. No import/approval/submit until this is clean.\n{failed}"),
        report.clone(),
    )))
}
