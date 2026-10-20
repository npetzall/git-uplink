//! `adopt --branch`: groups are adopted one at a time. A group that fails
//! is put to the operator: adopt it with its preflight failure ignored,
//! make it internal-only, skip it, or amend it inline, which pauses the run
//! the way `init --upgrade` pauses on a patch until `--continue`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    ADOPT_TEST_BRANCH_PREFIX, AdoptGroup, AheadAnalysis, adopted_patch, remove_patch_files,
    resolve_group,
};
use crate::assess::company_commit_message;
use crate::error::{Error, PreflightError, Result};
use crate::gate::{assert_resolution_clean, commit_resolution};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::ops::{checkout_identity, restore_checkout};
use crate::preflight::{
    ScriptVerdict, apply_abs, detect_depends_on, export_preflight, is_script_failure,
};
use crate::queue::{
    active_upstream, add_event, get_patch, patch_path, read_queue as read_queue_file,
    write_queue as write_queue_file,
};
use crate::repo::{
    UPSTREAM_REF, commit_queue, conflicted_files, ensure_state_worktree, rev_parse,
    stable_patch_id, write_product_patch,
};
use crate::types::{ApplyOutcome, Patch, PatchIntent, PatchStatus, QueueState};

/// Keeps an amended group's commit from being garbage collected while the
/// groups file names it.
pub const ADOPT_AMENDED_REF_PREFIX: &str = "refs/uplink/adopt-amended/";

/// One patch adopted from a branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchAdopted {
    pub id: String,
    pub title: String,
    pub intent: PatchIntent,
    pub depends_on: Vec<String>,
    /// Left at upstream + dependencies + this patch, when asked for.
    pub test_branch: Option<String>,
    /// Token of the tree the export preflight passed on; None for
    /// internal-only, and for a patch adopted with its failure ignored.
    pub preflight: Option<String>,
    /// Adopted although `preflight.sh` failed on it.
    pub preflight_ignored: bool,
    /// Its change was amended inline while adopting.
    pub amended: bool,
}

#[derive(Debug, Clone)]
pub struct BranchAdoption {
    pub queue: QueueState,
    pub adopted: Vec<BranchAdopted>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptFailureKind {
    /// No set of queued upstream patches makes the group apply onto public upstream.
    Apply,
    /// `preflight.sh` fails on the group, with every set of queued patches tried.
    Preflight,
}

/// A group that cannot be adopted as it is.
#[derive(Debug, Clone)]
pub struct AdoptFailure {
    pub kind: AdoptFailureKind,
    /// Its number in the groups file, from 1.
    pub group: usize,
    pub title: String,
    pub message: String,
    /// Amending replaces the checkout, so it needs one without changes.
    pub can_amend: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptDecision {
    /// Only for a preflight failure.
    IgnorePreflight,
    InternalOnly,
    /// Ignored when [`AdoptFailure::can_amend`] is false.
    Amend,
    Skip,
    Abort,
}

/// Says what to do with a group that fails.
pub type AdoptDecider = Arc<dyn Fn(&AdoptFailure) -> Result<AdoptDecision> + Send + Sync>;

/// How `adopt --continue`, `--skip` or `--abort` takes up a paused run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptResume {
    /// The amended files are added: what they hold becomes the group's change.
    Continue,
    /// Leave out the group the run paused on.
    Skip,
    /// Put the checkout back; nothing is written.
    Abort,
}

/// The groups of one `adopt --branch` and how far it got.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchRun {
    pub branch: String,
    pub groups_file: PathBuf,
    pub groups: Vec<AdoptGroup>,
    pub keep_test_branches: bool,
    /// Index in `groups` of the group adopted next.
    next: usize,
    settled: Vec<Settled>,
}

impl BranchRun {
    pub fn new(
        branch: String,
        groups_file: PathBuf,
        groups: Vec<AdoptGroup>,
        keep_test_branches: bool,
    ) -> Self {
        Self {
            branch,
            groups_file,
            groups,
            keep_test_branches,
            next: 0,
            settled: Vec::new(),
        }
    }
}

/// A group adopted in this run; written to the queue when the run ends.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settled {
    patch: Patch,
    internal_only: bool,
    adopted: BranchAdopted,
}

/// A run paused on a group to amend, in `<git-dir>/uplink-adopt.json`.
/// Its settled patch files are in `.uplink/patches`; queue.json is not
/// written yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PausedAdopt {
    run: BranchRun,
    /// The checkout the run started from, as `checkout_identity` gives it.
    original: String,
    original_sha: String,
    /// uplink/upstream with `deps` applied, before the group's change.
    onto: String,
    deps: Vec<String>,
    reason: String,
    /// In conflict after applying the group's change.
    files: Vec<String>,
    /// The change could not be applied at all.
    unapplied: bool,
}

impl PausedAdopt {
    fn instructions(&self) -> String {
        let group = &self.run.groups[self.run.next];
        let on = if self.deps.is_empty() {
            "uplink/upstream".to_string()
        } else {
            format!("uplink/upstream + {}", self.deps.join(", "))
        };
        let state = if self.unapplied {
            "could not be applied; make the change by hand".to_string()
        } else if self.files.is_empty() {
            "applied and staged".to_string()
        } else {
            format!("applied, in conflict:\n  {}", self.files.join("\n  "))
        };
        format!(
            "adopt paused on group {} (\"{}\"): {}.\n\
The checkout holds {on} with the group's change {state}\n\
Amend it (run preflight.sh to check), git add the files, then:  git uplink adopt --continue\n\
Leave the group out:     git uplink adopt --skip\n\
Stop, changing nothing:  git uplink adopt --abort\n\
Groups file: {}",
            self.run.next + 1,
            group.title,
            self.reason,
            self.run.groups_file.display()
        )
    }
}

fn common_dir(repo: &Path) -> Result<PathBuf> {
    Ok(repo.join(git_ok(repo, &["rev-parse", "--git-common-dir"])?))
}

fn file_safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Where `adopt --branch` writes the groups of `branch` by default.
pub fn groups_file_path(repo: &Path, branch: &str) -> Result<PathBuf> {
    Ok(common_dir(repo)?
        .join("uplink/adopt")
        .join(format!("{}.json", file_safe(branch))))
}

pub fn write_groups_file(path: &Path, groups: &[AdoptGroup]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(groups)?))?;
    Ok(())
}

fn paused_path(repo: &Path) -> Result<PathBuf> {
    Ok(common_dir(repo)?.join("uplink-adopt.json"))
}

/// Refuses to start an adopt while another one is paused.
pub fn refuse_when_paused(repo: &Path) -> Result<()> {
    let path = paused_path(repo)?;
    if !path.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path)?;
    let detail = serde_json::from_str::<PausedAdopt>(&raw)
        .map(|p| p.instructions())
        .unwrap_or_else(|_| format!("see {}", path.display()));
    Err(Error::msg(format!(
        "an adopt is paused; finish it first.\n{detail}"
    )))
}

/// Adopts the groups of `run` from where it got to, one at a time, and
/// writes the queue in one uplink/state commit at the end. The caller holds
/// the queue lock. A run that pauses returns its instructions as the error,
/// keeping what it settled; any other error rolls back what the run wrote.
pub fn run_branch(
    repo: &Path,
    analysis: &AheadAnalysis,
    mut run: BranchRun,
    verdict: &ScriptVerdict,
    decide: Option<&AdoptDecider>,
) -> Result<BranchAdoption> {
    let mut queue = read_queue_file(repo)?;
    for settled in &run.settled {
        queue.push_patch(settled.patch.clone(), settled.internal_only);
    }
    let outcome = (|| -> Result<Option<String>> {
        while run.next < run.groups.len() {
            match adopt_next(repo, analysis, &queue, &mut run, verdict, decide)? {
                Step::Settled(settled) => {
                    queue.push_patch(settled.patch.clone(), settled.internal_only);
                    run.settled.push(*settled);
                    run.next += 1;
                }
                Step::Again => write_groups_file(&run.groups_file, &run.groups)?,
                Step::Skip => {
                    run.groups.remove(run.next);
                    write_groups_file(&run.groups_file, &run.groups)?;
                }
                Step::Paused(instructions) => return Ok(Some(instructions)),
            }
        }
        if run.settled.is_empty() {
            return Err(Error::msg("every group was skipped; nothing to adopt"));
        }
        write_queue_file(repo, &queue)?;
        let n = run.settled.len();
        commit_queue(
            repo,
            &format!(
                "uplink: adopt {n} patch{} from {}",
                if n == 1 { "" } else { "es" },
                run.branch
            ),
        )?;
        Ok(None)
    })();
    match outcome {
        Ok(Some(instructions)) => Err(Error::msg(instructions)),
        Ok(None) => Ok(BranchAdoption {
            queue: read_queue_file(repo)?,
            adopted: run.settled.into_iter().map(|s| s.adopted).collect(),
        }),
        Err(err) => {
            roll_back(repo, &run);
            Err(err)
        }
    }
}

/// Removes the patch files and test branches of what `run` settled.
fn roll_back(repo: &Path, run: &BranchRun) {
    let ids: Vec<String> = run.settled.iter().map(|s| s.patch.id.clone()).collect();
    remove_patch_files(repo, &ids);
    for branch in run
        .settled
        .iter()
        .filter_map(|s| s.adopted.test_branch.as_deref())
    {
        let _ = git(repo, &["branch", "-D", branch], GitOpts::allow_fail());
    }
}

enum Step {
    Settled(Box<Settled>),
    /// The group was changed (made internal-only); adopt it again.
    Again,
    Skip,
    Paused(String),
}

/// Adopts `run.groups[run.next]`, or says what happens to it instead.
fn adopt_next(
    repo: &Path,
    analysis: &AheadAnalysis,
    queue: &QueueState,
    run: &mut BranchRun,
    verdict: &ScriptVerdict,
    decide: Option<&AdoptDecider>,
) -> Result<Step> {
    let index = run.next;
    let group = run.groups[index].clone();
    let mut resolved = resolve_group(
        analysis,
        &group,
        index,
        &mut vec![None; analysis.commits.len()],
    )?;
    if let Some(sha) = &group.amended {
        resolved.head_sha = rev_parse(repo, sha)?;
        resolved.from_sha = rev_parse(repo, &format!("{sha}^"))?;
    }
    let internal_only = resolved.intent.is_internal_only();
    let mut patch = adopted_patch(repo, queue, &resolved, None, Some(&run.branch))?;
    let id = patch.id.clone();
    write_product_patch(
        repo,
        &id,
        &resolved.from_sha,
        &company_commit_message(&patch),
        &resolved.head_sha,
    )?;
    let test_branch = (run.keep_test_branches && !internal_only)
        .then(|| format!("{ADOPT_TEST_BRANCH_PREFIX}{}-{id}", index + 1));
    let mut done = BranchAdopted {
        id: id.clone(),
        title: patch.title.clone(),
        intent: resolved.intent,
        depends_on: Vec::new(),
        test_branch: None,
        preflight: None,
        preflight_ignored: false,
        amended: group.amended.is_some(),
    };
    let step = check_group(
        repo,
        queue,
        run,
        &mut patch,
        &mut done,
        test_branch.as_deref(),
        verdict,
        decide,
    )
    .map(|settled| match settled {
        Some(step) => step,
        None => Step::Settled(Box::new(Settled {
            patch,
            internal_only,
            adopted: done,
        })),
    });
    if !matches!(step, Ok(Step::Settled(_))) {
        remove_patch_files(repo, &[id]);
        if let Some(branch) = &test_branch {
            let _ = git(repo, &["branch", "-D", branch], GitOpts::allow_fail());
        }
    }
    step
}

/// Checks the written patch of the next group. `None` when it is adopted,
/// else what happens to the group instead.
#[allow(clippy::too_many_arguments)]
fn check_group(
    repo: &Path,
    queue: &QueueState,
    run: &mut BranchRun,
    patch: &mut Patch,
    done: &mut BranchAdopted,
    test_branch: Option<&str>,
    verdict: &ScriptVerdict,
    decide: Option<&AdoptDecider>,
) -> Result<Option<Step>> {
    let index = run.next;
    let group = run.groups[index].clone();
    let rel = patch_path(&patch.id)?;
    let candidate_abs = repo.join(&rel);
    patch.patch_id_stable = Some(stable_patch_id(repo, &rel.to_string_lossy())?);
    if let Some(existing) = queue.all_patches().find(|item| {
        item.patch_id_stable.is_some() && item.patch_id_stable == patch.patch_id_stable
    }) {
        return Err(Error::msg(format!(
            "\"{}\" is already in the queue as {} \"{}\"; skip those commits.",
            group.title, existing.id, existing.title
        )));
    }
    if group.amended.is_some() {
        add_event(patch, "amended", "Amended inline while adopting");
    }
    if group.intent.is_internal_only() {
        return Ok(None);
    }
    let Some(deps) = detect_depends_on(repo, queue, &candidate_abs, &patch.title, test_branch)?
    else {
        let failure = Failure {
            kind: AdoptFailureKind::Apply,
            message: format!(
                "\"{}\" does not apply onto public upstream, alone or on top of the queued upstream patches.",
                group.title
            ),
            error: None,
        };
        let all: Vec<String> = active_upstream(queue)
            .into_iter()
            .map(|p| p.id.clone())
            .collect();
        return on_failure(repo, queue, run, failure, &candidate_abs, &all, decide)
            .map(|step| Some(step.unwrap_or(Step::Again)));
    };
    done.test_branch = test_branch.map(str::to_string);
    if !deps.is_empty() {
        add_event(
            patch,
            "depends-on",
            format!("Needs {} to apply onto public upstream", deps.join(", ")),
        );
    }
    patch.depends_on = deps;
    let failed = match export_preflight(repo, queue, patch, &candidate_abs, None, verdict) {
        Ok(token) => {
            done.preflight = token;
            None
        }
        Err(Error::Preflight(err)) if is_script_failure(&err) => {
            with_preflight_deps(repo, queue, patch, done, &candidate_abs, verdict, err)?
        }
        Err(err) => return Err(err),
    };
    if let Some(err) = failed {
        if !group.ignore_preflight {
            let failure = Failure {
                kind: AdoptFailureKind::Preflight,
                message: err.to_string(),
                error: Some(err),
            };
            let deps = patch.depends_on.clone();
            if let Some(step) =
                on_failure(repo, queue, run, failure, &candidate_abs, &deps, decide)?
            {
                return Ok(Some(step));
            }
        }
        ignore_failure(queue, patch, done);
    }
    done.depends_on = patch.depends_on.clone();
    Ok(None)
}

/// When `preflight.sh` passes with queued upstream patches the group does
/// not need to apply, it depends on them too. Returns the failure when no
/// such set was found.
fn with_preflight_deps(
    repo: &Path,
    queue: &QueueState,
    patch: &mut Patch,
    done: &mut BranchAdopted,
    candidate_abs: &Path,
    verdict: &ScriptVerdict,
    err: PreflightError,
) -> Result<Option<PreflightError>> {
    let suggested = err.suggested_depends_on.clone();
    let added: Vec<String> = suggested
        .iter()
        .filter(|id| !patch.depends_on.contains(id))
        .cloned()
        .collect();
    if added.is_empty() {
        return Ok(Some(err));
    }
    let before = std::mem::replace(&mut patch.depends_on, suggested);
    match export_preflight(repo, queue, patch, candidate_abs, None, verdict) {
        Ok(token) => {
            add_event(
                patch,
                "depends-on",
                format!(
                    "Needs {} to pass preflight.sh on public upstream",
                    added.join(", ")
                ),
            );
            done.preflight = token;
            Ok(None)
        }
        Err(Error::Preflight(again)) if is_script_failure(&again) => {
            patch.depends_on = before;
            Ok(Some(err))
        }
        Err(other) => Err(other),
    }
}

/// Adopts the patch although `preflight.sh` fails on it. It is exported
/// after the upstream patch queued before it, so it also depends on that
/// patch and what that patch depends on, in queue order.
fn ignore_failure(queue: &QueueState, patch: &mut Patch, done: &mut BranchAdopted) {
    let mut ids = patch.depends_on.clone();
    let previous = active_upstream(queue).last().map(|p| (*p).clone());
    if let Some(previous) = &previous {
        for id in previous
            .depends_on
            .iter()
            .chain(std::iter::once(&previous.id))
        {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    let order: Vec<&str> = queue.upstream.iter().map(|p| p.id.as_str()).collect();
    ids.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    patch.depends_on = ids;
    let after = previous
        .map(|p| format!("; it depends on {} so it is exported after it", p.id))
        .unwrap_or_default();
    add_event(
        patch,
        "preflight-ignored",
        format!("Adopted although preflight.sh failed on public upstream{after}"),
    );
    done.preflight_ignored = true;
}

struct Failure {
    kind: AdoptFailureKind,
    message: String,
    error: Option<PreflightError>,
}

/// Asks what to do with a failing group and does it. `None` when the
/// preflight failure is to be ignored.
fn on_failure(
    repo: &Path,
    queue: &QueueState,
    run: &mut BranchRun,
    failure: Failure,
    candidate_abs: &Path,
    deps: &[String],
    decide: Option<&AdoptDecider>,
) -> Result<Option<Step>> {
    let index = run.next;
    let info = AdoptFailure {
        kind: failure.kind,
        group: index + 1,
        title: run.groups[index].title.clone(),
        message: failure.message.clone(),
        can_amend: checkout_clean(repo)?,
    };
    let Some(decide) = decide else {
        return Err(unattended(run, &info, failure.error));
    };
    let decision = decide(&info)?;
    Ok(Some(match decision {
        AdoptDecision::IgnorePreflight if info.kind == AdoptFailureKind::Preflight => {
            run.groups[index].ignore_preflight = true;
            write_groups_file(&run.groups_file, &run.groups)?;
            return Ok(None);
        }
        AdoptDecision::InternalOnly => {
            run.groups[index].intent = PatchIntent::InternalOnly;
            Step::Again
        }
        AdoptDecision::Amend if info.can_amend => {
            Step::Paused(pause(repo, queue, run, &info, candidate_abs, deps)?)
        }
        AdoptDecision::Skip => Step::Skip,
        _ => {
            return Err(Error::msg(format!(
                "adopt aborted on group {} (\"{}\"); nothing was changed.\nGroups file: {}",
                info.group,
                info.title,
                run.groups_file.display()
            )));
        }
    }))
}

/// The error for a failing group when nobody can be asked.
fn unattended(run: &BranchRun, info: &AdoptFailure, error: Option<PreflightError>) -> Error {
    let file = run.groups_file.display();
    let n = info.group;
    let hint = match info.kind {
        AdoptFailureKind::Preflight => format!(
            "To adopt it anyway, set \"ignorePreflight\": true (or \"intent\": \"internal-only\") on group {n} in {file} \
and re-run with --groups {file}, or run git uplink adopt in a terminal to amend it."
        ),
        AdoptFailureKind::Apply => format!(
            "Rebase those commits onto current upstream, set \"intent\": \"internal-only\" on group {n} in {file} \
and re-run with --groups {file}, or run git uplink adopt in a terminal to amend it."
        ),
    };
    match error {
        Some(err) => {
            let mut failure = PreflightError::new(
                format!("{err}\n\n{hint}"),
                err.suggested_depends_on,
                err.stage,
                err.output,
            );
            if let Some(token) = err.token {
                failure = failure.with_token(token);
            }
            Error::Preflight(failure)
        }
        None => Error::msg(format!("{} {hint}", info.message)),
    }
}

/// No staged or unstaged changes to tracked files.
fn checkout_clean(repo: &Path) -> Result<bool> {
    Ok(git_succeeds(repo, &["diff", "--cached", "--quiet"])?
        && git_succeeds(repo, &["diff", "--quiet"])?)
}

/// Leaves the checkout on uplink/upstream + `deps` with the group's change
/// applied, for the operator to amend, and saves the run. Returns the
/// instructions.
fn pause(
    repo: &Path,
    queue: &QueueState,
    run: &BranchRun,
    info: &AdoptFailure,
    candidate_abs: &Path,
    deps: &[String],
) -> Result<String> {
    let (original, original_sha) = checkout_identity(repo)?;
    let placed = (|| -> Result<(String, Vec<String>, bool)> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", UPSTREAM_REF],
            GitOpts::default(),
        )?;
        for id in deps {
            let dep = get_patch(queue, id)?;
            // Already in upstream, possibly in another form than its patch file.
            if dep.status == PatchStatus::Merged {
                continue;
            }
            if apply_abs(repo, &repo.join(patch_path(id)?), &dep.title)? == ApplyOutcome::Conflict {
                return Err(Error::msg(format!(
                    "{id} (\"{}\") does not apply onto public upstream, so group {} cannot be amended on it",
                    dep.title, info.group
                )));
            }
        }
        let onto = rev_parse(repo, "HEAD")?;
        let applied = git(
            repo,
            &[
                "apply",
                "--3way",
                "--index",
                candidate_abs.to_str().unwrap_or(""),
            ],
            GitOpts::allow_fail(),
        )?;
        let files = conflicted_files(repo)?;
        let unapplied = applied.code != 0 && files.is_empty();
        Ok((onto, files, unapplied))
    })();
    let (onto, files, unapplied) = match placed {
        Ok(placed) => placed,
        Err(err) => {
            restore_checkout(repo, &original, &original_sha)?;
            ensure_state_worktree(repo)?;
            return Err(err);
        }
    };
    let paused = PausedAdopt {
        run: run.clone(),
        original,
        original_sha,
        onto,
        deps: deps.to_vec(),
        reason: match info.kind {
            AdoptFailureKind::Apply => "it does not apply onto public upstream".into(),
            AdoptFailureKind::Preflight => "preflight.sh fails on it".into(),
        },
        files,
        unapplied,
    };
    let path = paused_path(repo)?;
    fs::write(&path, serde_json::to_string_pretty(&paused)?)?;
    Ok(paused.instructions())
}

/// Takes up a paused run as `resume` says and puts the checkout back.
/// `--abort` also removes what the run wrote.
pub fn resume_branch(repo: &Path, resume: AdoptResume) -> Result<BranchRun> {
    let path = paused_path(repo)?;
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::msg(
                "no adopt is paused; run git uplink adopt --branch <ref>",
            ));
        }
        Err(err) => return Err(err.into()),
    };
    let paused: PausedAdopt = serde_json::from_str(&raw)?;
    let mut run = paused.run.clone();
    match resume {
        AdoptResume::Abort => {}
        AdoptResume::Skip => {
            run.groups.remove(run.next);
        }
        AdoptResume::Continue => {
            assert_resolution_clean(repo)?;
            let group = &run.groups[run.next];
            commit_resolution(repo, &paused.onto, &group.title)?;
            if rev_parse(repo, "HEAD")? == paused.onto {
                eprintln!(
                    "group {} (\"{}\") is left with no change; skipping it",
                    run.next + 1,
                    group.title
                );
                run.groups.remove(run.next);
            } else {
                // The change stays the group's author's.
                if let Some(last) = group.commits.last() {
                    let author = git_ok(repo, &["log", "-1", "--format=%an <%ae>", last])?;
                    git(
                        repo,
                        &[
                            "commit",
                            "--amend",
                            "--quiet",
                            "--no-edit",
                            "--author",
                            &author,
                        ],
                        GitOpts::default(),
                    )?;
                }
                let sha = rev_parse(repo, "HEAD")?;
                git(
                    repo,
                    &[
                        "update-ref",
                        &format!("{ADOPT_AMENDED_REF_PREFIX}{sha}"),
                        &sha,
                    ],
                    GitOpts::default(),
                )?;
                run.groups[run.next].amended = Some(sha);
            }
        }
    }
    restore_checkout(repo, &paused.original, &paused.original_sha)?;
    ensure_state_worktree(repo)?;
    fs::remove_file(&path)?;
    if resume == AdoptResume::Abort {
        roll_back(repo, &run);
        return Ok(run);
    }
    write_groups_file(&run.groups_file, &run.groups)?;
    Ok(run)
}
