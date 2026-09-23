use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;

use regex::Regex;

use crate::error::{AssessError, Error, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::queue::now_iso;
use crate::repo::{ensure_revs, has_ref, show_at, state_branch};
use crate::types::{
    AssessCheck, AssessReport, DEFAULT_CUTOFF, DEFAULT_EXPORT_AUTHOR, PATCH_DIR, Patch, QueueState,
};

pub const TO_UPSTREAM_ENVIRONMENT: &str = "to-upstream";
pub const FROM_UPSTREAM_ENVIRONMENT: &str = "from-upstream";

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

pub fn strip_html_comments(raw: &str) -> String {
    let comments = Regex::new(r"(?s)<!--.*?-->").expect("html comment regex");
    let stripped = comments.replace_all(raw, "");
    let blanks = Regex::new(r"\n[ \t]*\n(?:[ \t]*\n)+").expect("blank-line regex");
    blanks.replace_all(&stripped, "\n\n").trim().to_string()
}

pub fn split_internal_message(raw: &str, marker: &str) -> (String, String) {
    let stripped = strip_html_comments(raw);
    if let Some(index) = stripped.find(marker) {
        (
            stripped[..index].trim().to_string(),
            stripped[index + marker.len()..].trim().to_string(),
        )
    } else {
        (stripped, String::new())
    }
}

/// Patch ids recorded on `Uplink-Depends-On:` lines after HTML comments are stripped.
/// Only `upl_` plus 10 hex digits match (`new_patch_id`); placeholders and prose do not.
pub fn parse_depends_on(message: &str) -> Vec<String> {
    let stripped = strip_html_comments(message);
    let header = Regex::new(r"(?im)^Uplink-Depends-On:\s*(.+)$").expect("depends-on header");
    let id = Regex::new(r"(?i)^upl_[0-9a-f]{10}$").expect("patch id");
    let mut ids = Vec::new();
    for caps in header.captures_iter(&stripped) {
        for token in caps[1].split(|c: char| c.is_whitespace() || c == ',') {
            let token = token.trim();
            if token.is_empty() || !id.is_match(token) {
                continue;
            }
            let token = token.to_ascii_lowercase();
            if !ids.contains(&token) {
                ids.push(token);
            }
        }
    }
    ids
}

fn union_depends_on(from_message: Vec<String>, extra: &[String]) -> Vec<String> {
    let mut ids = from_message;
    for id in extra {
        if !id.is_empty() && !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids
}

pub fn depends_on_from_message(message: &str, extra: &[String]) -> Vec<String> {
    union_depends_on(parse_depends_on(message), extra)
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
    if let Some(caps) = re.captures(internal_text)
        && let Some(person) = parse_person(Some(&caps[1]))
    {
        return person;
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

pub(crate) fn append_patch_id_trailer(message: &str, id: &str) -> String {
    let mut body = message.trim_end().to_string();
    if !body.is_empty() {
        body.push('\n');
    }
    body.push('\n');
    body.push_str(&format!("Uplink-Patch-Id: {id}\n"));
    body
}

fn with_trailers(message: &str, patch: &Patch) -> String {
    append_patch_id_trailer(message, &patch.id)
}

fn public_subject_and_body(patch: &Patch) -> (String, String) {
    if let Some(assess) = &patch.assess {
        let subject = if assess.public_subject.is_empty() {
            patch.title.clone()
        } else {
            assess.public_subject.clone()
        };
        return (subject, assess.public_body.trim().to_string());
    }
    let (public, _) = split_internal_message(&stored_commit_message(patch), DEFAULT_CUTOFF);
    subject_and_body(&public, &patch.title)
}

pub fn stored_commit_message(patch: &Patch) -> String {
    if !patch.commit_message.trim().is_empty() {
        return patch.commit_message.trim().to_string();
    }
    if let Some(assess) = &patch.assess {
        if !assess.commit_message.trim().is_empty() {
            return assess.commit_message.trim().to_string();
        }
        let mut parts = vec![assess.public_subject.clone()];
        if !assess.public_body.trim().is_empty() {
            parts.push(String::new());
            parts.push(assess.public_body.trim().to_string());
        }
        let joined = parts.join("\n");
        if !joined.trim().is_empty() {
            return joined.trim().to_string();
        }
    }
    patch.title.clone()
}

pub fn company_commit_message(patch: &Patch) -> String {
    with_trailers(&stored_commit_message(patch), patch)
}

pub fn export_commit_message(patch: &Patch) -> String {
    let (subject, body) = public_subject_and_body(patch);
    let mut lines = vec![subject];
    if !body.is_empty() {
        lines.push(String::new());
        lines.push(body);
    }
    with_trailers(&lines.join("\n"), patch)
}

fn format_fenced(message: &str) -> String {
    format!("```\n{}\n```", message.trim_end())
}

pub fn format_assess_markdown(report: &AssessReport) -> String {
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
    let company = if report.commit_message.trim().is_empty() {
        public.clone()
    } else {
        report.commit_message.trim().to_string()
    };
    format!(
        "## Uplink assess-for-upstream\n\n\
This is the contribution as it would leave the enterprise. HTML comments from the PR template are stripped. Internal lines below the cutoff stay on company main and are removed before export. Author is rewritten. Approvers can use this report instead of reconstructing the public PR by hand.\n\n\
**Ready:** {}\n\
**Public subject:** {}\n\
**Export author:** {} <{}>\n\
**Original author:** {} <{}>\n\
**Cutoff found:** {}\n\n\
### Company commit message\n\n\
{}\n\n\
### Upstream commit message\n\n\
{}\n\n\
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
        },
        format_fenced(&company),
        format_fenced(&public),
    )
}

pub fn format_approver_packet(patch: &crate::types::Patch) -> String {
    let assessment = patch
        .assess
        .as_ref()
        .map(format_assess_markdown)
        .unwrap_or_else(|| {
            "## Uplink assess-for-upstream\n\nNo assess report stored. Run `git uplink assess` on the internal PR first.\n".into()
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
        "# Contribution packet — {id}\n\n\
Review this packet (the same markdown is on the Actions job summary / `GITHUB_STEP_SUMMARY`), then approve the **{env}** GitHub Environment on the waiting Actions run. That approval is the IP gate. GitHub records it in the environment deployment history and the enterprise audit log. After you approve, the same run submits to the upstream-owned private fork.\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Patch | `{id}` |\n\
| Title | {title} |\n\
| Queue | {queue} |\n\
| Queue status | {status} |\n\
| Depends on | {depends} |\n\
| Internal PR | {pr} |\n\n\
## Commit messages that will be used\n\n\
Company `main` keeps the cutoff and internal notes. The contribution fork does not.\n\n\
### Company main\n\n\
{company}\n\n\
### Upstream contrib\n\n\
{contrib}\n\n\
{assessment}\n\
## What happens when you approve the {env} environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/{id}/approval.md` on `uplink/state`.\n\
3. `git uplink approve` then `git uplink submit` run with App credentials that exist **only** on the {env} environment (git push to the contrib fork).\n\
4. The workflow opens the public pull request with `POST /repos/{{parent}}/pulls` (`head` is the branch, `head_repo` is `<contrib_owner>/<contrib_repo>`, `maintainer_can_modify` false) and runs `git uplink submitted`. No public PR is opened unless export preflight still passes.\n",
        id = patch.id,
        env = TO_UPSTREAM_ENVIRONMENT,
        title = patch.title,
        queue = "upstream",
        status = patch.status,
        company = format_fenced(&company_commit_message(patch)),
        contrib = format_fenced(&export_commit_message(patch)),
    )
}

pub fn format_contribution_packet(repo: &Path, patch: &Patch) -> Result<String> {
    format_contribution_packet_with_extras(repo, patch, None)
}

pub fn format_contribution_packet_with_extras(
    repo: &Path,
    patch: &Patch,
    extra_dir: Option<&Path>,
) -> Result<String> {
    let packet = if patch.status == "amended" {
        format_delta_approver_packet(repo, patch)?
    } else {
        format_approver_packet(patch)
    };
    prepend_report_extras(&packet, extra_dir)
}

pub fn load_extra_markdown(dir: &Path) -> Result<String> {
    if !dir.is_dir() {
        return Err(Error::msg(format!(
            "extra-dir {} is not a directory",
            dir.display()
        )));
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || !name.ends_with(".md") || entry.path().is_dir() {
            continue;
        }
        crate::queue::require_path_component(name.as_ref())?;
        names.push(name.into_owned());
    }
    names.sort();
    let mut parts = Vec::new();
    for name in names {
        let body = fs::read_to_string(dir.join(&name))?;
        let body = body.trim();
        if !body.is_empty() {
            parts.push(body.to_string());
        }
    }
    Ok(parts.join("\n\n"))
}

pub fn prepend_report_extras(packet: &str, extra_dir: Option<&Path>) -> Result<String> {
    let Some(dir) = extra_dir else {
        return Ok(packet.to_string());
    };
    let extras = load_extra_markdown(dir)?;
    if extras.is_empty() {
        return Ok(packet.to_string());
    }
    Ok(format!("{extras}\n\n{packet}"))
}

pub fn format_delta_approver_packet(repo: &Path, patch: &Patch) -> Result<String> {
    let last = patch.last_approval();
    let amendment = patch.approvals.len();
    let pr = patch
        .upstream
        .as_ref()
        .and_then(|u| u.pr_url.clone())
        .or_else(|| {
            patch
                .upstream
                .as_ref()
                .and_then(|u| u.pr_number)
                .map(|n| format!("#{n}"))
        })
        .unwrap_or_else(|| "not recorded".into());
    let last_at = last.map(|a| a.at.as_str()).unwrap_or("not recorded");
    let last_sha = last.map(|a| a.sha.as_str()).unwrap_or("not recorded");
    let last_run = last
        .and_then(|a| a.run_url.as_deref())
        .unwrap_or("not recorded");

    let delta = match last {
        Some(approval) => format_delta_since(repo, patch, &approval.sha)?,
        None => {
            "No prior approval SHA is recorded on this patch. Review the full updated contribution.\n"
                .into()
        }
    };
    let history = format_historical_approvals(repo, patch)?;

    Ok(format!(
        "# Delta packet — {id}\n\n\
This contribution was **already IP-approved** and submitted. Review **only the delta** since the last approval. Historical packets below were already approved; do not re-litigate them unless the delta depends on that context.\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Patch | `{id}` |\n\
| Title | {title} |\n\
| Queue | {queue} |\n\
| Queue status | {status} |\n\
| Amendment | {amendment} |\n\
| Public PR | {pr} |\n\
| Last approved at | {last_at} |\n\
| Last approved commit | `{last_sha}` |\n\
| Last approval run | {last_run} |\n\n\
## Delta since last approval\n\n\
{delta}\n\n\
## Commit messages that will be used\n\n\
Company `main` keeps the cutoff and internal notes. The contribution fork does not. **Upstream contrib** below is the message that will be used on the updated fork commit.\n\n\
### Company main\n\n\
{company}\n\n\
### Upstream contrib\n\n\
{contrib}\n\n\
## What happens when you approve the {env} environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/{id}/approval.md` on `uplink/state`.\n\
3. `git uplink approve` then `git uplink submit` run with App credentials that exist **only** on the {env} environment (git push to the contrib fork).\n\
4. The workflow opens the public pull request with `POST /repos/{{parent}}/pulls` (`head` is the branch, `head_repo` is `<contrib_owner>/<contrib_repo>`, `maintainer_can_modify` false) or reuses the recorded PR, then runs `git uplink submitted`. No second PR is opened.\n\n\
{history}",
        id = patch.id,
        env = TO_UPSTREAM_ENVIRONMENT,
        title = patch.title,
        queue = "upstream",
        status = patch.status,
        company = format_fenced(&company_commit_message(patch)),
        contrib = format_fenced(&export_commit_message(patch)),
    ))
}

fn format_delta_since(repo: &Path, patch: &Patch, sha: &str) -> Result<String> {
    let old_path = format!("{PATCH_DIR}/{}.patch", patch.id);
    let new_path = format!("{PATCH_DIR}/{}.patch", patch.id);
    let old_patch = match show_at(repo, sha, &old_path) {
        Ok(body) => body,
        Err(_) => {
            return Ok(format!(
                "Could not read `.uplink/patches/{}.patch` at `{sha}`.\n",
                patch.id
            ));
        }
    };
    let new_patch = fs::read_to_string(repo.join(&new_path))
        .unwrap_or_else(|_| show_at(repo, &state_branch(repo), &new_path).unwrap_or_default());
    if let Some(tree) = tree_diff_patches(repo, &old_patch, &new_patch) {
        return Ok(format!(
            "Source tree diff of the last approved patch vs the current patch, both applied on the same base.\n\n\
```\n{}\n```\n",
            tree.trim_end()
        ));
    }
    let file_diff = patch_file_diff(repo, &old_patch, &new_patch);
    Ok(format!(
        "The previously approved patch no longer applies cleanly on the current export base (typical after upstream moved). Diff of the two patch files, plus the current patch that will be exported:\n\n\
### Patch-file diff\n\n\
```\n{}\n```\n\n\
### Current patch (will be exported)\n\n\
```\n{}\n```\n",
        file_diff.trim_end(),
        new_patch.trim_end()
    ))
}

fn patch_file_diff(repo: &Path, old_patch: &str, new_patch: &str) -> String {
    let dir = env::temp_dir().join(format!("uplink-delta-files-{}", uuid::Uuid::new_v4()));
    let _ = fs::create_dir_all(&dir);
    let old_file = dir.join("approved.patch");
    let new_file = dir.join("current.patch");
    let _ = fs::write(&old_file, old_patch);
    let _ = fs::write(&new_file, new_patch);
    let result = git(
        repo,
        &[
            "diff",
            "--no-index",
            "--",
            old_file.to_str().unwrap_or(""),
            new_file.to_str().unwrap_or(""),
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    );
    let _ = fs::remove_dir_all(&dir);
    match result {
        Ok(out) if !out.stdout.trim().is_empty() => out.stdout,
        _ => "(no textual difference in patch files)".into(),
    }
}

fn tree_diff_patches(repo: &Path, old_patch: &str, new_patch: &str) -> Option<String> {
    let base = if has_ref(repo, "uplink/upstream").ok()? {
        "uplink/upstream"
    } else {
        "HEAD"
    };
    let root = env::temp_dir().join(format!("uplink-delta-tree-{}", uuid::Uuid::new_v4()));
    let old_dir = root.join("old");
    let new_dir = root.join("new");
    let old_file = root.join("approved.patch");
    let new_file = root.join("current.patch");
    fs::create_dir_all(&root).ok()?;
    fs::write(&old_file, old_patch).ok()?;
    fs::write(&new_file, new_patch).ok()?;
    let added_old = git(
        repo,
        &["worktree", "add", "--detach", old_dir.to_str()?, base],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .ok()?;
    if added_old.code != 0 {
        let _ = fs::remove_dir_all(&root);
        return None;
    }
    let added_new = git(
        repo,
        &["worktree", "add", "--detach", new_dir.to_str()?, base],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .ok()?;
    if added_new.code != 0 {
        let _ = git(
            repo,
            &[
                "worktree",
                "remove",
                "--force",
                old_dir.to_str().unwrap_or(""),
            ],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        );
        let _ = fs::remove_dir_all(&root);
        return None;
    }
    let applied_old = git(
        &old_dir,
        &["apply", old_file.to_str().unwrap_or("")],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .ok();
    let applied_new = git(
        &new_dir,
        &["apply", new_file.to_str().unwrap_or("")],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .ok();
    let diff =
        if applied_old.is_some_and(|r| r.code == 0) && applied_new.is_some_and(|r| r.code == 0) {
            let old_tree = write_worktree_tree(&old_dir);
            let new_tree = write_worktree_tree(&new_dir);
            match (old_tree, new_tree) {
                (Some(old_tree), Some(new_tree)) => git(
                    repo,
                    &["diff", &old_tree, &new_tree],
                    GitOpts {
                        allow_fail: true,
                        ..GitOpts::default()
                    },
                )
                .ok()
                .map(|out| out.stdout)
                .filter(|s| !s.trim().is_empty()),
                _ => None,
            }
        } else {
            None
        };
    let _ = git(
        repo,
        &[
            "worktree",
            "remove",
            "--force",
            old_dir.to_str().unwrap_or(""),
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    );
    let _ = git(
        repo,
        &[
            "worktree",
            "remove",
            "--force",
            new_dir.to_str().unwrap_or(""),
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    );
    let _ = fs::remove_dir_all(&root);
    diff
}

fn write_worktree_tree(worktree: &Path) -> Option<String> {
    git(worktree, &["add", "-A"], GitOpts::default()).ok()?;
    git_ok(worktree, &["write-tree"]).ok()
}

fn format_historical_approvals(repo: &Path, patch: &Patch) -> Result<String> {
    if patch.approvals.is_empty() {
        return Ok(
            "## Previously approved packets\n\nNo prior approval SHAs are recorded.\n".into(),
        );
    }
    let mut sections = vec![
        "## Previously approved packets\n\nEach packet below **was already approved**. The delta above is what still needs review.\n"
            .to_string(),
    ];
    let (_, assessment_path, approval_path) = report_paths(&patch.id)?;
    for approval in &patch.approvals {
        let packet = show_at(repo, &approval.sha, &assessment_path)
            .unwrap_or_else(|_| "No `assessment.md` stored at this commit.\n".into());
        let receipt = show_at(repo, &approval.sha, &approval_path).ok();
        let run = approval.run_url.as_deref().unwrap_or("not recorded");
        let mut body = format!(
            "### Already approved ({kind}) — {at}\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Version | {version} |\n\
| Queue commit | `{sha}` |\n\
| Run | {run} |\n\n\
This historical report has **already been approved**.\n\n\
{packet}\n",
            kind = approval.kind,
            at = approval.at,
            version = approval.version,
            sha = approval.sha,
        );
        if let Some(receipt) = receipt
            && !receipt.trim().is_empty()
        {
            body.push_str("\n<details>\n<summary>Approval receipt at this commit</summary>\n\n");
            body.push_str(&receipt);
            body.push_str("\n</details>\n");
        }
        sections.push(body);
    }
    Ok(sections.join("\n"))
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
        "# {env} environment approval — {id}\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Environment | `{env}` |\n\
| Workflow actor (dispatcher) | {actor} |\n\
| Environment reviewers | See the Deployments tab and the GitHub Enterprise audit log for this run |\n\
| Run | {run} |\n\
| Queue commit | `{sha}` |\n\
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

pub fn report_paths(id: &str) -> Result<(String, String, String)> {
    let id = crate::queue::require_path_component(id)?;
    let dir = format!(".uplink/reports/{id}");
    Ok((
        dir.clone(),
        format!("{dir}/assessment.md"),
        format!("{dir}/approval.md"),
    ))
}

pub fn from_upstream_report_paths() -> (String, String, String) {
    let dir = ".uplink/reports/from-upstream".to_string();
    (
        dir.clone(),
        format!("{dir}/incoming.md"),
        format!("{dir}/approval.md"),
    )
}

pub struct IncomingFlowedBack<'a> {
    pub id: &'a str,
    pub via: &'a str,
    pub title: &'a str,
    pub sha: &'a str,
}

pub fn format_incoming_packet(
    repo: &Path,
    from_sha: Option<&str>,
    pending_sha: &str,
    flowed_back: &[IncomingFlowedBack<'_>],
    foreign_shas: &[String],
) -> Result<String> {
    let env = FROM_UPSTREAM_ENVIRONMENT;
    let current = from_sha.unwrap_or("not recorded");
    let flowed_list = if flowed_back.is_empty() {
        "None.\n".to_string()
    } else {
        flowed_back
            .iter()
            .map(|item| {
                format!(
                    "- `{id}` via {via} at `{sha}` — {title}\n",
                    id = item.id,
                    via = item.via,
                    sha = item.sha,
                    title = item.title,
                )
            })
            .collect()
    };
    let mut foreign = String::new();
    if foreign_shas.is_empty() {
        foreign.push_str("None.\n");
    } else {
        for sha in foreign_shas {
            let shown = git(
                repo,
                &["show", "--pretty=fuller", sha],
                GitOpts {
                    allow_fail: true,
                    ..GitOpts::default()
                },
            )?;
            let body = if shown.stdout.trim().is_empty() {
                format!("(no `git show` output for `{sha}`)\n")
            } else {
                format_fenced(&shown.stdout)
            };
            foreign.push_str(&format!("### `{sha}`\n\n{body}\n\n"));
        }
    }
    Ok(format!(
        "# Incoming upstream — {env}\n\n\
These commits on public main are not matched to any company patch. Review this packet (the same markdown is on the Actions job summary / `GITHUB_STEP_SUMMARY`), then approve the **{env}** GitHub Environment on the waiting Actions run. That approval updates `uplink/upstream` and rebuilds company main. GitHub records it in the environment deployment history and the enterprise audit log.\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Current `uplink/upstream` | `{current}` |\n\
| Pending public main | `{pending}` |\n\
| Flowed back | {flowed_n} |\n\
| Foreign commits | {foreign_n} |\n\n\
## Flowed back (no extra review)\n\n\
These commits match a company patch (`{trailer}` trailer or `git patch-id --stable`). They will be marked `merged` when you approve.\n\n\
{flowed_list}\n\
## Foreign commits\n\n\
{foreign}\
## What happens when you approve the {env} environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/from-upstream/approval.md` on `uplink/state`.\n\
3. `git uplink accept-upstream` moves `uplink/upstream` to `{pending}` and rebuilds company main.\n\
4. Flowed-back patches are marked merged and are not applied again. Remaining patches replay onto the new upstream.\n",
        trailer = "Uplink-Patch-Id",
        pending = pending_sha,
        flowed_n = flowed_back.len(),
        foreign_n = foreign_shas.len(),
    ))
}

pub fn assess_from_message(
    repo: &Path,
    queue: &QueueState,
    from_ref: &str,
    head_ref: &str,
    message: &str,
    title: Option<&str>,
    intent: &str,
) -> Result<AssessReport> {
    let shas = ensure_revs(repo, &[from_ref, head_ref])?;
    let from_ref = shas[0].as_str();
    let head_ref = shas[1].as_str();
    let marker = cutoff_marker(queue);
    let fallback = title.unwrap_or("Contribution");
    let mut stored = strip_html_comments(message);
    if stored.is_empty() {
        stored = fallback.to_string();
    }
    let (public_text, internal_text) = split_internal_message(&stored, &marker);
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
        AssessCheck {
            id: "message-scrubbed".into(),
            status: "pass".into(),
            detail: if !internal_text.is_empty() {
                "Internal section removed. Public body is what upstream will see.".into()
            } else {
                "No cutoff in the message. The whole message is treated as public.".into()
            },
        },
        AssessCheck {
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
                format!(
                    "Optional. Add “{marker}” under the public body for issue ids and other internal notes."
                )
            },
        },
        AssessCheck {
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
        checks.push(AssessCheck {
            id: "affiliation-leak".into(),
            status: "skip".into(),
            detail: "internal-only patches are not exported; keyword scan skipped.".into(),
        });
    } else if keys.is_empty() && domains.is_empty() {
        checks.push(AssessCheck {
            id: "affiliation-leak".into(),
            status: "warn".into(),
            detail: "No redactKeywords / internalEmailDomains configured. Set them (or UPLINK_REDACT_KEYWORDS) so tests cannot mention the company.".into(),
        });
    } else {
        let mut hits = key_hits;
        hits.extend(domain_hits);
        checks.push(AssessCheck {
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
    let cutoff_found = !internal_text.is_empty() || stored.contains(&marker);
    Ok(AssessReport {
        at: now_iso(),
        ok,
        commit_message: stored,
        public_subject: subject,
        public_body: body,
        author_name: author.0,
        author_email: author.1,
        original_author,
        original_email,
        cutoff_found,
        checks,
    })
}

pub fn assert_assess_ok(report: &AssessReport, label: &str) -> Result<()> {
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
    Err(Error::Assess(AssessError::new(
        format!(
            "Assess-for-upstream failed for {label}. No import/approval/submit until this is clean.\n{failed}"
        ),
        report.clone(),
    )))
}
