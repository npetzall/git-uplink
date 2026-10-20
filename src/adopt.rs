use std::fs;
use std::io::{IsTerminal, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::assess::{assert_assess_ok, assess_from_message, company_commit_message};
use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::lock::with_queue_lock;
use crate::queue::{
    add_event, cannot_depend_on, get_patch, patch_path, read_queue as read_queue_file,
    write_queue as write_queue_file,
};
use crate::repo::{
    commit_queue, has_ref, new_patch_id, rev_parse, stable_patch_id, stamp, write_product_patch,
};
use crate::types::{Patch, PatchIntent, PatchSource, PatchStatus, QueueState};

mod branch;

pub use branch::{
    ADOPT_AMENDED_REF_PREFIX, AdoptDecider, AdoptDecision, AdoptFailure, AdoptFailureKind,
    AdoptResume, BranchAdopted, BranchAdoption, BranchRun, groups_file_path, refuse_when_paused,
    resume_branch, run_branch, write_groups_file,
};

pub const ADOPT_FROM_REF: &str = "uplink/adopt-from";
const ADOPT_NOTE_PREFIX: &str = "adopted from ";

#[derive(Debug, Clone)]
pub struct AdoptCommit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    /// The message after the subject, trimmed.
    pub body: String,
    pub is_merge: bool,
    pub parent_count: usize,
    pub first_parent: String,
    /// An equivalent change is already on company main.
    pub on_main: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptGroup {
    pub commits: Vec<String>,
    pub title: String,
    #[serde(default)]
    pub intent: PatchIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Adopt the group even when `preflight.sh` fails on it. It then also
    /// depends on the upstream patch queued before it. `adopt --branch` only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_preflight: bool,
    /// A commit holding the group's change as amended inline; its diff is
    /// adopted instead of the commits'. `adopt --branch` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amended: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AheadAnalysis {
    pub upstream_sha: String,
    pub head_sha: String,
    pub commits: Vec<AdoptCommit>,
    pub behind: bool,
    /// Commits may be left out of every group (adopting from a branch).
    pub allow_skip: bool,
}

struct ResolvedGroup {
    from_sha: String,
    head_sha: String,
    shas: Vec<String>,
    title: String,
    intent: PatchIntent,
    message: String,
    ignore_preflight: bool,
    amended: Option<String>,
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
            allow_skip: false,
        });
    }
    let upstream_sha = rev_parse(repo, "uplink/upstream")?;
    let head_sha = rev_parse(repo, tip)?;
    let merge_base = git(
        repo,
        &["merge-base", "uplink/upstream", tip],
        GitOpts::allow_fail(),
    )?;
    let behind = merge_base.code != 0 || merge_base.stdout != upstream_sha;
    if behind {
        return Ok(AheadAnalysis {
            upstream_sha,
            head_sha,
            commits: Vec::new(),
            behind: true,
            allow_skip: false,
        });
    }
    let commits = list_first_parent(repo, "uplink/upstream", tip)?;
    Ok(AheadAnalysis {
        upstream_sha,
        head_sha,
        commits,
        behind: false,
        allow_skip: false,
    })
}

/// Commits on `branch` since it left public upstream, for adopting from a
/// branch other than company main. `behind` here only means the branch
/// forked from an older upstream; the commits are listed all the same.
/// Every ref must already be local: nothing is fetched.
pub fn analyze_branch(repo: &Path, branch: &str, company_branch: &str) -> Result<AheadAnalysis> {
    let upstream_sha = rev_parse(repo, "uplink/upstream")?;
    let head_sha = rev_parse(repo, branch)?;
    let merge_base = git(
        repo,
        &["merge-base", "uplink/upstream", &head_sha],
        GitOpts::allow_fail(),
    )?;
    if merge_base.code != 0 || merge_base.stdout.is_empty() {
        return Err(Error::msg(format!(
            "{branch} shares no history with uplink/upstream; nothing to adopt from it."
        )));
    }
    let base = merge_base.stdout;
    let mut commits = list_first_parent(repo, &base, &head_sha)?;
    let unique = commits_not_on_main(repo, company_branch, &head_sha, &base)?;
    for commit in &mut commits {
        commit.on_main = !unique.contains(&commit.sha);
    }
    Ok(AheadAnalysis {
        behind: base != upstream_sha,
        upstream_sha,
        head_sha,
        commits,
        allow_skip: true,
    })
}

/// First-parent commits in `base..tip`, oldest first, without root commits
/// and commits that change nothing outside `.uplink`.
fn list_first_parent(repo: &Path, base: &str, tip: &str) -> Result<Vec<AdoptCommit>> {
    let range = format!("{base}..{tip}");
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
    Ok(commits)
}

/// Commits in `base..tip` whose change is not on `company_branch`. `git
/// cherry` marks an equivalent change `-` and leaves out commits that are
/// on it already, so only `+` counts.
fn commits_not_on_main(
    repo: &Path,
    company_branch: &str,
    tip: &str,
    base: &str,
) -> Result<Vec<String>> {
    let listed = git_ok(repo, &["cherry", company_branch, tip, base])?;
    Ok(listed
        .lines()
        .filter_map(|line| line.strip_prefix("+ "))
        .map(str::to_string)
        .collect())
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
            ignore_preflight: g.ignore_preflight,
            amended: g.amended,
        })
        .collect())
}

pub fn apply_groups(
    repo: &Path,
    analysis: &AheadAnalysis,
    groups: &[AdoptGroup],
) -> Result<QueueState> {
    if let Some(group) = groups
        .iter()
        .find(|g| g.ignore_preflight || g.amended.is_some())
    {
        return Err(Error::msg(format!(
            "adopt group \"{}\" sets ignorePreflight or amended, which only git uplink adopt --branch supports",
            group.title
        )));
    }
    let resolved = resolve_group_runs(analysis, groups)?;
    with_queue_lock(repo, || apply_groups_locked(repo, resolved))
}

pub const ADOPT_TEST_BRANCH_PREFIX: &str = "uplink/adopt-test/";

pub fn adopted_next_steps() -> &'static str {
    "Company main is unchanged.\n\
Preview with: git uplink rebuild --branch uplink/preview/verify\n\
Inspect with:  git diff main uplink/preview/verify\n\
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
    let message = git_ok(repo, &["log", "-1", "--format=%s%x00%b", sha])?;
    let (subject, body) = message.split_once('\0').unwrap_or((message.as_str(), ""));
    let (subject, body) = (subject.to_string(), body.trim().to_string());
    let parent_count = parent_list.len();
    Ok(Some(AdoptCommit {
        sha: full.clone(),
        short: full.chars().take(7).collect(),
        subject,
        body,
        is_merge: parent_count >= 2,
        parent_count,
        first_parent: parent_list[0].clone(),
        on_main: false,
    }))
}

fn product_diff_empty(repo: &Path, from: &str, head: &str) -> Result<bool> {
    git_succeeds(
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
    )
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
        return Err(Error::msg(if analysis.allow_skip {
            "no adopt groups; pass --groups <file> or re-run in a terminal to pick commits"
        } else {
            "internal is ahead of uplink/upstream; pass --adopt-groups <file> or re-run in a terminal to group commits"
        }));
    }
    let mut assigned = vec![None; analysis.commits.len()];
    let runs = groups
        .iter()
        .enumerate()
        .map(|(group_idx, group)| resolve_group(analysis, group, group_idx, &mut assigned))
        .collect::<Result<Vec<_>>>()?;
    check_group_order(analysis, &assigned)?;
    Ok(runs)
}

/// Resolves one group to a contiguous first-parent run, recording its commits in `assigned`.
fn resolve_group(
    analysis: &AheadAnalysis,
    group: &AdoptGroup,
    group_idx: usize,
    assigned: &mut [Option<usize>],
) -> Result<ResolvedGroup> {
    if group.commits.is_empty() {
        return Err(Error::msg(format!(
            "adopt group {} has no commits",
            group_idx + 1
        )));
    }
    let mut indices = Vec::new();
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
    }
    indices.sort_unstable();
    // Indices are unique, so a span as long as the count means no gaps.
    let first = indices[0];
    let last = *indices.last().unwrap();
    if last + 1 - first != indices.len() {
        let skipped = if analysis.allow_skip {
            "; a group cannot span a skipped commit"
        } else {
            ""
        };
        return Err(Error::msg(format!(
            "adopt group \"{}\" is not a contiguous first-parent run{skipped}",
            group.title
        )));
    }
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
    Ok(ResolvedGroup {
        from_sha: analysis.commits[first].first_parent.clone(),
        head_sha: analysis.commits[last].sha.clone(),
        shas: indices
            .iter()
            .map(|i| analysis.commits[*i].sha.clone())
            .collect(),
        title: title.to_string(),
        intent: group.intent,
        message,
        ignore_preflight: group.ignore_preflight,
        amended: group.amended.clone(),
    })
}

/// Every commit must be in a group, unless the analysis allows skipping, and
/// group numbers must not interleave along history. A group is already a
/// contiguous run of the full list, so it never spans a skipped commit.
fn check_group_order(analysis: &AheadAnalysis, assigned: &[Option<usize>]) -> Result<()> {
    if !analysis.allow_skip
        && let Some(idx) = assigned.iter().position(|g| g.is_none())
    {
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
    Ok(())
}

/// init: the queue holds only tooling, and every upstream-bound group
/// depends on the upstream-bound group before it.
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
            let internal_only = group.intent.is_internal_only();
            let mut patch = adopted_patch(repo, &queue, group, last_upstream_id.as_deref(), None)?;
            let id = patch.id.clone();
            let message = company_commit_message(&patch);
            write_product_patch(repo, &id, &group.from_sha, &message, &group.head_sha)?;
            written.push(id.clone());
            let rel = patch_path(&id)?.to_string_lossy().into_owned();
            patch.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
            if !internal_only {
                last_upstream_id = Some(id);
            }
            queue.push_patch(patch, internal_only);
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
        remove_patch_files(repo, &written);
    }
    outcome?;
    read_queue_file(repo)
}

fn remove_patch_files(repo: &Path, ids: &[String]) {
    for id in ids {
        if let Ok(rel) = patch_path(id) {
            let _ = fs::remove_file(repo.join(rel));
        }
    }
}

/// Builds and assesses the queued patch for one adopt group, depending on
/// `chained` (the previous upstream-bound group in init) when upstream-bound.
fn adopted_patch(
    repo: &Path,
    queue: &QueueState,
    group: &ResolvedGroup,
    chained: Option<&str>,
    source: Option<&str>,
) -> Result<Patch> {
    let intent = group.intent;
    let depends_on: Vec<String> = if !intent.is_internal_only() {
        chained.map(str::to_string).into_iter().collect()
    } else {
        Vec::new()
    };
    if let Some(dep_id) = depends_on.first() {
        get_patch(queue, dep_id)?;
        if cannot_depend_on(queue, intent.is_internal_only(), dep_id) {
            return Err(Error::msg(format!(
                "Upstream-bound patch \"{}\" cannot depend on internal-only patch {dep_id}.",
                group.title
            )));
        }
    }
    let shas = format!(
        "{}..{}",
        &group.from_sha[..group.from_sha.len().min(8)],
        &group.head_sha[..group.head_sha.len().min(8)]
    );
    let range = match source {
        Some(branch) => format!("{branch} {shas}"),
        None => shas,
    };
    let created_at = stamp();
    let mut patch = Patch {
        id: new_patch_id(),
        title: group.title.clone(),
        commit_message: String::new(),
        status: PatchStatus::Queued,
        depends_on,
        created_at: created_at.clone(),
        updated_at: created_at,
        patch_id_stable: None,
        source: PatchSource {
            note: Some(format!("{ADOPT_NOTE_PREFIX}{range}")),
            ..Default::default()
        },
        assess: None,
        upstream: None,
        merged: None,
        conflict: None,
        approvals: Vec::new(),
        extras: None,
        events: Vec::new(),
        kind: None,
    };
    add_event(
        &mut patch,
        "created",
        format!("Adopted from {range} as {intent}"),
    );
    let report = assess_from_message(
        repo,
        queue,
        &group.from_sha,
        &group.head_sha,
        &group.message,
        Some(&group.title),
        intent,
    )?;
    patch.commit_message = report.commit_message.clone();
    if !intent.is_internal_only() {
        assert_assess_ok(&report, &group.title)?;
    }
    patch.assess = Some(report);
    Ok(patch)
}
