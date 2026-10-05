use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::{Regex, RegexSet};

use crate::error::{AssessError, Error, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::ops::IncomingClaim;
use crate::queue::now_iso;
use crate::repo::{TempWorktree, ensure_revs, has_ref, show_at};
use crate::types::{
    AssessCheck, AssessReport, CheckStatus, DEFAULT_CUTOFF, MergeVia, PATCH_DIR, Patch,
    PatchExtras, PatchIntent, PatchStatus, QueueState, STATE_BRANCH,
};

static HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("html comment regex"));
static BLANK_LINES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n[ \t]*\n(?:[ \t]*\n)+").expect("blank-line regex"));
static DEPENDS_ON_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^Uplink-Depends-On:\s*(.+)$").expect("depends-on header regex")
});
static PATCH_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^upl_[0-9a-f]{10}$").expect("patch id regex"));
static PERSON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*)\s+<([^>]+)>$").expect("person regex"));
static EXPORT_AUTHOR_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^Uplink-Export-Author:\s*(.+)$").expect("export-author header regex")
});
static TICKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{2,10}-\d+\b").expect("ticket regex"));

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
    let stripped = HTML_COMMENT.replace_all(raw, "");
    BLANK_LINES
        .replace_all(&stripped, "\n\n")
        .trim()
        .to_string()
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
    let mut ids = Vec::new();
    for caps in DEPENDS_ON_HEADER.captures_iter(&stripped) {
        for token in caps[1].split(|c: char| c.is_whitespace() || c == ',') {
            let token = token.trim();
            if token.is_empty() || !PATCH_ID.is_match(token) {
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

/// `public_text` without its `Uplink-Depends-On:` lines. They name internal
/// patch ids, so they stay on company `main` and are never exported.
fn strip_depends_on(public_text: &str) -> String {
    let stripped = DEPENDS_ON_HEADER.replace_all(public_text, "");
    BLANK_LINES
        .replace_all(&stripped, "\n\n")
        .trim()
        .to_string()
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
    if let Some(caps) = PERSON.captures(value) {
        return Some((caps[1].trim().to_string(), caps[2].trim().to_string()));
    }
    if value.contains('@') && !value.contains(' ') {
        let name = value.split('@').next().unwrap_or(value);
        return Some((name.to_string(), value.to_string()));
    }
    None
}

/// The `Uplink-Export-Author` header below the cutoff, if any.
fn resolve_co_author(internal_text: &str) -> Option<(String, String)> {
    let caps = EXPORT_AUTHOR_HEADER.captures(internal_text)?;
    parse_person(Some(&caps[1]))
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

/// Configured domains are dynamic, so they compile once per call as one set.
fn find_domain_hits(haystack: &str, domains: &[String]) -> Vec<String> {
    let set = RegexSet::new(
        domains
            .iter()
            .map(|domain| format!(r"(?i)@{}\b", regex::escape(domain))),
    )
    .expect("escaped domain regex");
    let matched = set.matches(haystack);
    let mut hits = Vec::new();
    for (index, domain) in domains.iter().enumerate() {
        let needle = format!("@{domain}");
        if matched.matched(index) && !hits.contains(&needle) {
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
    // Without a report no text was scanned and the cutoff is not known, so
    // nothing of the stored message is exported.
    (patch.title.clone(), String::new())
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

/// Identifies what leaves the company for `patch`: its content, the public
/// PR title, and the public commit message. An approval records the token it
/// was given for, and `submit` exports only content with that approval.
pub fn review_token(repo: &Path, patch: &Patch) -> Result<String> {
    let Some(stable) = patch.patch_id_stable.as_deref() else {
        return Err(Error::msg(format!(
            "{} has no stable patch id; rebuild before asking for approval",
            patch.id
        )));
    };
    let content = format!(
        "{stable}\n{}\n{}",
        patch.title.trim(),
        export_commit_message(patch)
    );
    Ok(git(
        repo,
        &["hash-object", "--stdin"],
        GitOpts {
            input: Some(content.as_bytes()),
            ..GitOpts::default()
        },
    )?
    .stdout)
}

/// Where `report` writes the review token of the packet it produced.
pub fn review_token_path(id: &str) -> Result<String> {
    Ok(format!("{}/review-token", report_paths(id)?.0))
}

pub fn export_commit_message(patch: &Patch) -> String {
    let (subject, body) = public_subject_and_body(patch);
    let mut lines = vec![subject];
    if !body.is_empty() {
        lines.push(String::new());
        lines.push(body);
    }
    let message = with_trailers(&lines.join("\n"), patch);
    match patch.assess.as_ref().and_then(|a| a.co_author.as_deref()) {
        Some(co_author) => format!("{message}Co-Authored-By: {co_author}\n"),
        None => message,
    }
}

/// A fenced block the content cannot close: the fence is longer than any
/// run of backticks inside it.
fn format_fenced(message: &str) -> String {
    let longest = message.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{}\n{fence}", message.trim_end())
}

/// Explains the report's checks and how to fix a failure.
pub const ASSESS_DOCS_URL: &str = "https://npetzall.github.io/git-uplink/day-to-day#assess-fails";

fn assessment_heading(ok: bool) -> String {
    format!("## Upstream Assessment: {}", if ok { "✅" } else { "❌" })
}

fn check_emoji(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Pass => "✅",
        CheckStatus::Fail => "❌",
        CheckStatus::Warn => "⚠️",
        CheckStatus::Skip => "⏭️",
    }
}

/// Keeps free text inside one markdown table cell.
fn table_cell(text: &str) -> String {
    text.trim().replace('|', "\\|").replace('\n', "<br>")
}

/// Text we did not write (a title, an upstream author or subject), kept
/// inside one table cell or list item and unable to start markup.
fn plain_cell(text: &str) -> String {
    table_cell(text).replace('<', "&lt;").replace('`', "'")
}

fn format_checks_table(report: &AssessReport) -> String {
    let rows = report
        .checks
        .iter()
        .map(|check| {
            format!(
                "| `{}` | {} | {} |",
                check.id,
                table_cell(&check.detail),
                check_emoji(check.status)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("### Checks\n\n| Check | Description | Result |\n| --- | --- | --- |\n{rows}\n")
}

/// The report as it appears on the PR: the public title and body as they will
/// leave the company, then one row per check. The company commit message is
/// deliberately left out.
pub fn format_assess_markdown(report: &AssessReport) -> String {
    let body = if report.public_body.trim().is_empty() {
        "_empty_".to_string()
    } else {
        format_fenced(&report.public_body)
    };
    format!(
        "{heading}\n\n\
[What this checks and how to fix a failure]({ASSESS_DOCS_URL})\n\n\
### Public title\n\n\
{title}\n\n\
### Public body\n\n\
{body}\n\n\
{checks}",
        heading = assessment_heading(report.ok),
        title = format_fenced(&report.public_subject),
        checks = format_checks_table(report),
    )
}

/// The report as it appears in the contribution packet, which already shows
/// the exact public commit message: the verdict and the checks only.
pub fn format_assess_checks_markdown(report: &AssessReport) -> String {
    format!(
        "{heading}\n\n\
[What this checks and how to fix a failure]({ASSESS_DOCS_URL})\n\n\
{checks}",
        heading = assessment_heading(report.ok),
        checks = format_checks_table(report),
    )
}

fn packet_assessment(patch: &Patch) -> String {
    patch
        .assess
        .as_ref()
        .map(format_assess_checks_markdown)
        .unwrap_or_else(|| {
            format!(
                "{}\n\nNo assess report stored. Run `git uplink assess` on the internal PR first.\n",
                assessment_heading(false)
            )
        })
}

pub fn format_approver_packet(patch: &crate::types::Patch) -> String {
    approver_packet(patch, None)
}

/// `| Review token | … |`, or nothing when the patch has none yet.
fn review_token_row(token: Option<&str>) -> String {
    token
        .map(|token| format!("| Review token | `{token}` |\n"))
        .unwrap_or_default()
}

fn approver_packet(patch: &Patch, token: Option<&str>) -> String {
    let assessment = packet_assessment(patch);
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
| Field | Value |\n\
| --- | --- |\n\
| Patch | `{id}` |\n\
| Title | {title} |\n\
| Depends on | {depends} |\n\
| Internal PR | {pr} |\n\
{token}\n\
## Upstream commit message\n\n\
{contrib}\n\n\
{assessment}\n\
## What happens when you approve the {env} environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/{id}/approval.md` on `uplink/state`.\n\
3. `git uplink approve` then `git uplink submit` run with App credentials that exist **only** on the {env} environment. The approval is for the review token above: if the patch changed since this packet, approve stops and nothing is exported.\n\
4. The workflow opens the public pull request with `POST /repos/{{parent}}/pulls` (`head` is the branch, `head_repo` is `<contrib_owner>/<contrib_repo>`, `maintainer_can_modify` false) and runs `git uplink submitted`. No public PR is opened unless export preflight still passes.\n",
        token = review_token_row(token),
        id = patch.id,
        env = TO_UPSTREAM_ENVIRONMENT,
        title = plain_cell(&patch.title),
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
    let packet = if patch.status == PatchStatus::Amended {
        format_delta_approver_packet(repo, patch)?
    } else {
        approver_packet(patch, review_token(repo, patch).ok().as_deref())
    };
    if extra_dir.is_none() && stored_extras_fresh(patch) {
        let stored = repo.join(extras_dir(&patch.id)?);
        if stored.is_dir() {
            return prepend_report_extras(&packet, Some(&stored));
        }
    }
    prepend_report_extras(&packet, extra_dir)
}

/// Where the stored company extras for patch `id` live on `uplink/state`.
pub fn extras_dir(id: &str) -> Result<String> {
    Ok(format!("{}/extras", report_paths(id)?.0))
}

/// Stored extras are reused while the patch content is unchanged. An amended
/// patch always gets a fresh assessment.
pub fn stored_extras_fresh(patch: &Patch) -> bool {
    patch.status != PatchStatus::Amended
        && patch
            .extras
            .as_ref()
            .is_some_and(|extras| patch.patch_id_stable.as_ref() == Some(&extras.patch_id_stable))
}

/// Replaces the stored extras for `patch` with the `*.md` files in `src` and
/// records them as fresh for the patch's current content. No files is a valid
/// result: the hook ran and had nothing to add.
pub fn store_extras(
    repo: &Path,
    patch: &mut Patch,
    src: &Path,
    source: Option<String>,
) -> Result<()> {
    let Some(stable) = patch.patch_id_stable.clone() else {
        return Err(Error::msg(format!(
            "{} has no stable patch id; cannot store extras",
            patch.id
        )));
    };
    let names = extra_markdown_names(src)?;
    let dest = repo.join(extras_dir(&patch.id)?);
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(&dest)?;
    for name in names {
        fs::copy(src.join(&name), dest.join(&name))?;
    }
    patch.extras = Some(PatchExtras {
        at: now_iso(),
        patch_id_stable: stable,
        source,
    });
    Ok(())
}

fn extra_markdown_names(dir: &Path) -> Result<Vec<String>> {
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
    Ok(names)
}

pub fn load_extra_markdown(dir: &Path) -> Result<String> {
    let names = extra_markdown_names(dir)?;
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
| Amendment | {amendment} |\n\
| Public PR | {pr} |\n\
| Last approved at | {last_at} |\n\
| Last approved commit | `{last_sha}` |\n\
| Last approval run | {last_run} |\n\
{token}\n\
## Delta since last approval\n\n\
{delta}\n\n\
## Upstream commit message\n\n\
{contrib}\n\n\
{assessment}\n\
## What happens when you approve the {env} environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/{id}/approval.md` on `uplink/state`.\n\
3. `git uplink approve` then `git uplink submit` run with App credentials that exist **only** on the {env} environment. The approval is for the review token above: if the patch changed since this packet, approve stops and nothing is exported.\n\
4. The workflow opens the public pull request with `POST /repos/{{parent}}/pulls` (`head` is the branch, `head_repo` is `<contrib_owner>/<contrib_repo>`, `maintainer_can_modify` false) or reuses the recorded PR, then runs `git uplink submitted`. No second PR is opened.\n\n\
{history}",
        token = review_token_row(review_token(repo, patch).ok().as_deref()),
        id = patch.id,
        env = TO_UPSTREAM_ENVIRONMENT,
        title = plain_cell(&patch.title),
        contrib = format_fenced(&export_commit_message(patch)),
        assessment = packet_assessment(patch),
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
        .unwrap_or_else(|_| show_at(repo, STATE_BRANCH, &new_path).unwrap_or_default());
    if let Some(tree) = tree_diff_patches(repo, &old_patch, &new_patch) {
        return Ok(delta_tree_section(&tree));
    }
    let file_diff = patch_file_diff(repo, &old_patch, &new_patch);
    Ok(delta_files_section(&file_diff, &new_patch))
}

fn delta_tree_section(tree: &str) -> String {
    format!(
        "Source tree diff of the last approved patch vs the current patch, both applied on the same base.\n\n\
{}\n",
        format_fenced(tree)
    )
}

fn delta_files_section(file_diff: &str, new_patch: &str) -> String {
    format!(
        "The previously approved patch no longer applies cleanly on the current export base (typical after upstream moved). Diff of the two patch files, plus the current patch that will be exported:\n\n\
### Patch-file diff\n\n\
{}\n\n\
### Current patch (will be exported)\n\n\
{}\n",
        format_fenced(file_diff),
        format_fenced(new_patch)
    )
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
        GitOpts::allow_fail(),
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
    let root =
        RemoveOnDrop(env::temp_dir().join(format!("uplink-delta-tree-{}", uuid::Uuid::new_v4())));
    fs::create_dir_all(&root.0).ok()?;
    let old_tree = patched_tree(repo, &root.0, "approved", base, old_patch)?;
    let new_tree = patched_tree(repo, &root.0, "current", base, new_patch)?;
    git(repo, &["diff", &old_tree, &new_tree], GitOpts::allow_fail())
        .ok()
        .map(|out| out.stdout)
        .filter(|s| !s.trim().is_empty())
}

/// Tree id of `base` with `patch` applied, built in a throwaway worktree
/// under `root`. None when the worktree cannot be added or the patch fails.
fn patched_tree(repo: &Path, root: &Path, name: &str, base: &str, patch: &str) -> Option<String> {
    let patch_file = root.join(format!("{name}.patch"));
    fs::write(&patch_file, patch).ok()?;
    let dir = root.join(name);
    if !git_succeeds(repo, &["worktree", "add", "--detach", dir.to_str()?, base]).ok()? {
        return None;
    }
    let worktree = TempWorktree { repo, dir };
    if !git_succeeds(&worktree.dir, &["apply", patch_file.to_str()?]).ok()? {
        return None;
    }
    write_worktree_tree(&worktree.dir)
}

/// Removes a directory tree when dropped.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
    /// Review token of the approved content; none for from-upstream.
    pub reviewed: Option<&'a str>,
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
{token}\
| Recorded at | {at} |\n\n\
This file is the in-repo receipt. The authoritative approval event is the GitHub Environment review on **{env}**.\n",
        id = opts.patch_id,
        env = opts.environment,
        actor = opts.actor,
        run = opts.run_url,
        sha = opts.sha,
        token = review_token_row(opts.reviewed),
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

/// A patch the packet says will be marked merged on approval.
pub struct IncomingMergeRow {
    pub id: String,
    pub title: String,
    pub via: MergeVia,
    pub sha: String,
    pub modified: bool,
}

/// One commit of the pending range, for the packet's commit table.
pub struct IncomingCommitRow {
    pub sha: String,
    pub author: String,
    pub subject: String,
    /// Markdown: how sync classified the commit.
    pub class: String,
}

pub struct IncomingPacket<'a> {
    pub from_sha: Option<&'a str>,
    pub pending_sha: &'a str,
    pub trailer_key: &'a str,
    pub merges: &'a [IncomingMergeRow],
    pub claims: &'a [IncomingClaim],
    pub commits: &'a [IncomingCommitRow],
    /// `git diff --stat` and the diff of the unaccounted changes.
    pub stat: &'a str,
    pub diff: &'a str,
}

pub fn format_incoming_packet(packet: &IncomingPacket<'_>) -> String {
    let env = FROM_UPSTREAM_ENVIRONMENT;
    let pending = packet.pending_sha;
    let current = packet.from_sha.unwrap_or("not recorded");
    let merges = if packet.merges.is_empty() {
        "None.\n".to_string()
    } else {
        packet
            .merges
            .iter()
            .map(|item| {
                let note = if item.modified {
                    " **Modified by the maintainer**: the public PR merged, but this commit does not contain the patch as we hold it. The difference is part of the unaccounted changes below."
                } else {
                    ""
                };
                format!(
                    "- `{id}` via {via} at `{sha}` — {title}.{note}\n",
                    id = item.id,
                    via = item.via,
                    sha = item.sha,
                    title = plain_cell(&item.title),
                )
            })
            .collect()
    };
    let claims = if packet.claims.is_empty() {
        "None.\n".to_string()
    } else {
        packet
            .claims
            .iter()
            .map(|claim| {
                format!(
                    "- `{sha}` carries `{key}: {id}`, but its diff is not that patch. Approving does **not** mark `{id}` merged; it is applied again on the new upstream. If upstream did take it in another form, run `git uplink merged {id} --sha {sha}`.\n",
                    sha = claim.sha,
                    key = packet.trailer_key,
                    id = claim.id,
                )
            })
            .collect()
    };
    let commits = packet
        .commits
        .iter()
        .map(|commit| {
            format!(
                "| `{}` | {} | {} | {} |\n",
                commit.sha,
                plain_cell(&commit.author),
                plain_cell(&commit.subject),
                commit.class
            )
        })
        .collect::<String>();
    format!(
        "# Incoming upstream — {env}\n\n\
Public main moved, and company patches do not explain all of it. Review the unaccounted changes below (the same markdown is on the Actions job summary / `GITHUB_STEP_SUMMARY`), then approve the **{env}** GitHub Environment on the waiting Actions run. That approval updates `uplink/upstream` and rebuilds company main. GitHub records it in the environment deployment history and the enterprise audit log.\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Current `uplink/upstream` | `{current}` |\n\
| Pending public main | `{pending}` |\n\
| Commits | {commit_n} |\n\
| Patches merged | {merge_n} |\n\
| Unproven claims | {claim_n} |\n\n\
## Marked merged when you approve\n\n\
A patch is merged when a commit has its `git patch-id --stable`, or when its recorded public pull request is merged. The `{key}` trailer alone is not enough.\n\n\
{merges}\n\
## Claims without proof\n\n\
{claims}\n\
## Commits\n\n\
| Commit | Author | Subject | Classified |\n\
| --- | --- | --- | --- |\n\
{commits}\n\
## Unaccounted changes\n\n\
The diff from `uplink/upstream` with the merged patches applied to pending public main. This is what the approval is for.\n\n\
{stat}\n\n\
{diff}\n\n\
## What happens when you approve the {env} environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/from-upstream/approval.md` on `uplink/state`.\n\
3. `git uplink accept-upstream` moves `uplink/upstream` to `{pending}` and rebuilds company main.\n\
4. The patches listed above are marked merged and are not applied again. Remaining patches replay onto the new upstream; one that applies empty is marked merged too.\n",
        key = packet.trailer_key,
        commit_n = packet.commits.len(),
        merge_n = packet.merges.len(),
        claim_n = packet.claims.len(),
        stat = format_fenced(packet.stat),
        diff = format_fenced(packet.diff),
    )
}

pub fn assess_from_message(
    repo: &Path,
    queue: &QueueState,
    from_ref: &str,
    head_ref: &str,
    message: &str,
    title: Option<&str>,
    intent: PatchIntent,
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
    let public_text = strip_depends_on(&public_text);
    let (subject, body) = subject_and_body(&public_text, fallback);
    let co_author = resolve_co_author(&internal_text);
    let (original_author, original_email) = head_author(repo, head_ref)?;

    let mut checks = message_checks(
        &marker,
        &public_text,
        &internal_text,
        co_author.as_ref(),
        (original_author.as_deref(), original_email.as_deref()),
    );
    if intent.is_internal_only() {
        checks.push(AssessCheck {
            id: "affiliation-leak".into(),
            status: CheckStatus::Skip,
            detail: "internal-only patches are not exported; keyword scan skipped.".into(),
        });
    } else {
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
        // The co-author trailer lands in the public commit, so it is scanned too.
        let trailer = co_author
            .as_ref()
            .map(|(name, email)| format!("{name} <{email}>\n"))
            .unwrap_or_default();
        // The title is the public PR title; it can differ from the subject.
        let title = title.unwrap_or_default();
        let export_surface = format!("{trailer}{title}\n{subject}\n{body}\n{diff}");
        checks.push(affiliation_check(queue, &export_surface));
        checks.extend(binary_files_check(repo, from_ref, head_ref)?);
    }

    let ok = checks.iter().all(|c| c.status != CheckStatus::Fail);
    let cutoff_found = !internal_text.is_empty() || stored.contains(&marker);
    Ok(AssessReport {
        at: now_iso(),
        ok,
        commit_message: stored,
        public_subject: subject,
        public_body: body,
        co_author: co_author.map(|(name, email)| format!("{name} <{email}>")),
        original_author,
        original_email,
        cutoff_found,
        checks,
    })
}

/// Author name and email of `head_ref`, when git has them.
fn head_author(repo: &Path, head_ref: &str) -> Result<(Option<String>, Option<String>)> {
    let original = git(
        repo,
        &["log", "-1", "--format=%an%x00%ae", head_ref],
        GitOpts::allow_fail(),
    )?;
    let mut parts = original.stdout.split('\u{0}');
    let name = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let email = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    Ok((name, email))
}

/// Checks on the commit message and co-author: message-scrubbed, cutoff-used, co-author.
fn message_checks(
    marker: &str,
    public_text: &str,
    internal_text: &str,
    co_author: Option<&(String, String)>,
    original: (Option<&str>, Option<&str>),
) -> Vec<AssessCheck> {
    let tickets: Vec<&str> = TICKET.find_iter(public_text).map(|m| m.as_str()).collect();
    vec![
        AssessCheck {
            id: "message-scrubbed".into(),
            status: CheckStatus::Pass,
            detail: if !internal_text.is_empty() {
                "Internal section removed. Public body is what upstream will see.".into()
            } else {
                "No cutoff in the message. The whole message is treated as public.".into()
            },
        },
        AssessCheck {
            id: "cutoff-used".into(),
            status: if !internal_text.is_empty() {
                CheckStatus::Pass
            } else if !tickets.is_empty() {
                CheckStatus::Warn
            } else {
                CheckStatus::Skip
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
        match co_author {
            Some((name, email)) => AssessCheck {
                id: "co-author".into(),
                status: CheckStatus::Pass,
                detail: format!(
                    "Co-Authored-By: {name} <{email}> (wrote {} <{}>). The commit author is the contrib token's identity.",
                    original.0.unwrap_or("unknown"),
                    original.1.unwrap_or("")
                ),
            },
            None => AssessCheck {
                id: "co-author".into(),
                status: CheckStatus::Skip,
                detail: "No Uplink-Export-Author below the cutoff, so no Co-Authored-By trailer. The commit author is the contrib token's identity.".into(),
            },
        },
    ]
}

/// Scans the export surface for configured company keywords and internal email domains.
fn affiliation_check(queue: &QueueState, export_surface: &str) -> AssessCheck {
    let settings = match queue.settings.usable() {
        Ok(settings) => settings,
        // Fail closed: without the settings there is nothing to scan for.
        Err(err) => {
            return AssessCheck {
                id: "affiliation-leak".into(),
                status: CheckStatus::Fail,
                detail: format!("Cannot scan for company keywords: {err}."),
            };
        }
    };
    let keys = &settings.redact_keywords;
    let domains: Vec<String> = settings
        .internal_email_domains
        .iter()
        .map(|d| d.to_lowercase())
        .collect();
    if keys.is_empty() && domains.is_empty() {
        return AssessCheck {
            id: "affiliation-leak".into(),
            status: CheckStatus::Warn,
            detail: "No redact_keywords / internal_email_domains configured. Set them in uplink.toml on uplink/hooks so tests cannot mention the company.".into(),
        };
    }
    let mut hits = find_keyword_hits(export_surface, keys);
    hits.extend(find_domain_hits(export_surface, &domains));
    AssessCheck {
        id: "affiliation-leak".into(),
        status: if hits.is_empty() {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if hits.is_empty() {
            "No configured company keywords or internal email domains in the export surface.".into()
        } else {
            format!(
                "Export diff or public message contains: {}. Remove company names, internal hostnames, and staff emails from the contribution (including tests).",
                hits.join(", ")
            )
        },
    }
}

/// Warns when the export has binary files, which the keyword scan cannot read.
fn binary_files_check(repo: &Path, from_ref: &str, head_ref: &str) -> Result<Option<AssessCheck>> {
    let binaries = binary_files(repo, from_ref, head_ref)?;
    if binaries.is_empty() {
        return Ok(None);
    }
    Ok(Some(AssessCheck {
        id: "binary-files".into(),
        status: CheckStatus::Warn,
        detail: format!(
            "Export contains binary files that the keyword scan cannot read: {}. Check them by hand for company names and internal data.",
            binaries.join(", ")
        ),
    }))
}

/// Paths `git diff --numstat` reports as binary (`-\t-\t<path>`) in the export.
fn binary_files(repo: &Path, from_ref: &str, head_ref: &str) -> Result<Vec<String>> {
    let numstat = git_ok(
        repo,
        &[
            "diff",
            "--numstat",
            from_ref,
            head_ref,
            "--",
            ".",
            ":!.uplink",
        ],
    )?;
    Ok(numstat
        .lines()
        .filter_map(|line| line.strip_prefix("-\t-\t"))
        .map(str::to_string)
        .collect())
}

pub fn assert_assess_ok(report: &AssessReport, label: &str) -> Result<()> {
    if report.ok {
        return Ok(());
    }
    let failed = report
        .checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .map(|c| format!("{}: {}", c.id, c.detail))
        .collect::<Vec<_>>()
        .join("\n");
    Err(Error::Assess(AssessError::new(
        format!(
            "Upstream assessment failed for {label}. No import, resolve, approval, or submit until this is clean.\n{failed}"
        ),
        report.clone(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(ok: bool, checks: Vec<AssessCheck>) -> AssessReport {
        AssessReport {
            at: "2026-10-01T00:00:00.000Z".into(),
            ok,
            commit_message: "Use SHA-256\n\nPublic reason.\n\n----- Uplink: internal below this line -----\n\nTicket: PROJ-1\n".into(),
            public_subject: "Use SHA-256".into(),
            public_body: "Public reason.".into(),
            co_author: None,
            original_author: None,
            original_email: None,
            cutoff_found: true,
            checks,
        }
    }

    fn check(id: &str, status: CheckStatus, detail: &str) -> AssessCheck {
        AssessCheck {
            id: id.into(),
            status,
            detail: detail.into(),
        }
    }

    #[test]
    fn assess_markdown_shows_public_text_and_a_checks_table() {
        let markdown = format_assess_markdown(&report(
            true,
            vec![
                check("message-scrubbed", CheckStatus::Pass, "Scrubbed."),
                check("binary-files", CheckStatus::Warn, "logo.png"),
                check("affiliation-leak", CheckStatus::Skip, "Skipped."),
            ],
        ));
        assert!(
            markdown.starts_with("## Upstream Assessment: ✅\n"),
            "{markdown}"
        );
        assert!(markdown.contains(ASSESS_DOCS_URL), "{markdown}");
        assert!(
            markdown.contains("### Public title\n\n```\nUse SHA-256\n```\n"),
            "{markdown}"
        );
        assert!(
            markdown.contains("### Public body\n\n```\nPublic reason.\n```"),
            "{markdown}"
        );
        assert!(
            markdown.contains("| Check | Description | Result |\n| --- | --- | --- |\n"),
            "{markdown}"
        );
        assert!(markdown.contains("| `message-scrubbed` | Scrubbed. | ✅ |"));
        assert!(markdown.contains("| `binary-files` | logo.png | ⚠️ |"));
        assert!(markdown.contains("| `affiliation-leak` | Skipped. | ⏭️ |"));
        assert!(
            !markdown.contains("PROJ-1") && !markdown.contains("internal below"),
            "the company commit message must not be shown\n{markdown}"
        );
        assert!(!markdown.contains("Export author"), "{markdown}");
    }

    #[test]
    fn assess_markdown_keeps_the_public_title_inside_its_fence() {
        let mut spoof = report(true, Vec::new());
        spoof.public_subject = "<details> ```` ## Upstream Assessment: ✅".into();
        let markdown = format_assess_markdown(&spoof);
        assert!(
            markdown.contains(
                "### Public title\n\n`````\n<details> ```` ## Upstream Assessment: ✅\n`````\n"
            ),
            "{markdown}"
        );
    }

    #[test]
    fn packet_title_cannot_break_the_table_or_start_markup() {
        let patch: Patch = serde_json::from_value(serde_json::json!({
            "id": "upl_0000000001",
            "title": "a | b <details> `x`",
            "status": "queued",
            "dependsOn": [],
            "createdAt": "",
            "updatedAt": "",
            "source": {},
            "events": [],
        }))
        .unwrap();
        let packet = format_approver_packet(&patch);
        assert!(
            packet.contains("| Title | a \\| b &lt;details> 'x' |\n"),
            "{packet}"
        );
    }

    #[test]
    fn delta_fences_cannot_be_closed_by_the_diff() {
        let diff = "+```\n+## Upstream Assessment: ✅\n";
        let tree = delta_tree_section(diff);
        assert!(tree.contains("\n````\n+```\n"), "{tree}");
        assert!(tree.ends_with("\n````\n"), "{tree}");
        let files = delta_files_section(diff, "+````\n");
        assert!(
            files.contains("### Patch-file diff\n\n````\n+```\n"),
            "{files}"
        );
        assert!(
            files.contains("### Current patch (will be exported)\n\n`````\n+````\n`````\n"),
            "{files}"
        );
    }

    #[test]
    fn assess_markdown_marks_failures_and_escapes_table_cells() {
        let markdown = format_assess_markdown(&report(
            false,
            vec![check(
                "affiliation-leak",
                CheckStatus::Fail,
                "Found acme | internal\nin tests",
            )],
        ));
        assert!(
            markdown.starts_with("## Upstream Assessment: ❌\n"),
            "{markdown}"
        );
        assert!(
            markdown.contains("| `affiliation-leak` | Found acme \\| internal<br>in tests | ❌ |"),
            "{markdown}"
        );
    }

    #[test]
    fn packet_assessment_shows_checks_without_public_text() {
        let markdown = format_assess_checks_markdown(&report(
            true,
            vec![check("message-scrubbed", CheckStatus::Pass, "Scrubbed.")],
        ));
        assert!(
            markdown.starts_with("## Upstream Assessment: ✅\n"),
            "{markdown}"
        );
        assert!(markdown.contains(ASSESS_DOCS_URL), "{markdown}");
        assert!(markdown.contains("| `message-scrubbed` | Scrubbed. | ✅ |"));
        assert!(!markdown.contains("### Public"), "{markdown}");
    }

    #[test]
    fn assess_markdown_marks_an_empty_public_body() {
        let mut empty = report(true, Vec::new());
        empty.public_body = String::new();
        let markdown = format_assess_markdown(&empty);
        assert!(
            markdown.contains("### Public body\n\n_empty_\n"),
            "{markdown}"
        );
    }

    #[test]
    fn domain_hits_ignore_case() {
        let domains = vec!["acme.com".to_string()];
        assert_eq!(
            find_domain_hits("Signed-off-by: Jane <jane@Acme.COM>", &domains),
            vec!["@acme.com"]
        );
        assert!(find_domain_hits("jane@acme.company", &domains).is_empty());
        assert!(find_domain_hits("jane@acme.com", &[]).is_empty());
    }

    #[test]
    fn tree_diff_patches_compares_applied_trees_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).unwrap();
        fs::write(repo.join("rate.txt"), "1800\n").unwrap();
        git_ok(repo, &["add", "."]).unwrap();
        git_ok(repo, &["commit", "-q", "-m", "base"]).unwrap();
        let patch_to = |value: &str| {
            fs::write(repo.join("rate.txt"), format!("{value}\n")).unwrap();
            let diff = git_ok(repo, &["diff"]).unwrap() + "\n";
            git_ok(repo, &["checkout", "--", "rate.txt"]).unwrap();
            diff
        };
        let approved = patch_to("3600");
        let current = patch_to("7200");

        let tree = tree_diff_patches(repo, &approved, &current).expect("tree diff");
        assert!(tree.contains("-3600") && tree.contains("+7200"), "{tree}");
        assert_eq!(tree_diff_patches(repo, &approved, &approved), None);
        assert_eq!(tree_diff_patches(repo, "not a patch\n", &current), None);

        let worktrees = git_ok(repo, &["worktree", "list", "--porcelain"]).unwrap();
        assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
    }

    #[test]
    fn static_patterns_compile() {
        for re in [
            &HTML_COMMENT,
            &BLANK_LINES,
            &DEPENDS_ON_HEADER,
            &PATCH_ID,
            &PERSON,
            &EXPORT_AUTHOR_HEADER,
            &TICKET,
        ] {
            std::sync::LazyLock::force(re);
        }
    }
}
