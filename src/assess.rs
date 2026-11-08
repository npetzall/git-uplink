use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{AssessError, Error, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::ops::IncomingClaim;
use crate::queue::now_iso;
use crate::repo::{TempWorktree, ensure_revs, has_ref, show_at};
use crate::types::{
    AssessCheck, AssessReport, CheckStatus, DEFAULT_CUTOFF, MergeVia, PATCH_DIR, Patch,
    PatchApproval, PatchExtras, PatchIntent, PatchStatus, QueueState, STATE_BRANCH,
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
static CO_AUTHOR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^Co-Authored-By:\s*(.+)$").expect("co-author trailer regex")
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

/// The first line of `message` and the rest, both trimmed: the subject and
/// body of a commit or pull request.
pub fn split_subject_body(message: &str) -> (String, String) {
    let message = message.trim();
    match message.split_once('\n') {
        Some((subject, body)) => (subject.trim().to_string(), body.trim().to_string()),
        None => (message.to_string(), String::new()),
    }
}

/// The subject of the internal `message`: its first line once HTML comments
/// are stripped. Fails when there is none, or the message starts with the
/// cutoff, so nothing would be public.
pub fn message_subject(message: &str, marker: &str) -> Result<String> {
    let internal = strip_html_comments(message);
    let subject = crate::types::first_line(&internal);
    if subject.is_empty() || subject.contains(marker) {
        return Err(Error::msg(
            "The message needs a subject line before the internal cutoff",
        ));
    }
    Ok(subject.to_string())
}

/// The banned words in `haystack`, ignoring case and matched anywhere.
fn find_banned_words(haystack: &str, words: &[String]) -> Vec<String> {
    let lower = haystack.to_lowercase();
    let mut hits = Vec::new();
    for word in words {
        if word.is_empty() {
            continue;
        }
        if lower.contains(&word.to_lowercase()) && !hits.contains(word) {
            hits.push(word.clone());
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

/// The assessed public subject and body of `patch`: what may leave the
/// company. Never the internal commit message.
pub fn public_subject_and_body(patch: &Patch) -> (String, String) {
    match &patch.assess {
        Some(assess) if !assess.public_message.trim().is_empty() => {
            split_subject_body(&assess.public_message)
        }
        // Without a report no text was scanned and the cutoff is not known,
        // so only the subject is exported.
        _ => (patch.subject().to_string(), String::new()),
    }
}

/// The message of `patch` on company main: the stored message, the
/// `Uplink-Patch-Id` trailer, and a `Co-Authored-By` trailer per internal
/// author. The authors go below the cutoff, so they are internal: the cutoff
/// is added when the stored message has none. An author the stored message
/// already names as co-author is not repeated.
pub fn company_commit_message(queue: &QueueState, patch: &Patch) -> String {
    let stored = patch.commit_message.trim().to_string();
    let authors: Vec<&String> = patch
        .source
        .authors
        .iter()
        .filter(|author| !names_co_author(&stored, author))
        .collect();
    if authors.is_empty() {
        return with_trailers(&stored, patch);
    }
    let marker = cutoff_marker(queue);
    let base = if strip_html_comments(&stored).contains(&marker) {
        stored
    } else {
        format!("{}\n\n{marker}", stored.trim_end())
    };
    let mut message = with_trailers(&base, patch);
    for author in authors {
        message.push_str(&format!("Co-Authored-By: {author}\n"));
    }
    message
}

/// The email of a `Name <email>` person, lowercased.
fn person_email(person: &str) -> Option<String> {
    parse_person(Some(person)).map(|(_, email)| email.to_ascii_lowercase())
}

/// True when `message` has a `Co-Authored-By` line (any case) for the email of `author`.
fn names_co_author(message: &str, author: &str) -> bool {
    let Some(email) = person_email(author) else {
        return false;
    };
    CO_AUTHOR_LINE
        .captures_iter(message)
        .filter_map(|caps| person_email(caps[1].trim()))
        .any(|found| found == email)
}

/// Adds the people in `new` that `authors` does not have yet (by email), in order.
pub fn merge_authors(authors: &mut Vec<String>, new: impl IntoIterator<Item = String>) {
    for person in new {
        let Some(email) = person_email(&person) else {
            continue;
        };
        if !authors
            .iter()
            .any(|known| person_email(known).as_deref() == Some(email.as_str()))
        {
            authors.push(person);
        }
    }
}

/// True for an identity that is not a person: the uplink bot, or a forge bot.
fn is_bot(name: &str, email: &str) -> bool {
    email.eq_ignore_ascii_case(crate::git::BOT_EMAIL)
        || name.ends_with("[bot]")
        || email.to_ascii_lowercase().contains("[bot]@")
}

/// The people who wrote `from..head`: the authors of its non-merge commits
/// and the `Co-authored-by` trailers in them, as `Name <email>`, each once.
pub fn patch_authors(repo: &Path, from: &str, head: &str) -> Result<Vec<String>> {
    let range = format!("{from}..{head}");
    let log = git_ok(
        repo,
        &[
            "log",
            "--no-merges",
            "--reverse",
            "--format=%an <%ae>%x00%(trailers:key=Co-authored-by,valueonly,separator=%x00)%x1e",
            &range,
        ],
    )?;
    let mut people = Vec::new();
    for record in log.split('\u{1e}') {
        for value in record.split('\u{0}') {
            if let Some((name, email)) = parse_person(Some(value.trim()))
                && !is_bot(&name, &email)
            {
                people.push(format!("{name} <{email}>"));
            }
        }
    }
    let mut authors = Vec::new();
    merge_authors(&mut authors, people);
    Ok(authors)
}

/// The authors of a resolution or amend on `onto..HEAD`. When it is only in
/// the index or work tree (no commit of a person yet), the operator's own
/// `user.name` and `user.email`.
pub fn resolution_authors(repo: &Path, onto: &str) -> Result<Vec<String>> {
    let mut authors = patch_authors(repo, onto, "HEAD")?;
    if authors.is_empty() {
        let pending = git_ok(
            repo,
            &["status", "--porcelain", "--", ".", ":(exclude).uplink"],
        )?;
        if !pending.trim().is_empty()
            && let Some((name, email)) = crate::git::operator_identity(repo)
            && !is_bot(&name, &email)
        {
            authors.push(format!("{name} <{email}>"));
        }
    }
    Ok(authors)
}

/// What `patch` (format-patch text) changes, file by file: each file's
/// `diff --git` line, what happens to the file as a whole (created, deleted,
/// renamed, mode, the new blob of a binary file), and the lines it adds and
/// removes. The unchanged lines around them and their position are left
/// out, so the text stays the same when the patch is replayed onto a moved
/// upstream, or a conflict is resolved without changing what the patch adds
/// and removes.
pub fn changed_lines(patch: &str) -> String {
    const FILE_CHANGES: [&str; 5] = [
        "new file mode ",
        "deleted file mode ",
        "new mode ",
        "rename to ",
        "copy to ",
    ];
    enum At {
        Message,
        Header,
        Hunk { old: usize, new: usize },
        Binary,
    }
    let mut out = String::new();
    let mut at = At::Message;
    let mut new_blob = String::new();
    // Whether the last hunk line was a changed one, for `\ No newline`.
    let mut changed = false;
    for line in patch.lines() {
        // Hunk lines start with ' ', '-', '+' or '\', so this is never content.
        if line.starts_with("diff --git ") {
            out.push_str(line);
            out.push('\n');
            at = At::Header;
            new_blob.clear();
            continue;
        }
        if let At::Hunk { old, new } = &mut at {
            match line.as_bytes().first() {
                Some(b'+') if *new > 0 => {
                    *new -= 1;
                    changed = true;
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
                Some(b'-') if *old > 0 => {
                    *old -= 1;
                    changed = true;
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
                Some(b' ') | None if *old > 0 && *new > 0 => {
                    *old -= 1;
                    *new -= 1;
                    changed = false;
                    continue;
                }
                Some(b'\\') => {
                    if changed {
                        out.push_str(line);
                        out.push('\n');
                    }
                    continue;
                }
                _ => at = At::Header,
            }
        }
        match at {
            At::Message | At::Binary | At::Hunk { .. } => {}
            At::Header => {
                if let Some(counts) = hunk_counts(line) {
                    at = At::Hunk {
                        old: counts.0,
                        new: counts.1,
                    };
                    changed = false;
                } else if let Some(index) = line.strip_prefix("index ") {
                    let ids = index.split(' ').next().unwrap_or_default();
                    new_blob = ids.split("..").nth(1).unwrap_or_default().to_string();
                } else if line == "GIT binary patch" || line.starts_with("Binary files ") {
                    out.push_str(&format!("binary {new_blob}\n"));
                    at = At::Binary;
                } else if FILE_CHANGES.iter().any(|change| line.starts_with(change)) {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Line counts of a `@@ -start,old +start,new @@` hunk header.
fn hunk_counts(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.strip_prefix("@@ -")?.split(' ');
    let count = |range: &str| match range.split_once(',') {
        Some((_, count)) => count.parse().ok(),
        None => Some(1),
    };
    let old = count(parts.next()?)?;
    let new = count(parts.next()?.strip_prefix('+')?)?;
    Some((old, new))
}

/// The patch file of `id`: the checked-out copy, or the one on `uplink/state`.
pub fn patch_text(repo: &Path, id: &str) -> Result<String> {
    let path = format!("{PATCH_DIR}/{id}.patch");
    match fs::read_to_string(repo.join(&path)) {
        Ok(text) => Ok(text),
        Err(_) => show_at(repo, STATE_BRANCH, &path),
    }
}

fn hash_text(repo: &Path, content: &str) -> Result<String> {
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

/// Identifies what leaves the company for `patch`: the lines it adds and
/// removes ([`changed_lines`]), the patch subject, and the public commit
/// message.
/// An approval records the token it was given for, and `submit` exports only
/// content with that approval.
pub fn review_token(repo: &Path, patch: &Patch) -> Result<String> {
    review_token_for(repo, patch, &patch_text(repo, &patch.id)?)
}

/// [`review_token`] for `patch` with the patch file `text`, for example as
/// read from a git ref.
pub fn review_token_for(repo: &Path, patch: &Patch, text: &str) -> Result<String> {
    let changes = changed_lines(text);
    hash_text(
        repo,
        &format!(
            "{changes}\n{}\n{}",
            patch.subject(),
            export_commit_message(patch)
        ),
    )
}

/// The review token as it was before it was made from [`changed_lines`]:
/// the stable patch id in its place. Approvals recorded with it still count.
fn stable_review_token(repo: &Path, patch: &Patch) -> Option<String> {
    let stable = patch.patch_id_stable.as_deref()?;
    hash_text(
        repo,
        &format!(
            "{stable}\n{}\n{}",
            patch.subject(),
            export_commit_message(patch)
        ),
    )
    .ok()
}

/// True when `approval` was given for the content `token` identifies.
/// Approvals from before tokens existed compare the stable patch id.
pub fn approval_covers(repo: &Path, approval: &PatchApproval, patch: &Patch, token: &str) -> bool {
    match approval.reviewed.as_deref() {
        Some(reviewed) => {
            reviewed == token || stable_review_token(repo, patch).as_deref() == Some(reviewed)
        }
        None => {
            approval.patch_id_stable.is_some() && approval.patch_id_stable == patch.patch_id_stable
        }
    }
}

/// The newest approval of `patch`, when it covers the patch as it is now.
pub fn covering_approval<'a>(repo: &Path, patch: &'a Patch) -> Option<&'a PatchApproval> {
    covering_approval_for(repo, patch, &patch_text(repo, &patch.id).ok()?)
}

/// [`covering_approval`] for `patch` with the patch file `text`.
pub fn covering_approval_for<'a>(
    repo: &Path,
    patch: &'a Patch,
    text: &str,
) -> Option<&'a PatchApproval> {
    let token = review_token_for(repo, patch, text).ok()?;
    patch
        .last_approval()
        .filter(|approval| approval_covers(repo, approval, patch, &token))
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

/// The upstream PR body: the assessed public body, then a line naming Uplink.
/// The `Uplink-Patch-Id` trailer stays on the exported commit only.
pub fn upstream_pr_body(patch: &Patch) -> String {
    const FOOTER: &str =
        "Contribution exported by [Uplink](https://npetzall.github.io/git-uplink/).\n";
    let (_, body) = public_subject_and_body(patch);
    if body.is_empty() {
        FOOTER.to_string()
    } else {
        format!("{body}\n\n{FOOTER}")
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
pub const ASSESS_DOCS_URL: &str = "https://npetzall.github.io/git-uplink/developer#assess-fails";

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
    let (subject, body) = split_subject_body(&report.public_message);
    let body = if body.is_empty() {
        "_empty_".to_string()
    } else {
        format_fenced(&body)
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
        title = format_fenced(&subject),
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
        .map(format_ip_review)
        .unwrap_or_else(|| {
            format!(
                "{}\n\nNo assess report stored. Run `git uplink assess` on the internal PR first.\n",
                ip_review_heading(false)
            )
        })
}

fn ip_review_heading(ok: bool) -> String {
    format!("## IP review: {}", if ok { "✅" } else { "❌" })
}

/// The assessment as an IP reviewer needs it: whether the export names the
/// company, what the scan could not read, and who is credited. Checks about
/// how the message was written show only when they block the export, or
/// when the public message names tickets.
pub fn format_ip_review(report: &AssessReport) -> String {
    let mut items = Vec::new();
    for check in &report.checks {
        let detail = plain_cell(&check.detail);
        match (check.id.as_str(), check.status) {
            ("affiliation-leak", CheckStatus::Pass) => items.push(
                "- ✅ **Banned words:** none in the title, message, file paths or added lines."
                    .to_string(),
            ),
            ("affiliation-leak", status) => items.push(format!(
                "- {} **Banned words:** {detail}",
                check_emoji(status)
            )),
            ("binary-files", _) => items.push(format!("- ⚠️ **Binary files:** {detail}")),
            ("cutoff-used", CheckStatus::Warn) => {
                items.push(format!("- ⚠️ **Tickets in the public message:** {detail}"))
            }
            (id, CheckStatus::Fail) => items.push(format!("- ❌ **Blocking, `{id}`:** {detail}")),
            _ => {}
        }
    }
    if let Some(co_author) = report.co_author.as_deref().filter(|c| !c.trim().is_empty()) {
        items.push(format!("- **Co-author:** {}", plain_cell(co_author)));
    }
    format!(
        "{heading}\n\n\
[What this checks and how to fix a failure]({ASSESS_DOCS_URL})\n\n\
{items}\n",
        heading = ip_review_heading(report.ok),
        items = items.join("\n"),
    )
}

/// `text` safe inside HTML: no tags, no entities of its own.
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The internal and the upstream commit message side by side. Each cell is
/// one line (`<br>` for line breaks), so a blank line in a message cannot end
/// the HTML block.
fn format_message_table(patch: &Patch) -> String {
    let cell = |message: &str| html_escape(message.trim_end()).replace('\n', "<br>");
    format!(
        "## Commit message\n\n\
<table>\n\
<tr><th>Internal</th><th>Upstream</th></tr>\n\
<tr valign=\"top\"><td><pre>{internal}</pre></td><td><pre>{upstream}</pre></td></tr>\n\
</table>\n",
        internal = cell(&patch.commit_message),
        upstream = cell(&export_commit_message(patch)),
    )
}

/// The patch id, linked to the patch file at the `uplink/state` commit it
/// was last written in, when the run knows its GitHub repository.
fn patch_link(repo: &Path, id: &str) -> String {
    patch_link_with(
        repo,
        id,
        env::var("GITHUB_SERVER_URL").ok().as_deref(),
        env::var("GITHUB_REPOSITORY").ok().as_deref(),
    )
}

fn patch_link_with(
    repo: &Path,
    id: &str,
    server: Option<&str>,
    repository: Option<&str>,
) -> String {
    let plain = format!("`{id}`");
    let (Some(server), Some(repository)) = (
        server.filter(|s| !s.is_empty()),
        repository.filter(|r| !r.is_empty()),
    ) else {
        return plain;
    };
    let path = format!("{PATCH_DIR}/{id}.patch");
    let sha = git_ok(
        repo,
        &["log", "-1", "--format=%H", STATE_BRANCH, "--", &path],
    )
    .unwrap_or_default();
    if sha.is_empty() {
        return plain;
    }
    format!(
        "[`{id}`]({}/{repository}/blob/{sha}/{path})",
        server.trim_end_matches('/')
    )
}

/// Added lines past this many are left out of the packet, which also goes
/// to the run summary.
const ADDITIONS_LIMIT: usize = 2000;

/// One file of a diff, as far as what it adds.
#[derive(Debug, Default, PartialEq, Eq)]
struct AddedFile {
    path: String,
    note: Option<String>,
    binary: bool,
    removes: bool,
    /// Runs of added lines: the new line number of the first, and the lines.
    blocks: Vec<(usize, Vec<String>)>,
}

/// What `diff` (git diff or format-patch text) adds, file by file, with the
/// new line numbers. Removed and context lines are only counted.
fn parse_additions(diff: &str) -> Vec<AddedFile> {
    let mut files: Vec<AddedFile> = Vec::new();
    // Old and new lines left in the current hunk.
    let mut left = (0usize, 0usize);
    let mut line_no = 0usize;
    let mut last_added: Option<usize> = None;
    for line in diff.lines() {
        if left != (0, 0)
            && let Some(file) = files.last_mut()
        {
            match line.as_bytes().first() {
                Some(b'+') if left.1 > 0 => {
                    let text = line[1..].to_string();
                    let follows = last_added.is_some_and(|n| n + 1 == line_no);
                    match file.blocks.last_mut() {
                        Some((_, lines)) if follows => lines.push(text),
                        _ => file.blocks.push((line_no, vec![text])),
                    }
                    last_added = Some(line_no);
                    line_no += 1;
                    left.1 -= 1;
                    continue;
                }
                Some(b'-') if left.0 > 0 => {
                    file.removes = true;
                    left.0 -= 1;
                    continue;
                }
                Some(b' ') | None if left.0 > 0 && left.1 > 0 => {
                    line_no += 1;
                    left = (left.0 - 1, left.1 - 1);
                    continue;
                }
                Some(b'\\') => continue,
                // Not hunk content: the hunk is over; read it as a header.
                _ => left = (0, 0),
            }
        }
        if let Some(header) = line.strip_prefix("diff --git ") {
            files.push(AddedFile {
                path: delta_path(&format!("diff --git {header}")),
                ..AddedFile::default()
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(counts) = line.strip_prefix("@@ ").and_then(|_| hunk_counts(line)) {
            let start = line
                .split(" +")
                .nth(1)
                .and_then(|range| range.split([',', ' ']).next())
                .and_then(|start| start.parse::<usize>().ok())
                .unwrap_or(1);
            left = counts;
            line_no = start;
            last_added = None;
        } else if line.starts_with("new file mode") {
            file.note = Some("new file".into());
        } else if line.starts_with("deleted file mode") {
            file.removes = true;
            file.note = Some("deleted".into());
        } else if let Some(from) = line.strip_prefix("rename from ") {
            file.note = Some(format!("renamed from `{}`", plain_cell(from)));
        } else if line == "GIT binary patch" || line.starts_with("Binary files ") {
            file.binary = true;
        }
    }
    // A `diff --git` line quoted in a commit message has nothing under it.
    files.retain(|f| !f.blocks.is_empty() || f.binary || f.removes || f.note.is_some());
    files
}

/// What of `diff` leaves the company as new text: the path of each file the
/// export keeps, and the lines it adds. Removed and context lines are
/// upstream's (or company main's) text and are not part of the export.
fn export_surface_lines(diff: &str) -> String {
    let mut out = String::new();
    for file in parse_additions(diff) {
        if file.note.as_deref() != Some("deleted") {
            out.push_str(&file.path);
            out.push('\n');
        }
        for (_, lines) in &file.blocks {
            for line in lines {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// A fenced block of `lang` the content cannot close.
fn fenced_as(lang: &str, text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{}\n{fence}", text.trim_end_matches('\n'))
}

fn format_additions(files: &[AddedFile], note: &str) -> String {
    let mut out = format!("## Upstream additions\n\n{note}\n\n");
    let mut shown = 0usize;
    let mut hidden = 0usize;
    let mut removes_only = Vec::new();
    for file in files {
        if file.blocks.is_empty() && !file.binary {
            removes_only.push(format!("`{}`", plain_cell(&file.path)));
            continue;
        }
        out.push_str(&format!("### `{}`\n\n", plain_cell(&file.path)));
        if let Some(note) = &file.note {
            out.push_str(&format!("_{note}_\n\n"));
        }
        if file.binary {
            out.push_str(
                "⚠️ Binary file: the banned-word scan cannot read it. Check it by hand.\n\n",
            );
        }
        for (start, lines) in &file.blocks {
            if shown >= ADDITIONS_LIMIT {
                hidden += lines.len();
                continue;
            }
            let end = start + lines.len() - 1;
            let range = if end == *start {
                format!("Line {start}")
            } else {
                format!("Lines {start}–{end}")
            };
            let body = lines
                .iter()
                .map(|line| format!("+{line}"))
                .collect::<Vec<_>>()
                .join("\n");
            out.push_str(&format!("{range}\n\n{}\n\n", fenced_as("diff", &body)));
            shown += lines.len();
        }
    }
    if shown == 0 && !files.iter().any(|f| f.binary) {
        out.push_str("_Nothing added._\n\n");
    }
    if hidden > 0 {
        out.push_str(&format!(
            "_{hidden} more added lines are not shown. Open the patch file above for all of them._\n\n"
        ));
    }
    if !removes_only.is_empty() {
        out.push_str(&format!(
            "Only removes lines (not shown): {}.\n\n",
            removes_only.join(", ")
        ));
    }
    out
}

/// What the export adds, with upstream's line numbers: the patch applied
/// onto `uplink/upstream` in a throwaway index. When it does not apply there,
/// the patch file as stored, with its own line numbers.
fn upstream_additions(repo: &Path, patch: &Patch) -> String {
    let text = match patch_text(repo, &patch.id) {
        Ok(text) => text,
        Err(_) => {
            return format!(
                "## Upstream additions\n\nCould not read `.uplink/patches/{}.patch`.\n\n",
                patch.id
            );
        }
    };
    match diff_on_upstream(repo, &text) {
        Some(diff) => format_additions(
            &parse_additions(&diff),
            "What the export adds onto `uplink/upstream`, by file, with upstream's line numbers. Removed lines are not shown.",
        ),
        None => format_additions(
            &parse_additions(&text),
            "> The patch does not apply onto `uplink/upstream` here, so this is the patch file as stored. Line numbers are from company `main`. Removed lines are not shown.",
        ),
    }
}

/// `git diff -U0` of `patch_text` applied onto `uplink/upstream`, without a
/// checkout. None when there is no `uplink/upstream` or it does not apply.
fn diff_on_upstream(repo: &Path, patch_text: &str) -> Option<String> {
    // A fresh clone has it only as origin/uplink/upstream.
    let _ = crate::repo::ensure_upstream_ref(repo);
    if !has_ref(repo, "uplink/upstream").unwrap_or(false) {
        return None;
    }
    let id = uuid::Uuid::new_v4();
    let git_dir = git_ok(repo, &["rev-parse", "--absolute-git-dir"]).ok()?;
    let index = PathBuf::from(&git_dir).join(format!("uplink-packet-index-{id}"));
    let file = PathBuf::from(&git_dir).join(format!("uplink-packet-{id}.patch"));
    let env = vec![(
        "GIT_INDEX_FILE".to_string(),
        index.to_string_lossy().into_owned(),
    )];
    let opts = || GitOpts {
        extra_env: env.clone(),
        allow_fail: true,
        ..GitOpts::default()
    };
    let diff = (|| {
        fs::write(&file, patch_text).ok()?;
        let read = git(repo, &["read-tree", "uplink/upstream"], opts()).ok()?;
        if read.code != 0 {
            return None;
        }
        let applied = git(
            repo,
            &["apply", "--cached", "--3way", file.to_str()?],
            opts(),
        )
        .ok()?;
        if applied.code != 0 {
            return None;
        }
        let diff = git(
            repo,
            &["diff", "--cached", "-U0", "--no-color", "uplink/upstream"],
            opts(),
        )
        .ok()?;
        (diff.code == 0).then_some(diff.stdout)
    })();
    let _ = fs::remove_file(&index);
    let _ = fs::remove_file(&file);
    diff
}

pub fn format_approver_packet(patch: &crate::types::Patch) -> String {
    approver_packet(patch, None, &format!("`{}`", patch.id), "")
}

/// `| Review token | … |`, or nothing when the patch has none yet.
fn review_token_row(token: Option<&str>) -> String {
    token
        .map(|token| format!("| Review token | `{token}` |\n"))
        .unwrap_or_default()
}

/// The approval that covers `patch` as it is now, when the patch is approved
/// or submitted. A run for such a patch records no new approval.
pub fn already_approved<'a>(repo: &Path, patch: &'a Patch) -> Option<&'a PatchApproval> {
    if !matches!(patch.status, PatchStatus::Approved | PatchStatus::Submitted) {
        return None;
    }
    covering_approval(repo, patch)
}

/// The packet of a patch an approval already covers: nothing to review, so
/// only what covers it and what the Environment click still does.
fn covered_packet(
    patch: &Patch,
    token: Option<&str>,
    approval: &PatchApproval,
    link: &str,
) -> String {
    let run = approval
        .run_url
        .as_deref()
        .map(|url| format!(" [Approval run]({url}) ·"))
        .unwrap_or_default();
    format!(
        "# Contribution packet — {id}\n\n\
> **Already approved. Nothing new leaves the company.** Approval {version} ({at}) covers this patch as it is now: it adds and removes the same lines under the same subject and commit message.{run} [approval.md](approval.md)\n\n\
| Field | Value |\n\
| --- | --- |\n\
| Patch | {link} |\n\
| Subject | {title} |\n\
{token}\n\
## What happens when you approve the {env} environment\n\n\
No new approval is recorded. The click only releases the credentials that export the patch again onto the current upstream. If the patch changed since this packet, approve stops and nothing is exported.\n",
        id = patch.id,
        version = approval.version,
        at = approval.at,
        title = plain_cell(patch.subject()),
        token = review_token_row(token),
        env = TO_UPSTREAM_ENVIRONMENT,
    )
}

/// The full packet. `link` is the Patch cell, `additions` the
/// "Upstream additions" section (empty without a repository to apply in).
fn approver_packet(patch: &Patch, token: Option<&str>, link: &str, additions: &str) -> String {
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
| Patch | {link} |\n\
| Subject | {title} |\n\
| Depends on | {depends} |\n\
| Internal PR | {pr} |\n\
{token}\n\
{messages}\n\
{additions}\
{assessment}\n\
## What happens when you approve the {env} environment\n\n\
1. GitHub records the environment reviewer (audit log + Deployments).\n\
2. This workflow writes `.uplink/reports/{id}/approval.md` on `uplink/state`.\n\
3. `git uplink approve` then `git uplink submit` run with App credentials that exist **only** on the {env} environment. The approval is for the review token above: if the patch changed since this packet, approve stops and nothing is exported.\n\
4. The workflow opens the public pull request with `POST /repos/{{parent}}/pulls` (`head` is the branch, `head_repo` is `<contrib_owner>/<contrib_repo>`, `maintainer_can_modify` false) and runs `git uplink submitted`. No public PR is opened unless export preflight still passes.\n",
        token = review_token_row(token),
        id = patch.id,
        env = TO_UPSTREAM_ENVIRONMENT,
        title = plain_cell(patch.subject()),
        messages = format_message_table(patch),
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
    let token = review_token(repo, patch).ok();
    let assessed_ok = patch.assess.as_ref().is_some_and(|assess| assess.ok);
    if assessed_ok && let Some(approval) = already_approved(repo, patch) {
        let link = patch_link(repo, &patch.id);
        return Ok(covered_packet(patch, token.as_deref(), approval, &link));
    }
    let packet = if patch.reviews_delta() {
        format_delta_approver_packet(repo, patch)?
    } else {
        approver_packet(
            patch,
            token.as_deref(),
            &patch_link(repo, &patch.id),
            &upstream_additions(repo, patch),
        )
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

/// Stored extras are reused while the patch content is unchanged. A delta
/// review (see [`Patch::reviews_delta`]) always gets a fresh assessment.
pub fn stored_extras_fresh(patch: &Patch) -> bool {
    !patch.reviews_delta()
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
| Patch | {link} |\n\
| Subject | {title} |\n\
| Amendment | {amendment} |\n\
| Public PR | {pr} |\n\
| Last approved at | {last_at} |\n\
| Last approved commit | `{last_sha}` |\n\
| Last approval run | {last_run} |\n\
{token}\n\
## Delta since last approval\n\n\
{delta}\n\n\
{messages}\n\
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
        title = plain_cell(patch.subject()),
        link = patch_link(repo, &patch.id),
        messages = format_message_table(patch),
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
    let added = delta_added_section(repo, &old_patch, &new_patch);
    if let Some(tree) = tree_diff_patches(repo, &old_patch, &new_patch) {
        return Ok(format!("{added}\n{}", delta_tree_section(&tree)));
    }
    let file_diff = patch_file_diff(repo, &old_patch, &new_patch);
    Ok(format!(
        "{added}\n{}",
        delta_files_section(&file_diff, &new_patch)
    ))
}

/// What the current patch changes compared with the approved one. Unlike the
/// tree diff this needs no common base, so it also reads well after a
/// conflict, when the approved patch no longer applies.
fn delta_added_section(repo: &Path, old_patch: &str, new_patch: &str) -> String {
    let diff = text_diff(
        repo,
        ("approved.txt", &changed_lines(old_patch)),
        ("current.txt", &changed_lines(new_patch)),
        None,
    );
    format!(
        "### Changed lines: approved vs current\n\n\
The lines the patch adds and removes, file by file, compared with what was last approved. The unchanged lines around them are not listed.\n\n\
{}\n",
        format_fenced(
            diff.as_deref()
                .unwrap_or("(the patch adds and removes the same lines as approved)")
        )
    )
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
    text_diff(
        repo,
        ("approved.patch", old_patch),
        ("current.patch", new_patch),
        None,
    )
    .unwrap_or_else(|| "(no textual difference in patch files)".into())
}

/// `git diff --no-index` of two named texts, or None when they do not differ.
/// `git diff --no-index` of two texts. `context` is the number of unchanged
/// lines around each change, git's default when `None`.
fn text_diff(
    repo: &Path,
    old: (&str, &str),
    new: (&str, &str),
    context: Option<usize>,
) -> Option<String> {
    let dir = RemoveOnDrop(env::temp_dir().join(format!("uplink-delta-{}", uuid::Uuid::new_v4())));
    fs::create_dir_all(&dir.0).ok()?;
    let old_file = dir.0.join(old.0);
    let new_file = dir.0.join(new.0);
    fs::write(&old_file, old.1).ok()?;
    fs::write(&new_file, new.1).ok()?;
    let unified = context.map(|lines| format!("--unified={lines}"));
    let mut args = vec!["diff", "--no-index"];
    args.extend(unified.as_deref());
    args.extend(["--", old_file.to_str()?, new_file.to_str()?]);
    let result = git(repo, &args, GitOpts::allow_fail()).ok()?;
    Some(result.stdout).filter(|diff| !diff.trim().is_empty())
}

/// How one file of a patch differs between two revisions of that patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeltaState {
    /// Only the newer revision touches the file.
    Added,
    /// Only the older revision touches the file.
    Removed,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDelta {
    pub path: String,
    /// The file's `diff --git` line.
    pub header: String,
    pub state: DeltaState,
    /// For `Changed`: the file's changed lines in both revisions, each
    /// prefixed with ' ' (in both), '-' (older only) or '+' (newer only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// For `Added` and `Removed`: the file's changed lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<String>,
}

/// [`changed_lines`] of `patch`, split per file into the `diff --git` line
/// and the lines under it. A `diff --git` line quoted in the commit message
/// has nothing under it and is left out.
pub fn changed_lines_by_file(patch: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    for line in changed_lines(patch).lines() {
        if line.starts_with("diff --git ") {
            files.push((line.to_string(), String::new()));
        } else if let Some((_, lines)) = files.last_mut() {
            lines.push_str(line);
            lines.push('\n');
        }
    }
    files.retain(|(_, lines)| !lines.is_empty());
    files
}

/// The path in a `diff --git a/old b/new` line, the new one when renamed.
fn delta_path(header: &str) -> String {
    let names = header.strip_prefix("diff --git ").unwrap_or(header);
    names
        .rsplit_once(" b/")
        .map_or(names, |(_, path)| path)
        .to_string()
}

/// What changed between two revisions of a patch, file by file, comparing
/// the lines each revision adds and removes ([`changed_lines`]). Files are
/// compared one at a time, so a file one revision dropped is never matched
/// against one the other picked up. Files come in the newer revision's
/// order, then the ones only the older revision has.
pub fn revision_delta(repo: &Path, old_patch: &str, new_patch: &str) -> Result<Vec<FileDelta>> {
    let old = changed_lines_by_file(old_patch);
    let new = changed_lines_by_file(new_patch);
    let delta = |header: &str, state, diff, lines| FileDelta {
        path: delta_path(header),
        header: header.to_string(),
        state,
        diff,
        lines,
    };
    let mut files = Vec::new();
    for (header, lines) in &new {
        let before = old.iter().find(|(other, _)| other == header);
        files.push(match before {
            None => delta(header, DeltaState::Added, None, Some(lines.clone())),
            Some((_, before)) if before == lines => {
                delta(header, DeltaState::Unchanged, None, None)
            }
            Some((_, before)) => {
                let context = before.lines().count() + lines.lines().count();
                let diff = text_diff(
                    repo,
                    ("older.txt", before),
                    ("newer.txt", lines),
                    Some(context),
                )
                .ok_or_else(|| Error::msg(format!("could not compare {}", delta_path(header))))?;
                delta(header, DeltaState::Changed, Some(hunk_body(&diff)), None)
            }
        });
    }
    for (header, lines) in &old {
        if !new.iter().any(|(other, _)| other == header) {
            files.push(delta(
                header,
                DeltaState::Removed,
                None,
                Some(lines.clone()),
            ));
        }
    }
    Ok(files)
}

/// The lines of a one-hunk diff, without the file header and the `@@` line.
fn hunk_body(diff: &str) -> String {
    let mut body = String::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if in_hunk {
            body.push_str(line);
            body.push('\n');
        } else if line.starts_with("@@ ") {
            in_hunk = true;
        }
    }
    body
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
                    "- `{sha}` carries `{key}: {id}`, but its diff is not that patch. Approving does **not** mark `{id}` merged; it is applied again on the new upstream. If upstream did take it in another form, ask the operator to run `git uplink merged {id} --sha {sha}`.\n",
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
    intent: PatchIntent,
) -> Result<AssessReport> {
    message_subject(message, &cutoff_marker(queue))?;
    let shas = ensure_revs(repo, &[from_ref, head_ref])?;
    let from_ref = shas[0].as_str();
    let head_ref = shas[1].as_str();
    let export = if intent.is_internal_only() {
        None
    } else {
        Some(ExportDiff {
            diff: git_ok(
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
            )?,
            binaries: binary_files(repo, from_ref, head_ref)?,
        })
    };
    Ok(assess_export(
        queue,
        message,
        head_author(repo, head_ref)?,
        export,
    ))
}

/// Assesses an upstream-bound `patch` from its patch file `patch_text` as it
/// is in the queue. Nothing is applied or built, so it needs no
/// `uplink/upstream` and says nothing about whether the patch still applies
/// (preflight does). Message and author are the ones stored with the
/// patch; the settings are today's. A patch in the queue can also be judged
/// by what it depends on, so this adds the `dependencies` check.
pub fn assess_patch_file(
    queue: &QueueState,
    patch: &Patch,
    patch_text: &str,
    intent: PatchIntent,
) -> AssessReport {
    let author = patch
        .assess
        .as_ref()
        .map(|stored| {
            (
                stored.original_author.clone(),
                stored.original_email.clone(),
            )
        })
        .unwrap_or_default();
    let mut report = assess_export(
        queue,
        &patch.commit_message,
        author,
        (!intent.is_internal_only()).then(|| export_diff(patch_text)),
    );
    if !intent.is_internal_only() {
        let check = dependencies_check(queue, patch);
        report.ok &= check.status != CheckStatus::Fail;
        report.checks.push(check);
    }
    report
}

/// Fails while an upstream-bound patch that `patch` depends on is not merged
/// upstream: `patch` cannot be reviewed or exported before it. Internal-only
/// dependencies never go upstream and do not count. `approve` and `submit`
/// refuse for the same reason.
fn dependencies_check(queue: &QueueState, patch: &Patch) -> AssessCheck {
    let unmerged: Vec<String> = patch
        .depends_on
        .iter()
        .filter_map(|id| match queue.all_patches().find(|dep| &dep.id == id) {
            None => Some(format!("{id} (not in the queue)")),
            Some(dep) if queue.is_upstream(id) && dep.status != PatchStatus::Merged => {
                Some(format!("{id} ({})", dep.status))
            }
            Some(_) => None,
        })
        .collect();
    let (status, detail) = if patch.depends_on.is_empty() {
        (CheckStatus::Skip, "No Uplink-Depends-On.".to_string())
    } else if unmerged.is_empty() {
        (
            CheckStatus::Pass,
            "Every upstream-bound dependency is merged upstream.".to_string(),
        )
    } else {
        (
            CheckStatus::Fail,
            format!(
                "Depends on patches not merged upstream yet: {}. Contribute {} after they merge.",
                unmerged.join(", "),
                patch.id
            ),
        )
    };
    AssessCheck {
        id: "dependencies".into(),
        status,
        detail,
    }
}

/// Whether the company assessment-hook result stored with a patch is still
/// for its content, so the hook need not run again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageStoredExtras {
    pub fresh: bool,
    /// Where the stored result came from, such as the hook run URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// File names of an assessment package (`assess --package <dir>`).
pub const PACKAGE_JSON: &str = "assessment.json";
pub const PACKAGE_MARKDOWN: &str = "assessment.md";
pub const PACKAGE_CHANGE: &str = "change.patch";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    /// A change that is not a patch yet, such as an open pull request.
    Pr,
    /// A patch in the queue.
    Patch,
}

/// The stored message of a change and what of it is public.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMessage {
    /// The whole internal commit message.
    pub internal: String,
    /// What leaves the company: the subject, a blank line, the body.
    pub public: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub co_author: Option<String>,
}

/// What the assessment scanned for, from `uplink.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageSettings {
    #[serde(default)]
    pub banned_words: Vec<String>,
    /// Why `uplink.toml` could not be used; the scan then fails closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// `assessment.json` of an assessment package: the result of one assessment,
/// what it was made for, and what it was made with. The company assessment
/// hook reads it, and `report --assess-result` stores it with the patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessPackage {
    pub kind: PackageKind,
    /// Id of the assessed patch, for kind `patch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Commit of `uplink/state` the patch was read at, for kind `patch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub at: String,
    pub message: PackageMessage,
    pub ok: bool,
    pub checks: Vec<AssessCheck>,
    pub cutoff_found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_email: Option<String>,
    pub settings: PackageSettings,
    /// Git blob id of `change.patch` in the package.
    pub change_blob: String,
    /// The stored company assessment-hook result, for kind `patch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_extras: Option<PackageStoredExtras>,
    /// The patch as it is in `queue.json`, for kind `patch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_entry: Option<Patch>,
    /// The approval that already covers the patch as it is now, for kind
    /// `patch`. Submit then skips the hook and shows a short packet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered_by: Option<PatchApproval>,
}

impl AssessPackage {
    /// Describes `report`, the assessment of the internal message `internal`
    /// and `change` (the text written as `change.patch`). `patch` is the
    /// queue entry when a patch was assessed.
    pub fn new(
        repo: &Path,
        queue: &QueueState,
        report: &AssessReport,
        internal: &str,
        change: &str,
        patch: Option<&Patch>,
    ) -> Result<Self> {
        let state = match patch {
            Some(_) => git_ok(repo, &["rev-parse", "--verify", "--quiet", STATE_BRANCH]).ok(),
            None => None,
        };
        Ok(Self {
            kind: if patch.is_some() {
                PackageKind::Patch
            } else {
                PackageKind::Pr
            },
            patch: patch.map(|patch| patch.id.clone()),
            state,
            at: report.at.clone(),
            message: PackageMessage {
                internal: internal.to_string(),
                public: report.public_message.clone(),
                co_author: report.co_author.clone(),
            },
            ok: report.ok,
            checks: report.checks.clone(),
            cutoff_found: report.cutoff_found,
            original_author: report.original_author.clone(),
            original_email: report.original_email.clone(),
            settings: PackageSettings {
                banned_words: queue.settings.banned_words.clone(),
                problem: queue.settings.problem.clone(),
            },
            change_blob: hash_text(repo, change)?,
            stored_extras: patch.map(|patch| PackageStoredExtras {
                fresh: stored_extras_fresh(patch),
                source: patch
                    .extras
                    .as_ref()
                    .and_then(|extras| extras.source.clone()),
            }),
            queue_entry: patch.cloned(),
            covered_by: patch.and_then(|patch| already_approved(repo, patch).cloned()),
        })
    }

    /// The assessment this package holds.
    pub fn report(&self) -> AssessReport {
        AssessReport {
            at: self.at.clone(),
            ok: self.ok,
            public_message: self.message.public.clone(),
            co_author: self.message.co_author.clone(),
            original_author: self.original_author.clone(),
            original_email: self.original_email.clone(),
            cutoff_found: self.cutoff_found,
            checks: self.checks.clone(),
        }
    }

    /// Why this package does not describe `patch` (with `patch_text` as its
    /// patch file) assessed now, or None when it does. `fresh` is that
    /// assessment: the same change, message and settings give the
    /// same checks.
    pub fn stale_for(
        &self,
        repo: &Path,
        patch: &Patch,
        patch_text: &str,
        fresh: &AssessReport,
    ) -> Result<Option<&'static str>> {
        let same_checks = self.checks.len() == fresh.checks.len()
            && self
                .checks
                .iter()
                .zip(&fresh.checks)
                .all(|(a, b)| a.id == b.id && a.status == b.status && a.detail == b.detail);
        Ok(if self.patch.as_deref() != Some(patch.id.as_str()) {
            Some("it is for another patch")
        } else if self.change_blob != hash_text(repo, patch_text)? {
            Some("the patch changed since")
        } else if self.message.internal != strip_html_comments(&patch.commit_message)
            || self.message.public != fresh.public_message
        {
            Some("the message changed since")
        } else if self.ok != fresh.ok || !same_checks {
            Some("uplink.toml changed since, or the result was edited")
        } else {
            None
        })
    }
}

/// The export diff between two revisions, as `change.patch` of a package.
pub fn change_between(repo: &Path, from_ref: &str, head_ref: &str) -> Result<String> {
    let shas = ensure_revs(repo, &[from_ref, head_ref])?;
    let diff = git_ok(
        repo,
        &[
            "diff",
            "--full-index",
            "--binary",
            &shas[0],
            &shas[1],
            "--",
            ".",
            ":!.uplink",
        ],
    )?;
    Ok(format!("{diff}\n"))
}

/// The diff that would leave the company, and the binary files in it.
struct ExportDiff {
    diff: String,
    binaries: Vec<String>,
}

/// The diffs of format-patch text. The mail header and the company commit
/// message above them are not exported and are left out, and so is the
/// encoded content of binary files, which the banned-word scan cannot read.
fn export_diff(patch_text: &str) -> ExportDiff {
    let mut diff = String::new();
    let mut binaries = Vec::new();
    let mut path = "";
    let mut in_diffs = false;
    let mut in_binary = false;
    for line in patch_text.lines() {
        if let Some(paths) = line.strip_prefix("diff --git ") {
            path = paths.rsplit_once(" b/").map_or(paths, |(_, new)| new);
            in_diffs = true;
            in_binary = false;
        } else if !in_diffs || in_binary {
            continue;
        } else if line == "GIT binary patch" || line.starts_with("Binary files ") {
            binaries.push(path.to_string());
            in_binary = true;
        }
        diff.push_str(line);
        diff.push('\n');
    }
    ExportDiff { diff, binaries }
}

/// The checks of an assessment. `export` is None for an internal-only
/// change, which is not exported and not scanned.
fn assess_export(
    queue: &QueueState,
    message: &str,
    (original_author, original_email): (Option<String>, Option<String>),
    export: Option<ExportDiff>,
) -> AssessReport {
    let marker = cutoff_marker(queue);
    let internal = strip_html_comments(message);
    let (public_message, internal_text) = public_and_internal(&internal, &marker);
    let co_author = resolve_co_author(&internal_text);

    let mut checks = message_checks(
        &marker,
        &public_message,
        &internal_text,
        co_author.as_ref(),
        (original_author.as_deref(), original_email.as_deref()),
    );
    match export {
        None => checks.push(AssessCheck {
            id: "affiliation-leak".into(),
            status: CheckStatus::Skip,
            detail: "internal-only patches are not exported; banned-word scan skipped.".into(),
        }),
        Some(export) => {
            // The co-author trailer lands in the public commit, so it is scanned too.
            let trailer = co_author
                .as_ref()
                .map(|(name, email)| format!("{name} <{email}>\n"))
                .unwrap_or_default();
            let diff = export_surface_lines(&export.diff);
            let export_surface = format!("{trailer}{public_message}\n{diff}");
            checks.push(affiliation_check(queue, &export_surface));
            checks.extend(binary_files_check(&export.binaries));
        }
    }

    let ok = checks.iter().all(|c| c.status != CheckStatus::Fail);
    let cutoff_found = !internal_text.is_empty() || internal.contains(&marker);
    AssessReport {
        at: now_iso(),
        ok,
        public_message,
        co_author: co_author.map(|(name, email)| format!("{name} <{email}>")),
        original_author,
        original_email,
        cutoff_found,
        checks,
    }
}

/// The public message of the internal `message` (HTML comments already
/// stripped): the text before the cutoff without `Uplink-Depends-On` lines,
/// and the internal text after the cutoff.
fn public_and_internal(message: &str, marker: &str) -> (String, String) {
    let (public_text, internal_text) = split_internal_message(message, marker);
    (strip_depends_on(&public_text), internal_text)
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

/// Scans the export surface for the banned words in `uplink.toml`.
fn affiliation_check(queue: &QueueState, export_surface: &str) -> AssessCheck {
    let settings = match queue.settings.usable() {
        Ok(settings) => settings,
        // Fail closed: without the settings there is nothing to scan for.
        Err(err) => {
            return AssessCheck {
                id: "affiliation-leak".into(),
                status: CheckStatus::Fail,
                detail: format!("Cannot scan for banned words: {err}."),
            };
        }
    };
    if settings.banned_words.is_empty() {
        return AssessCheck {
            id: "affiliation-leak".into(),
            status: CheckStatus::Warn,
            detail: "No banned_words configured. Set them in uplink.toml on uplink/hooks so tests cannot mention the company.".into(),
        };
    }
    let hits = find_banned_words(export_surface, &settings.banned_words);
    AssessCheck {
        id: "affiliation-leak".into(),
        status: if hits.is_empty() {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if hits.is_empty() {
            "No banned words in the title, message, file paths or added lines.".into()
        } else {
            format!(
                "The title, message, a file path or an added line contains banned words: {}. Remove company names, internal hostnames, and staff emails from the contribution (including tests).",
                hits.join(", ")
            )
        },
    }
}

/// Warns when the export has binary files, which the banned-word scan cannot read.
fn binary_files_check(binaries: &[String]) -> Option<AssessCheck> {
    if binaries.is_empty() {
        return None;
    }
    Some(AssessCheck {
        id: "binary-files".into(),
        status: CheckStatus::Warn,
        detail: format!(
            "Export contains binary files that the banned-word scan cannot read: {}. Check them by hand for company names and internal data.",
            binaries.join(", ")
        ),
    })
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
            public_message: "Use SHA-256\n\nPublic reason.".into(),
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

    const TTL_PATCH: &str = "From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: Asha <asha@example.com>
Subject: [PATCH] Extend TTL

diff --git a/README.md b/README.md is quoted in the message
---
 src/tokens.js | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

diff --git a/src/tokens.js b/src/tokens.js
index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644
--- a/src/tokens.js
+++ b/src/tokens.js
@@ -1,3 +1,3 @@
 function ttl() {
-  return 3600;
+  return 7200;
 }
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000000000000000000000000000000000000..3333333333333333333333333333333333333333
GIT binary patch
literal 4
Lc${NkU|;|M00aO5

literal 0
HcmV?d00001

--
2.50.0
";

    fn authored_patch(message: &str, authors: &[&str]) -> Patch {
        let mut patch: Patch = serde_json::from_value(serde_json::json!({
            "id": "upl_0123456789",
            "commitMessage": message,
            "status": "queued",
            "dependsOn": [],
            "createdAt": "2026-10-01T00:00:00.000Z",
            "updatedAt": "2026-10-01T00:00:00.000Z",
            "patchIdStable": null,
            "source": {},
            "events": [],
        }))
        .unwrap();
        patch.source.authors = authors.iter().map(|a| a.to_string()).collect();
        patch
    }

    fn empty_queue() -> QueueState {
        QueueState::empty(crate::types::QueueConfig::default())
    }

    #[test]
    fn only_the_text_before_the_first_cutoff_is_public() {
        let message = format!(
            "Use SHA-256\n\nPublic reason.\n\n{DEFAULT_CUTOFF}\nTicket: PROJ-1\n\n{DEFAULT_CUTOFF}\nSecret: PROJ-2\n"
        );
        let (public, internal) = split_internal_message(&message, DEFAULT_CUTOFF);
        assert_eq!(public, "Use SHA-256\n\nPublic reason.");
        assert!(internal.contains("PROJ-1"), "{internal}");
        assert!(internal.contains(DEFAULT_CUTOFF), "{internal}");
        assert!(internal.contains("PROJ-2"), "{internal}");

        let report = assess_export(&empty_queue(), &message, (None, None), None);
        assert_eq!(report.public_message, "Use SHA-256\n\nPublic reason.");
        let mut patch = authored_patch(&message, &[]);
        patch.assess = Some(report);
        let exported = export_commit_message(&patch);
        assert!(!exported.contains("PROJ"), "{exported}");
        assert!(!exported.contains(DEFAULT_CUTOFF), "{exported}");
    }

    #[test]
    fn the_subject_is_the_first_line_once_comments_are_stripped() {
        assert_eq!(
            message_subject("<!-- template -->\nUse SHA-256\n\nWhy.", DEFAULT_CUTOFF).unwrap(),
            "Use SHA-256"
        );
        assert!(message_subject("  \n", DEFAULT_CUTOFF).is_err());
        assert!(
            message_subject(&format!("{DEFAULT_CUTOFF}\nTicket: PROJ-1"), DEFAULT_CUTOFF).is_err()
        );
    }

    #[test]
    fn public_message_drops_depends_on_and_the_internal_part() {
        let message = format!(
            "Use SHA-256\n\nWhy.\nUplink-Depends-On: upl_0123456789\n\n{DEFAULT_CUTOFF}\nTicket: PROJ-1"
        );
        let report = assess_export(&empty_queue(), &message, (None, None), None);
        assert_eq!(report.public_message, "Use SHA-256\n\nWhy.");
        let mut patch = authored_patch(&message, &[]);
        patch.assess = Some(report);
        assert_eq!(
            public_subject_and_body(&patch),
            ("Use SHA-256".to_string(), "Why.".to_string())
        );
        assert!(upstream_pr_body(&patch).starts_with("Why.\n\n"));
    }

    #[test]
    fn a_cutoff_inside_an_html_comment_does_not_split() {
        let message = format!(
            "Subject\n\n<!-- {DEFAULT_CUTOFF} -->\nStill public.\n\n{DEFAULT_CUTOFF}\nInternal.\n"
        );
        let (public, internal) = split_internal_message(&message, DEFAULT_CUTOFF);
        assert_eq!(public, "Subject\n\nStill public.");
        assert_eq!(internal, "Internal.");
    }

    #[test]
    fn company_message_without_authors_is_unchanged() {
        let patch = authored_patch("Use SHA-256\n\nPublic reason.", &[]);
        assert_eq!(
            company_commit_message(&empty_queue(), &patch),
            "Use SHA-256\n\nPublic reason.\n\nUplink-Patch-Id: upl_0123456789\n"
        );
    }

    #[test]
    fn company_message_adds_the_cutoff_before_internal_authors() {
        let patch = authored_patch(
            "Use SHA-256\n\nPublic reason.",
            &["Asha <asha@corp.example>", "Ben <ben@corp.example>"],
        );
        assert_eq!(
            company_commit_message(&empty_queue(), &patch),
            format!(
                "Use SHA-256\n\nPublic reason.\n\n{DEFAULT_CUTOFF}\n\n\
Uplink-Patch-Id: upl_0123456789\n\
Co-Authored-By: Asha <asha@corp.example>\n\
Co-Authored-By: Ben <ben@corp.example>\n"
            )
        );
    }

    #[test]
    fn company_message_keeps_an_existing_cutoff_and_skips_named_co_authors() {
        let message = format!(
            "Use SHA-256\n\n{DEFAULT_CUTOFF}\nTicket: PROJ-1\nco-authored-by: ASHA <Asha@Corp.Example>"
        );
        let patch = authored_patch(
            &message,
            &["Asha <asha@corp.example>", "Ben <ben@corp.example>"],
        );
        let company = company_commit_message(&empty_queue(), &patch);
        assert_eq!(company.matches(DEFAULT_CUTOFF).count(), 1, "{company}");
        assert!(!company.contains("Co-Authored-By: Asha"), "{company}");
        assert!(
            company.ends_with(
                "Uplink-Patch-Id: upl_0123456789\nCo-Authored-By: Ben <ben@corp.example>\n"
            ),
            "{company}"
        );
        let (public, _) = split_internal_message(&company, DEFAULT_CUTOFF);
        assert!(!public.contains("Ben"), "{public}");
    }

    #[test]
    fn export_message_never_names_internal_authors() {
        let message = "Use SHA-256\n\nPublic reason.";
        let mut patch = authored_patch(message, &["Asha <asha@corp.example>"]);
        patch.assess = Some(assess_export(&empty_queue(), message, (None, None), None));
        let exported = export_commit_message(&patch);
        assert!(!exported.contains("asha"), "{exported}");
    }

    #[test]
    fn merge_authors_adds_each_email_once_and_keeps_order() {
        let mut authors = vec!["Asha <asha@corp.example>".to_string()];
        merge_authors(
            &mut authors,
            [
                "A. Sha <ASHA@corp.example>".to_string(),
                "Ben <ben@corp.example>".to_string(),
                "not a person".to_string(),
                "Ben B <ben@corp.example>".to_string(),
            ],
        );
        assert_eq!(
            authors,
            ["Asha <asha@corp.example>", "Ben <ben@corp.example>"]
        );
    }

    #[test]
    fn bots_are_not_authors() {
        assert!(is_bot("Uplink Bot", crate::git::BOT_EMAIL));
        assert!(is_bot(
            "github-actions[bot]",
            "41898282+github-actions[bot]@users.noreply.github.com"
        ));
        assert!(!is_bot("Asha", "asha@corp.example"));
    }

    #[test]
    fn changed_lines_is_what_the_patch_adds_and_removes() {
        assert_eq!(
            changed_lines(TTL_PATCH),
            "diff --git a/README.md b/README.md is quoted in the message\n\
diff --git a/src/tokens.js b/src/tokens.js\n\
-  return 3600;\n\
+  return 7200;\n\
diff --git a/logo.png b/logo.png\n\
new file mode 100644\n\
binary 3333333333333333333333333333333333333333\n"
        );
    }

    #[test]
    fn changed_lines_ignores_context_and_position() {
        // Replayed after upstream changed the lines around the patch's own.
        let replayed = TTL_PATCH
            .replace(" function ttl() {", " function defaultTtl() {")
            .replace("@@ -1,3 +1,3 @@", "@@ -40,3 +41,3 @@ header")
            .replace("index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222", "index 4444444444444444444444444444444444444444..5555555555555555555555555555555555555555");
        assert_ne!(replayed, TTL_PATCH);
        assert_eq!(changed_lines(&replayed), changed_lines(TTL_PATCH));
    }

    #[test]
    fn changed_lines_changes_with_what_the_patch_adds_or_removes() {
        let base = changed_lines(TTL_PATCH);
        let changes = [
            ("+  return 7200;", "+  return 7200; // AcmeCorp SLA"),
            // Kept its own line over an upstream change of the same line.
            ("-  return 3600;", "-  return 1800;"),
            (
                "+  return 7200;",
                "+  return 7200;\n\\ No newline at end of file",
            ),
            ("b/src/tokens.js", "b/src/token.js"),
            (
                "..3333333333333333333333333333333333333333",
                "..6666666666666666666666666666666666666666",
            ),
            ("new file mode 100644", "new file mode 100755"),
        ];
        for (from, to) in changes {
            let changed = TTL_PATCH.replace(from, to);
            assert_ne!(changed, TTL_PATCH, "{from}");
            assert_ne!(changed_lines(&changed), base, "{from}");
        }
        // A second added line in the same hunk.
        let more = TTL_PATCH
            .replace("@@ -1,3 +1,3 @@", "@@ -1,3 +1,4 @@")
            .replace("+  return 7200;", "+  return 7200;\n+  // extra");
        assert!(changed_lines(&more).contains("+  // extra\n"));
    }

    #[test]
    fn export_diff_is_the_diffs_without_the_mail_and_binary_content() {
        let patch = TTL_PATCH.replace(
            "Subject: [PATCH] Extend TTL\n",
            "Subject: [PATCH] Extend TTL\n\nPublic reason.\n\nTicket: PROJ-1 for AcmeCorp\n",
        );
        let export = export_diff(&patch);
        // The company message and the author's address are not exported.
        assert!(!export.diff.contains("AcmeCorp"), "{}", export.diff);
        assert!(!export.diff.contains("asha@example.com"), "{}", export.diff);
        assert!(export.diff.contains("-  return 3600;\n+  return 7200;\n"));
        assert!(export.diff.contains(" function ttl() {\n"));
        // A binary file is named, and its encoded content is not scanned.
        assert_eq!(export.binaries, ["logo.png"]);
        assert!(export.diff.contains("GIT binary patch\n"));
        assert!(!export.diff.contains("Lc${NkU|;|M00aO5"), "{}", export.diff);
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
        spoof.public_message = "<details> ```` ## Upstream Assessment: ✅".into();
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
            "commitMessage": "a | b <details> `x`",
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
            packet.contains("| Subject | a \\| b &lt;details> 'x' |\n"),
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
        empty.public_message = "Use SHA-256".into();
        let markdown = format_assess_markdown(&empty);
        assert!(
            markdown.contains("### Public body\n\n_empty_\n"),
            "{markdown}"
        );
    }

    #[test]
    fn banned_words_match_anywhere_ignoring_case() {
        let words = vec!["acme".to_string()];
        for text in [
            "Signed-off-by: Jane <jane@Acme.COM>",
            "see https://git.acme.com/x",
            "class AcmeCorpClient",
        ] {
            assert_eq!(find_banned_words(text, &words), ["acme"], "{text}");
        }
        assert!(find_banned_words("jane@example.com", &words).is_empty());
        assert!(find_banned_words("jane@acme.com", &[]).is_empty());
    }

    #[test]
    fn changed_lines_by_file_skips_the_quoted_diff_line() {
        let files = changed_lines_by_file(TTL_PATCH);
        let headers: Vec<&str> = files.iter().map(|(header, _)| header.as_str()).collect();
        assert_eq!(
            headers,
            [
                "diff --git a/src/tokens.js b/src/tokens.js",
                "diff --git a/logo.png b/logo.png"
            ]
        );
        assert_eq!(files[0].1, "-  return 3600;\n+  return 7200;\n");
    }

    /// A patch touching each `(path, body)`; `body` is the text under the `diff --git` line.
    fn patch_of(files: &[(&str, &str)]) -> String {
        let mut patch = String::from("Subject: [PATCH] x\n\n---\n");
        for (path, body) in files {
            patch.push_str(&format!("diff --git a/{path} b/{path}\n{body}"));
        }
        patch
    }

    const KEEP: &str = "--- a/keep.rs\n+++ b/keep.rs\n@@ -1 +1 @@\n-old\n+new\n";
    const SHARED: &str = "@@ -1,3 +1,3 @@\n fn main() {\n-    one();\n+    two();\n }\n";

    #[test]
    fn revision_delta_compares_file_by_file() {
        let dir = tempfile::tempdir().unwrap();
        let older = patch_of(&[
            ("keep.rs", KEEP),
            ("edit.rs", "@@ -1 +1 @@\n-a\n+b\n"),
            ("dropped.rs", SHARED),
        ]);
        let newer = patch_of(&[
            ("keep.rs", KEEP),
            ("edit.rs", "@@ -1 +1,2 @@\n-a\n+b\n+c\n"),
            ("picked.rs", SHARED),
        ]);

        let files = revision_delta(dir.path(), &older, &newer).unwrap();
        let states: Vec<(&str, DeltaState)> = files
            .iter()
            .map(|file| (file.path.as_str(), file.state))
            .collect();
        assert_eq!(
            states,
            [
                ("keep.rs", DeltaState::Unchanged),
                ("edit.rs", DeltaState::Changed),
                ("picked.rs", DeltaState::Added),
                ("dropped.rs", DeltaState::Removed),
            ]
        );
        assert_eq!(files[0].diff, None);
        assert_eq!(files[0].lines, None);
        assert_eq!(files[1].diff.as_deref(), Some(" -a\n +b\n++c\n"));
        // Same lines in the dropped and the picked-up file: each is listed whole.
        let shared = Some("-    one();\n+    two();\n");
        assert_eq!(files[2].lines.as_deref(), shared);
        assert_eq!(files[3].lines.as_deref(), shared);

        let same = revision_delta(dir.path(), &newer, &newer).unwrap();
        assert!(same.iter().all(|file| file.state == DeltaState::Unchanged));
    }

    #[test]
    fn revision_delta_shows_a_file_that_becomes_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let older = patch_of(&[("gone.rs", "@@ -1,2 +1 @@\n one\n-two\n")]);
        let newer = patch_of(&[(
            "gone.rs",
            "deleted file mode 100644\n@@ -1,2 +0,0 @@\n-one\n-two\n",
        )]);

        let files = revision_delta(dir.path(), &older, &newer).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].state, DeltaState::Changed);
        assert_eq!(
            files[0].diff.as_deref(),
            Some("+deleted file mode 100644\n+-one\n -two\n")
        );
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

    #[test]
    fn ip_review_shows_what_leaves_and_hides_how_the_message_was_written() {
        let passed = report(
            true,
            vec![
                check("dependencies", CheckStatus::Skip, "None."),
                check("message-scrubbed", CheckStatus::Pass, "Scrubbed."),
                check("cutoff-used", CheckStatus::Pass, "Cutoff found."),
                check("co-author", CheckStatus::Skip, "No Uplink-Export-Author."),
                check("affiliation-leak", CheckStatus::Pass, "No banned words."),
            ],
        );
        let review = format_ip_review(&passed);
        assert!(review.starts_with("## IP review: ✅\n"), "{review}");
        assert!(
            review.contains(
                "- ✅ **Banned words:** none in the title, message, file paths or added lines."
            ),
            "{review}"
        );
        for hidden in [
            "dependencies",
            "Scrubbed",
            "Cutoff",
            "Co-author",
            "Uplink-Export-Author",
        ] {
            assert!(!review.contains(hidden), "{hidden}\n{review}");
        }

        let mut flagged = report(
            false,
            vec![
                check("dependencies", CheckStatus::Fail, "upl_1 (queued)"),
                check(
                    "cutoff-used",
                    CheckStatus::Warn,
                    "public message contains PROJ-1",
                ),
                check("affiliation-leak", CheckStatus::Fail, "banned words: acme"),
                check("binary-files", CheckStatus::Warn, "logo.png"),
            ],
        );
        flagged.co_author = Some("Jane Doe <jane@example.com>".into());
        let review = format_ip_review(&flagged);
        assert!(review.starts_with("## IP review: ❌\n"), "{review}");
        assert!(
            review.contains("- ❌ **Banned words:** banned words: acme"),
            "{review}"
        );
        assert!(
            review.contains("- ⚠️ **Binary files:** logo.png"),
            "{review}"
        );
        assert!(
            review
                .contains("- ⚠️ **Tickets in the public message:** public message contains PROJ-1"),
            "{review}"
        );
        assert!(
            review.contains("- ❌ **Blocking, `dependencies`:** upl_1 (queued)"),
            "{review}"
        );
        assert!(
            review.contains("- **Co-author:** Jane Doe &lt;jane@example.com>"),
            "{review}"
        );
    }

    #[test]
    fn additions_keep_new_line_numbers_and_drop_removals() {
        let diff = "\
diff --git a/src/a.js b/src/a.js
index 1111111..2222222 100644
--- a/src/a.js
+++ b/src/a.js
@@ -2 +2 @@ fn
-  old();
+  new();
@@ -9,0 +10,2 @@
+one
+```
@@ -20,2 +21,0 @@
-gone
-gone too
diff --git a/src/b.js b/src/b.js
deleted file mode 100644
index 3333333..0000000
--- a/src/b.js
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..4444444
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
diff --git a/logo.png b/logo.png
index 5555555..6666666 100644
Binary files a/logo.png and b/logo.png differ
";
        let files = parse_additions(diff);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/a.js", "src/b.js", "new.txt", "logo.png"]);
        assert_eq!(
            files[0].blocks,
            [
                (2, vec!["  new();".to_string()]),
                (10, vec!["one".to_string(), "```".to_string()]),
            ]
        );
        assert!(files[1].blocks.is_empty() && files[1].removes);
        assert_eq!(files[2].note.as_deref(), Some("new file"));
        assert!(files[3].binary);

        let section = format_additions(&files, "Note.");
        assert!(
            section.contains("Line 2\n\n```diff\n+  new();\n```"),
            "{section}"
        );
        assert!(
            section.contains("Lines 10–11\n\n````diff\n+one\n+```\n````"),
            "{section}"
        );
        assert!(
            !section.contains("old();") && !section.contains("gone"),
            "{section}"
        );
        assert!(
            section.contains("Only removes lines (not shown): `src/b.js`."),
            "{section}"
        );
        assert!(section.contains("_new file_"), "{section}");
        assert!(section.contains("Binary file"), "{section}");
    }

    #[test]
    fn additions_of_a_patch_file_count_context_lines() {
        let patch = "\
From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
Subject: [PATCH] Quote diff --git a/x b/x in the message

diff --git a/x b/x
---
 src/a.js | 2 +-
diff --git a/src/a.js b/src/a.js
--- a/src/a.js
+++ b/src/a.js
@@ -4,3 +4,4 @@
 keep
-old
+new
+more
 keep
--
2.45.0
";
        let files = parse_additions(patch);
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(
            files[0].blocks,
            [(5, vec!["new".to_string(), "more".to_string()])]
        );
    }

    #[test]
    fn message_table_escapes_html_and_keeps_one_line_per_cell() {
        let patch: Patch = serde_json::from_value(serde_json::json!({
            "id": "upl_0000000001",
            "status": "queued",
            "dependsOn": [],
            "createdAt": "",
            "updatedAt": "",
            "source": {},
            "events": [],
            "commitMessage": "Fix <b> & co\n\n</pre></td> body",
        }))
        .unwrap();
        let table = format_message_table(&patch);
        assert!(
            table.contains("<pre>Fix &lt;b&gt; &amp; co<br><br>&lt;/pre&gt;&lt;/td&gt; body</pre>"),
            "{table}"
        );
        let inside = table.split("<table>").nth(1).unwrap();
        assert!(
            !inside.contains("\n\n"),
            "no blank line inside the table\n{table}"
        );
    }

    #[test]
    fn the_scan_reads_added_lines_and_kept_paths_only() {
        let diff = "\
diff --git a/src/a.js b/src/a.js
--- a/src/a.js
+++ b/src/a.js
@@ -1,3 +1,3 @@
 // context mentions acme-context
-// removed mentions acme-removed
+// added mentions acme-added
 end
diff --git a/old.txt b/acme-renamed.txt
similarity index 100%
rename from old.txt
rename to acme-renamed.txt
diff --git a/acme-deleted.txt b/acme-deleted.txt
deleted file mode 100644
--- a/acme-deleted.txt
+++ /dev/null
@@ -1 +0,0 @@
-acme-gone
";
        let surface = export_surface_lines(diff);
        assert!(surface.contains("acme-added"), "{surface}");
        assert!(surface.contains("acme-renamed.txt"), "{surface}");
        assert!(surface.contains("src/a.js"), "{surface}");
        for kept_out in [
            "acme-context",
            "acme-removed",
            "acme-deleted",
            "acme-gone",
            "old.txt",
        ] {
            assert!(!surface.contains(kept_out), "{kept_out}\n{surface}");
        }
    }
}
