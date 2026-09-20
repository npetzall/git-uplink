use std::fs;
use std::io::{IsTerminal, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::lock::with_queue_lock;
use crate::prepare::{assert_prepare_ok, company_commit_message, prepare_from_message};
use crate::queue::{
    add_event, cannot_depend_on, get_patch, patch_path, read_queue as read_queue_file,
    write_queue as write_queue_file,
};
use crate::repo::{
    commit_queue, has_ref, new_patch_id, rev_parse, stable_patch_id, stamp, write_product_patch,
};
use crate::types::{Patch, PatchSource, QueueState};

pub const ADOPT_FROM_REF: &str = "uplink/adopt-from";
const ADOPT_NOTE_PREFIX: &str = "adopted from ";

#[derive(Debug, Clone)]
pub struct AdoptCommit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub is_merge: bool,
    pub parent_count: usize,
    pub first_parent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdoptGroup {
    pub commits: Vec<String>,
    pub title: String,
    #[serde(default = "default_upstream_intent")]
    pub intent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn default_upstream_intent() -> String {
    "upstream".into()
}

#[derive(Debug, Clone)]
pub struct AheadAnalysis {
    pub upstream_sha: String,
    pub head_sha: String,
    pub commits: Vec<AdoptCommit>,
    pub behind: bool,
}

struct ResolvedGroup {
    from_sha: String,
    head_sha: String,
    shas: Vec<String>,
    title: String,
    intent: String,
    message: String,
    previous_was_internal_only: bool,
}

pub fn analyze_ahead(repo: &Path) -> Result<AheadAnalysis> {
    if !has_ref(repo, "uplink/upstream")? {
        return Err(Error::msg(
            "uplink/upstream is missing; cannot compare internal history to upstream.",
        ));
    }
    let tip = if has_ref(repo, ADOPT_FROM_REF)? {
        ADOPT_FROM_REF
    } else {
        "HEAD"
    };
    if !has_ref(repo, tip)? {
        return Ok(AheadAnalysis {
            upstream_sha: rev_parse(repo, "uplink/upstream")?,
            head_sha: String::new(),
            commits: Vec::new(),
            behind: false,
        });
    }
    let upstream_sha = rev_parse(repo, "uplink/upstream")?;
    let head_sha = rev_parse(repo, tip)?;
    let merge_base = git(
        repo,
        &["merge-base", "uplink/upstream", tip],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    let behind = merge_base.code != 0 || merge_base.stdout != upstream_sha;
    if behind {
        return Ok(AheadAnalysis {
            upstream_sha,
            head_sha,
            commits: Vec::new(),
            behind: true,
        });
    }
    let range = format!("uplink/upstream..{tip}");
    let listed = git_ok(repo, &["rev-list", "--first-parent", "--reverse", &range])?;
    let mut commits = Vec::new();
    for sha in listed.lines().filter(|line| !line.is_empty()) {
        let Some(commit) = describe_commit(repo, sha)? else {
            continue;
        };
        if product_diff_empty(repo, &commit.first_parent, &commit.sha)? {
            continue;
        }
        commits.push(commit);
    }
    Ok(AheadAnalysis {
        upstream_sha,
        head_sha,
        commits,
        behind: false,
    })
}

pub fn behind_error(analysis: &AheadAnalysis) -> Error {
    Error::msg(format!(
        "internal HEAD ({}) is not a fast-forward of uplink/upstream ({}). \
Rebase or merge current upstream onto internal, then re-run git uplink init.",
        &analysis.head_sha[..analysis.head_sha.len().min(8)],
        &analysis.upstream_sha[..analysis.upstream_sha.len().min(8)]
    ))
}

pub fn save_adopt_from(repo: &Path) -> Result<()> {
    git(
        repo,
        &["branch", "-f", ADOPT_FROM_REF, "HEAD"],
        GitOpts::default(),
    )?;
    Ok(())
}

pub fn clear_adopt_from(repo: &Path) -> Result<()> {
    if has_ref(repo, ADOPT_FROM_REF)? {
        git(repo, &["branch", "-D", ADOPT_FROM_REF], GitOpts::default())?;
    }
    Ok(())
}

pub fn has_adopt_from(repo: &Path) -> Result<bool> {
    has_ref(repo, ADOPT_FROM_REF)
}

pub fn has_product_patches(queue: &QueueState) -> bool {
    !queue.upstream.is_empty() || !queue.internal.is_empty()
}

pub fn load_groups_file(path: &Path) -> Result<Vec<AdoptGroup>> {
    let raw = if path.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        fs::read_to_string(path).map_err(|err| {
            Error::msg(format!(
                "could not read --adopt-groups {}: {err}",
                path.display()
            ))
        })?
    };
    let groups: Vec<AdoptGroup> = serde_json::from_str(&raw)?;
    Ok(groups)
}

pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn resolve_groups(analysis: &AheadAnalysis, groups: &[AdoptGroup]) -> Result<Vec<AdoptGroup>> {
    let resolved = resolve_group_runs(analysis, groups)?;
    Ok(resolved
        .into_iter()
        .map(|g| AdoptGroup {
            commits: g.shas,
            title: g.title,
            intent: g.intent,
            message: Some(g.message),
        })
        .collect())
}

pub fn apply_groups(
    repo: &Path,
    analysis: &AheadAnalysis,
    groups: &[AdoptGroup],
) -> Result<QueueState> {
    let resolved = resolve_group_runs(analysis, groups)?;
    with_queue_lock(repo, || apply_groups_locked(repo, resolved))
}

pub fn adopted_next_steps() -> &'static str {
    "Company main is unchanged and nothing was pushed.\n\
Preview with: git uplink rebuild --branch uplink/verify\n\
Inspect with:  git diff main uplink/verify\n\
After verification: git uplink rebuild --push"
}

fn describe_commit(repo: &Path, sha: &str) -> Result<Option<AdoptCommit>> {
    let parents = git_ok(repo, &["rev-list", "--parents", "-n1", sha])?;
    let mut parts = parents.split_whitespace();
    let full = parts.next().unwrap_or(sha).to_string();
    let parent_list: Vec<String> = parts.map(str::to_string).collect();
    if parent_list.is_empty() {
        return Ok(None);
    }
    let subject = git_ok(repo, &["log", "-1", "--format=%s", sha])?;
    let parent_count = parent_list.len();
    Ok(Some(AdoptCommit {
        sha: full.clone(),
        short: full.chars().take(7).collect(),
        subject,
        is_merge: parent_count >= 2,
        parent_count,
        first_parent: parent_list[0].clone(),
    }))
}

fn product_diff_empty(repo: &Path, from: &str, head: &str) -> Result<bool> {
    let diff = git(
        repo,
        &[
            "diff",
            "--quiet",
            "--full-index",
            from,
            head,
            "--",
            ".",
            ":!.uplink",
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    Ok(diff.code == 0)
}

fn resolve_sha<'a>(commits: &'a [AdoptCommit], spec: &str) -> Result<&'a AdoptCommit> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(Error::msg("adopt group commit id is empty"));
    }
    let matches: Vec<_> = commits
        .iter()
        .filter(|c| c.sha == spec || c.sha.starts_with(spec) || c.short == spec)
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(Error::msg(format!(
            "adopt group commit '{spec}' is not in the first-parent range ahead of upstream"
        ))),
        _ => Err(Error::msg(format!(
            "adopt group commit '{spec}' is ambiguous"
        ))),
    }
}

fn resolve_group_runs(
    analysis: &AheadAnalysis,
    groups: &[AdoptGroup],
) -> Result<Vec<ResolvedGroup>> {
    if analysis.commits.is_empty() {
        return Err(Error::msg("no unique first-parent commits to adopt"));
    }
    if groups.is_empty() {
        return Err(Error::msg(
            "internal is ahead of uplink/upstream; pass --adopt-groups <file> or re-run in a terminal to group commits",
        ));
    }
    let mut assigned = vec![None; analysis.commits.len()];
    let mut runs = Vec::new();
    let mut previous_was_internal_only = false;
    for (group_idx, group) in groups.iter().enumerate() {
        if group.commits.is_empty() {
            return Err(Error::msg(format!(
                "adopt group {} has no commits",
                group_idx + 1
            )));
        }
        let mut indices = Vec::new();
        let mut shas = Vec::new();
        for spec in &group.commits {
            let commit = resolve_sha(&analysis.commits, spec)?;
            let idx = analysis
                .commits
                .iter()
                .position(|c| c.sha == commit.sha)
                .expect("resolved commit is in the list");
            if assigned[idx].is_some() {
                return Err(Error::msg(format!(
                    "commit {} is in more than one adopt group",
                    commit.short
                )));
            }
            assigned[idx] = Some(group_idx);
            indices.push(idx);
            shas.push(commit.sha.clone());
        }
        indices.sort_unstable();
        shas = indices
            .iter()
            .map(|i| analysis.commits[*i].sha.clone())
            .collect();
        let first = indices[0];
        let last = *indices.last().unwrap();
        if last + 1 - first != indices.len() {
            return Err(Error::msg(format!(
                "adopt group \"{}\" is not a contiguous first-parent run",
                group.title
            )));
        }
        for window in indices.windows(2) {
            if window[1] != window[0] + 1 {
                return Err(Error::msg(format!(
                    "adopt group \"{}\" is not a contiguous first-parent run",
                    group.title
                )));
            }
        }
        let intent = group.intent.trim();
        let intent = if intent.is_empty() {
            "upstream"
        } else if intent == "upstream" || intent == "internal-only" {
            intent
        } else {
            return Err(Error::msg(format!(
                "adopt group \"{}\" has unknown intent \"{}\" (use upstream or internal-only)",
                group.title, group.intent
            )));
        };
        let title = group.title.trim();
        if title.is_empty() {
            return Err(Error::msg("adopt group title is empty"));
        }
        let message = group
            .message
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(title)
            .to_string();
        runs.push(ResolvedGroup {
            from_sha: analysis.commits[first].first_parent.clone(),
            head_sha: analysis.commits[last].sha.clone(),
            shas,
            title: title.to_string(),
            intent: intent.to_string(),
            message,
            previous_was_internal_only,
        });
        previous_was_internal_only = intent == "internal-only";
    }
    if let Some(idx) = assigned.iter().position(|g| g.is_none()) {
        return Err(Error::msg(format!(
            "commit {} is not assigned to an adopt group",
            analysis.commits[idx].short
        )));
    }
    let mut order: Vec<(usize, usize)> = assigned
        .iter()
        .enumerate()
        .filter_map(|(commit_idx, group)| group.map(|g| (g, commit_idx)))
        .collect();
    order.sort_unstable();
    let mut seen = Vec::new();
    for (group_idx, _) in order {
        if seen.last() == Some(&group_idx) {
            continue;
        }
        if seen.contains(&group_idx) {
            return Err(Error::msg(
                "adopt group numbers interleave along first-parent history (1, 2, 1 is not allowed)",
            ));
        }
        seen.push(group_idx);
    }
    Ok(runs)
}

fn apply_groups_locked(repo: &Path, groups: Vec<ResolvedGroup>) -> Result<QueueState> {
    let mut queue = read_queue_file(repo)?;
    if queue.tooling.is_none() {
        return Err(Error::msg(
            "Uplink tooling patch is missing; cannot adopt history until init installs it.",
        ));
    }
    if has_product_patches(&queue) {
        return Err(Error::msg(
            "queue already has product patches; delete uplink/adopt-from or reset uplink/state before adopting again",
        ));
    }
    let mut written = Vec::new();
    let mut last_upstream_id: Option<String> = None;
    let outcome = (|| -> Result<()> {
        for group in &groups {
            if group.intent == "upstream" && group.previous_was_internal_only {
                // still recorded; operator was warned in the TUI
            }
            let intent = group.intent.as_str();
            let id = new_patch_id();
            let created_at = stamp();
            let depends_on = if intent == "upstream" {
                last_upstream_id.clone().into_iter().collect()
            } else {
                Vec::new()
            };
            if let Some(dep_id) = depends_on.first() {
                get_patch(&queue, dep_id)?;
                if cannot_depend_on(&queue, intent == "internal-only", dep_id) {
                    return Err(Error::msg(format!(
                        "Upstream-bound patch \"{}\" cannot depend on internal-only patch {dep_id}.",
                        group.title
                    )));
                }
            }
            let mut patch = Patch {
                id: id.clone(),
                title: group.title.clone(),
                commit_message: String::new(),
                status: "queued".into(),
                depends_on,
                created_at: created_at.clone(),
                updated_at: created_at,
                patch_id_stable: None,
                source: PatchSource {
                    note: Some(format!(
                        "{ADOPT_NOTE_PREFIX}{}..{}",
                        &group.from_sha[..group.from_sha.len().min(8)],
                        &group.head_sha[..group.head_sha.len().min(8)]
                    )),
                    ..Default::default()
                },
                prepare: None,
                upstream: None,
                merged: None,
                conflict: None,
                approvals: Vec::new(),
                events: Vec::new(),
                kind: None,
            };
            add_event(
                &mut patch,
                "created",
                format!(
                    "Adopted from {}..{} as {intent}",
                    &group.from_sha[..group.from_sha.len().min(8)],
                    &group.head_sha[..group.head_sha.len().min(8)]
                ),
            );
            patch.prepare = Some(prepare_from_message(
                repo,
                &queue,
                &group.from_sha,
                &group.head_sha,
                &group.message,
                Some(&group.title),
                intent,
            )?);
            if let Some(report) = &patch.prepare {
                patch.commit_message = report.commit_message.clone();
                if intent == "upstream" {
                    assert_prepare_ok(report, &group.title)?;
                }
            }
            let message = company_commit_message(&patch);
            write_product_patch(repo, &id, &group.from_sha, &message, &group.head_sha)?;
            written.push(id.clone());
            let rel = patch_path(&id)?.to_string_lossy().into_owned();
            patch.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
            if intent == "upstream" {
                last_upstream_id = Some(id.clone());
            }
            queue.push_patch(patch, intent == "internal-only");
        }
        write_queue_file(repo, &queue)?;
        let n = written.len();
        commit_queue(
            repo,
            &format!("uplink: adopt {n} patch{}", if n == 1 { "" } else { "es" }),
        )?;
        Ok(())
    })();
    if outcome.is_err() {
        for id in written {
            if let Ok(rel) = patch_path(&id) {
                let _ = fs::remove_file(repo.join(rel));
            }
        }
    }
    outcome?;
    read_queue_file(repo)
}

pub fn default_title_for(commits: &[AdoptCommit]) -> String {
    commits
        .last()
        .map(|c| c.subject.clone())
        .unwrap_or_else(|| "Adopted change".into())
}

pub fn groups_from_numbers(commits: &[AdoptCommit], numbers: &[u32]) -> Result<Vec<AdoptGroup>> {
    if commits.len() != numbers.len() {
        return Err(Error::msg("group assignment does not match commit list"));
    }
    if numbers.contains(&0) {
        return Err(Error::msg("every commit must be assigned a group number"));
    }
    let mut groups = Vec::new();
    let mut i = 0;
    while i < commits.len() {
        let number = numbers[i];
        let mut j = i + 1;
        while j < commits.len() && numbers[j] == number {
            j += 1;
        }
        if numbers[j..].contains(&number) {
            return Err(Error::msg(
                "adopt group numbers interleave along first-parent history (1, 2, 1 is not allowed)",
            ));
        }
        let slice = &commits[i..j];
        let title = default_title_for(slice);
        groups.push(AdoptGroup {
            commits: slice.iter().map(|c| c.sha.clone()).collect(),
            title: title.clone(),
            intent: "upstream".into(),
            message: Some(title),
        });
        i = j;
    }
    Ok(groups)
}
