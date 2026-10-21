//! The queue run: items added to local uplink/state one at a time, each
//! committed as soon as it passes. `init` adopts company main through it,
//! `adopt --branch` the commits of another branch, and `restack` re-adds
//! local queue items onto origin's uplink/state.
//!
//! An item that fails is put to the operator: add it with its preflight
//! failure ignored, make it internal-only, skip it, amend it inline (which
//! pauses the run the way `git rebase` stops on a commit, until
//! `--continue`), stop keeping what was added, or abort resetting
//! uplink/state. `init` adopts every commit of company main, so it can
//! neither skip a group nor stop early.
//!
//! The run is recorded in `<git-common-dir>/uplink-run.json` from its start
//! to its end. While that file exists, every other command that changes
//! uplink/state refuses to run.

mod pause;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::adopt::{
    ADOPT_TEST_BRANCH_PREFIX, AdoptGroup, AheadAnalysis, adopted_patch, clear_adopt_from,
    remove_patch_files, resolve_group,
};
use crate::assess::{company_commit_message, report_paths};
use crate::error::{Error, PreflightError, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::preflight::{
    ScriptVerdict, apply_abs, detect_depends_on, export_preflight, export_preflight_with,
    is_script_failure,
};
use crate::progress::{ProgressMode, StepOutcome, StepProgress};
use crate::queue::{
    active_upstream, add_event, patch_path, read_queue as read_queue_file,
    write_queue as write_queue_file,
};
use crate::repo::{
    TempWorktree, commit_queue, restore_paths_from, rev_parse, set_state_branch, show_at,
    stable_patch_id, write_product_patch,
};
use crate::types::{ApplyOutcome, Patch, PatchIntent, QueueState, STATE_BRANCH};

pub use pause::AdoptResume;
use pause::{PauseAt, Paused};

/// Keeps an amended group's commit from being garbage collected while the
/// groups file names it.
pub const ADOPT_AMENDED_REF_PREFIX: &str = "refs/uplink/adopt-amended/";

const RUN_FILE: &str = "uplink-run.json";

/// What a run adds to the queue, and from where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunKind {
    /// `init`: the commits company main is ahead of upstream with.
    Init,
    /// `adopt --branch`: picked commits of another branch.
    Branch,
    /// `restack`: local queue items origin's uplink/state lacks.
    Restack,
}

impl RunKind {
    /// The command that resumes the run.
    pub fn command(self) -> &'static str {
        match self {
            Self::Init | Self::Branch => "adopt",
            Self::Restack => "restack",
        }
    }

    /// `init` adopts every commit of company main, so nothing may be left
    /// out: no Skip, no Stop, no `--abort-keep`.
    pub fn can_leave_out(self) -> bool {
        self != Self::Init
    }
}

/// One patch added by a run.
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
    /// internal-only, and for a patch added with its failure ignored.
    pub preflight: Option<String>,
    /// Added although `preflight.sh` failed on it.
    pub preflight_ignored: bool,
    /// Its change was amended inline.
    pub amended: bool,
}

#[derive(Debug, Clone)]
pub struct BranchAdoption {
    pub queue: QueueState,
    pub adopted: Vec<BranchAdopted>,
    /// The operator stopped the run; what was added before stays.
    pub stopped: bool,
    /// Items left out because their change is queued already.
    pub already_queued: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptFailureKind {
    /// No set of queued upstream patches makes the item apply onto public upstream.
    Apply,
    /// `preflight.sh` fails on the item, with every set of queued patches tried.
    Preflight,
    /// The item does not apply on top of origin's queue (restack).
    Stack,
}

/// An item that cannot be added as it is.
#[derive(Debug, Clone)]
pub struct AdoptFailure {
    pub kind: AdoptFailureKind,
    /// What failed: `group 2`, or `patch upl_…` when restacking.
    pub item: String,
    /// Its number in the run, from 1.
    pub group: usize,
    /// The patch id it was tried as.
    pub id: String,
    /// Short sha of the group's last (or amended) commit; empty for a queued patch.
    pub sha: String,
    pub title: String,
    /// One line on what failed.
    pub message: String,
    /// All of it: the output of `preflight.sh`, or why it does not apply.
    pub output: String,
    /// Amending replaces the checkout, so it needs one without changes.
    pub can_amend: bool,
    /// Skip and Stop are offered.
    pub can_leave_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptDecision {
    /// Only for a preflight failure.
    IgnorePreflight,
    InternalOnly,
    /// Ignored when [`AdoptFailure::can_amend`] is false.
    Amend,
    /// Not offered by `init`.
    Skip,
    /// End the run, keeping the patches already added. Not offered by `init`.
    Stop,
    /// End the run, resetting uplink/state to where it started.
    Abort,
}

/// Says what to do with an item that fails.
pub type AdoptDecider = Arc<dyn Fn(&AdoptFailure) -> Result<AdoptDecision> + Send + Sync>;

/// A local queue item restack adds again on top of origin's.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedItem {
    pub patch: Patch,
    pub internal_only: bool,
    /// The uplink/state commit holding its patch file and reports.
    pub source: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_preflight: bool,
    /// A commit holding the change as amended inline; its diff replaces the patch file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amended: Option<String>,
}

/// What a run adds: a group of commits that becomes a new patch, or a
/// queued patch added again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RunItem {
    Group(AdoptGroup),
    Queued(Box<QueuedItem>),
}

impl RunItem {
    pub fn title(&self) -> &str {
        match self {
            Self::Group(group) => &group.title,
            Self::Queued(item) => &item.patch.title,
        }
    }

    fn internal_only(&self) -> bool {
        match self {
            Self::Group(group) => group.intent.is_internal_only(),
            Self::Queued(item) => item.internal_only,
        }
    }

    fn make_internal_only(&mut self) {
        match self {
            Self::Group(group) => group.intent = PatchIntent::InternalOnly,
            Self::Queued(item) => item.internal_only = true,
        }
    }

    fn ignore_preflight(&self) -> bool {
        match self {
            Self::Group(group) => group.ignore_preflight,
            Self::Queued(item) => item.ignore_preflight,
        }
    }

    fn set_ignore_preflight(&mut self) {
        match self {
            Self::Group(group) => group.ignore_preflight = true,
            Self::Queued(item) => item.ignore_preflight = true,
        }
    }

    fn amended(&self) -> Option<&str> {
        match self {
            Self::Group(group) => group.amended.as_deref(),
            Self::Queued(item) => item.amended.as_deref(),
        }
    }

    fn set_amended(&mut self, sha: String) {
        match self {
            Self::Group(group) => group.amended = Some(sha),
            Self::Queued(item) => item.amended = Some(sha),
        }
    }

    /// The commits a conflict pause cherry-picks one by one; none when the
    /// change is a single patch (amended, or a queued patch).
    fn commits(&self) -> &[String] {
        match self {
            Self::Group(group) if group.amended.is_none() => &group.commits,
            _ => &[],
        }
    }
}

/// The items of one run and how far it got, kept in the run file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchRun {
    pub kind: RunKind,
    /// Where the items come from: the branch adopted from, or `origin` for restack.
    pub branch: String,
    /// Where the groups are written as they are decided. None for restack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups_file: Option<PathBuf>,
    pub items: Vec<RunItem>,
    pub keep_test_branches: bool,
    /// When `preflight.sh` fails, run it again with queued upstream patches
    /// to find the ones the item needs.
    #[serde(default = "on")]
    pub preflight_deps: bool,
    /// Run `preflight.sh` on upstream-bound items.
    #[serde(default = "on")]
    pub preflight: bool,
    /// Restack: the branch holding origin's queue rebuilt onto upstream, with
    /// each item added so far applied on top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// Leave `stack` when the run ends.
    #[serde(default)]
    pub keep_stack: bool,
    /// uplink/state when the run started; `--abort` goes back to it.
    start_state: String,
    /// Index in `items` of the item added next.
    next: usize,
    /// Added to uplink/state by this run, in order.
    adopted: Vec<BranchAdopted>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    already_queued: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paused: Option<Paused>,
}

fn on() -> bool {
    true
}

impl BranchRun {
    /// A run adopting groups of commits (`init` or `adopt --branch`).
    pub fn new(
        repo: &Path,
        kind: RunKind,
        branch: String,
        groups_file: PathBuf,
        groups: Vec<AdoptGroup>,
        keep_test_branches: bool,
        preflight_deps: bool,
    ) -> Result<Self> {
        Ok(Self {
            kind,
            branch,
            groups_file: Some(groups_file),
            items: groups.into_iter().map(RunItem::Group).collect(),
            keep_test_branches,
            preflight_deps,
            preflight: true,
            stack: None,
            keep_stack: false,
            start_state: rev_parse(repo, STATE_BRANCH)?,
            next: 0,
            adopted: Vec::new(),
            already_queued: Vec::new(),
            paused: None,
        })
    }

    /// A restack run: `items` are added onto the uplink/state checked out
    /// now; `--abort` goes back to `start_state`.
    pub fn restack(
        start_state: String,
        items: Vec<QueuedItem>,
        stack: String,
        keep_stack: bool,
        preflight: bool,
        preflight_deps: bool,
    ) -> Self {
        Self {
            kind: RunKind::Restack,
            branch: crate::repo::COMPANY_REMOTE.to_string(),
            groups_file: None,
            items: items
                .into_iter()
                .map(|item| RunItem::Queued(Box::new(item)))
                .collect(),
            keep_test_branches: false,
            preflight_deps,
            preflight,
            stack: Some(stack),
            keep_stack,
            start_state,
            next: 0,
            adopted: Vec::new(),
            already_queued: Vec::new(),
            paused: None,
        }
    }

    /// The groups of a group run, as the groups file holds them.
    pub fn groups(&self) -> Vec<AdoptGroup> {
        self.items
            .iter()
            .filter_map(|item| match item {
                RunItem::Group(group) => Some(group.clone()),
                RunItem::Queued(_) => None,
            })
            .collect()
    }

    fn write_groups(&self) -> Result<()> {
        match &self.groups_file {
            Some(path) => write_groups_file(path, &self.groups()),
            None => Ok(()),
        }
    }

    fn save(&self, repo: &Path) -> Result<()> {
        fs::write(run_path(repo)?, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// `group 2`, or `patch upl_…` for a queued patch.
    fn item_label(&self, index: usize) -> String {
        match &self.items[index] {
            RunItem::Group(_) => format!("group {}", index + 1),
            RunItem::Queued(item) => format!("patch {}", item.patch.id),
        }
    }

    /// What the run is, for messages: `adopt`, or `restack`.
    fn noun(&self) -> &'static str {
        self.kind.command()
    }

    /// `an adopt`, or `a restack`.
    fn a_noun(&self) -> &'static str {
        match self.kind {
            RunKind::Init | RunKind::Branch => "an adopt",
            RunKind::Restack => "a restack",
        }
    }

    /// The commands that end the run, one per line.
    fn ways_out(&self) -> String {
        let cmd = self.kind.command();
        let mut lines = vec![format!(
            "Stop, resetting uplink/state to the start:  git uplink {cmd} --abort"
        )];
        if self.kind.can_leave_out() {
            lines.insert(
                0,
                format!(
                    "Stop, keeping the {} patch(es) added so far:  git uplink {cmd} --abort-keep",
                    self.adopted.len()
                ),
            );
        }
        lines.join("\n")
    }

    /// Why other commands refuse to run now.
    fn blocking_message(&self, repo: &Path) -> String {
        if let Some(paused) = &self.paused {
            return format!(
                "{} is paused; finish it first.\n{}",
                self.a_noun(),
                paused.instructions(self)
            );
        }
        let cmd = self.kind.command();
        let skip = if self.kind.can_leave_out() {
            format!("\nLeave out the item it was on:  git uplink {cmd} --skip")
        } else {
            String::new()
        };
        format!(
            "{} run is in progress ({} of {} item(s) done), so uplink/state is locked.\n\
If no git uplink {cmd} is running (it was interrupted), take it up again:  git uplink {cmd} --continue{skip}\n{}\nRun file: {}",
            self.a_noun(),
            self.next.min(self.items.len()),
            self.items.len(),
            self.ways_out(),
            run_path(repo)
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )
    }
}

fn common_dir(repo: &Path) -> Result<PathBuf> {
    Ok(repo.join(git_ok(repo, &["rev-parse", "--git-common-dir"])?))
}

fn run_path(repo: &Path) -> Result<PathBuf> {
    Ok(common_dir(repo)?.join(RUN_FILE))
}

/// The run in progress, if any.
pub fn read_run(repo: &Path) -> Result<Option<BranchRun>> {
    match fs::read_to_string(run_path(repo)?) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn remove_run(repo: &Path) -> Result<()> {
    match fs::remove_file(run_path(repo)?) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
        _ => Ok(()),
    }
}

/// Why uplink/state is locked by a run, or None. Never fails: a directory
/// that is not a repository has no run.
pub fn blocking_run(repo: &Path) -> Option<String> {
    let path = run_path(repo).ok()?;
    if !path.is_file() {
        return None;
    }
    let message = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<BranchRun>(&raw).ok())
        .map(|run| run.blocking_message(repo))
        .unwrap_or_else(|| {
            format!(
                "a queue run is in progress, so uplink/state is locked; see {}",
                path.display()
            )
        });
    Some(message)
}

/// Refuses while a run is in progress, naming how to end it.
pub fn refuse_while_running(repo: &Path) -> Result<()> {
    match blocking_run(repo) {
        Some(message) => Err(Error::msg(message)),
        None => Ok(()),
    }
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

/// Where the groups adopted from `branch` are written by default.
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

/// The line of the item being added: a spinner while it is checked, then
/// its outcome, and a line for each decision taken on it.
struct Lines {
    progress: StepProgress,
    label: String,
    spinning: bool,
}

impl Lines {
    fn start(&mut self, label: String) {
        self.label = label;
        self.progress.start(&self.label);
        self.spinning = true;
    }

    /// Ends the spinner with `outcome`, or adds a line under it.
    fn line(&mut self, outcome: StepOutcome) {
        self.progress.finish(&self.label, &outcome);
        self.spinning = false;
    }
}

/// What a run needs besides its items.
struct Ctx<'a> {
    repo: &'a Path,
    /// The commits groups are resolved against. None for restack.
    analysis: Option<&'a AheadAnalysis>,
    verdict: &'a ScriptVerdict,
    decide: Option<&'a AdoptDecider>,
    lines: Lines,
}

/// Adds the items of `run` from where it got to, one at a time, each in
/// its own uplink/state commit, keeping the run file current. The caller
/// holds the run lock ([`crate::lock::with_run_lock`]). A run that pauses
/// returns its instructions as the error and keeps what it added; any
/// other error resets uplink/state to where the run started and ends it.
pub fn run_branch(
    repo: &Path,
    analysis: Option<&AheadAnalysis>,
    mut run: BranchRun,
    verdict: &ScriptVerdict,
    decide: Option<&AdoptDecider>,
    progress: ProgressMode,
) -> Result<BranchAdoption> {
    let mut ctx = Ctx {
        repo,
        analysis,
        verdict,
        decide,
        lines: Lines {
            progress: StepProgress::from_mode(progress),
            label: String::new(),
            spinning: false,
        },
    };
    run.paused = None;
    let outcome = (|| -> Result<Ended> {
        run.save(repo)?;
        while run.next < run.items.len() {
            match adopt_next(&mut ctx, &mut run)? {
                Step::Added | Step::AlreadyQueued => run.next += 1,
                Step::Again => run.write_groups()?,
                Step::Skip => {
                    run.items.remove(run.next);
                    run.write_groups()?;
                }
                Step::Paused(paused) => {
                    let instructions = paused.instructions(&run);
                    run.paused = Some(*paused);
                    return Ok(Ended::Paused(instructions));
                }
                Step::Stop => return Ok(Ended::Stopped),
            }
            run.save(repo)?;
        }
        if run.adopted.is_empty() && run.kind != RunKind::Restack {
            return Err(Error::msg("every group was skipped; nothing to adopt"));
        }
        Ok(Ended::Done)
    })();
    match outcome {
        Ok(Ended::Paused(instructions)) => {
            run.save(repo)?;
            Err(Error::msg(instructions))
        }
        Ok(ended) => {
            end_run(repo, &run, matches!(ended, Ended::Done))?;
            Ok(BranchAdoption {
                queue: read_queue_file(repo)?,
                adopted: run.adopted,
                stopped: matches!(ended, Ended::Stopped),
                already_queued: run.already_queued,
            })
        }
        Err(err) => {
            if ctx.lines.spinning {
                let first = err.to_string();
                let first = first.lines().next().unwrap_or("").to_string();
                ctx.lines.line(StepOutcome::fail(first));
            }
            reset_state(repo, &run)?;
            Err(err)
        }
    }
}

enum Ended {
    Done,
    Stopped,
    Paused(String),
}

/// Ends a run that kept what it added: an `init` that adopted every group
/// clears `uplink/adopt-from`, a restack removes its stack branch unless
/// asked to keep it, and the run file goes.
fn end_run(repo: &Path, run: &BranchRun, done: bool) -> Result<()> {
    if done && run.kind == RunKind::Init {
        clear_adopt_from(repo)?;
    }
    remove_stack(repo, run);
    remove_run(repo)
}

fn remove_stack(repo: &Path, run: &BranchRun) {
    if let Some(stack) = run.stack.as_deref().filter(|_| !run.keep_stack) {
        let _ = git(repo, &["branch", "-D", stack], GitOpts::allow_fail());
    }
}

/// Puts uplink/state back to where `run` started, removing the patch files
/// and test branches it added, and ends the run.
fn reset_state(repo: &Path, run: &BranchRun) -> Result<()> {
    if run.kind != RunKind::Restack {
        // A restack re-adds patches the start state has, files and all.
        let ids: Vec<String> = run.adopted.iter().map(|a| a.id.clone()).collect();
        remove_patch_files(repo, &ids);
    }
    for branch in run.adopted.iter().filter_map(|a| a.test_branch.as_deref()) {
        let _ = git(repo, &["branch", "-D", branch], GitOpts::allow_fail());
    }
    set_state_branch(repo, STATE_BRANCH, &run.start_state)?;
    remove_stack(repo, run);
    remove_run(repo)
}

enum Step {
    /// Added to the queue and committed.
    Added,
    /// An equal patch is queued already.
    AlreadyQueued,
    /// The item was changed (made internal-only); add it again.
    Again,
    Skip,
    Stop,
    Paused(Box<Paused>),
}

/// The candidate patch of the next item, written to its patch file.
struct Candidate {
    patch: Patch,
    abs: PathBuf,
    /// Short sha of the commit holding the change; empty for a queued patch.
    sha: String,
}

fn candidate(ctx: &Ctx<'_>, run: &BranchRun, queue: &QueueState) -> Result<Candidate> {
    let repo = ctx.repo;
    let index = run.next;
    match &run.items[index] {
        RunItem::Group(group) => {
            let analysis = ctx
                .analysis
                .ok_or_else(|| Error::msg("an adopt group needs the commits it is from"))?;
            let mut resolved = resolve_group(
                analysis,
                group,
                index,
                &mut vec![None; analysis.commits.len()],
            )?;
            if let Some(sha) = &group.amended {
                resolved.head_sha = rev_parse(repo, sha)?;
                resolved.from_sha = rev_parse(repo, &format!("{sha}^"))?;
            }
            let source = (run.kind == RunKind::Branch).then_some(run.branch.as_str());
            let patch = adopted_patch(repo, queue, &resolved, None, source)?;
            write_product_patch(
                repo,
                &patch.id,
                &resolved.from_sha,
                &company_commit_message(&patch),
                &resolved.head_sha,
            )?;
            Ok(Candidate {
                abs: repo.join(patch_path(&patch.id)?),
                sha: resolved.head_sha.chars().take(7).collect(),
                patch,
            })
        }
        RunItem::Queued(item) => {
            let mut patch = item.patch.clone();
            // Kept only when the queue still has them; the rest are found again.
            patch.depends_on.retain(|id| queue.layer_of(id).is_some());
            let rel = patch_path(&patch.id)?;
            match &item.amended {
                Some(sha) => {
                    write_product_patch(
                        repo,
                        &patch.id,
                        &format!("{sha}^"),
                        &company_commit_message(&patch),
                        sha,
                    )?;
                }
                None => {
                    let text = show_at(repo, &item.source, &rel.to_string_lossy())?;
                    let abs = repo.join(&rel);
                    if let Some(parent) = abs.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&abs, format!("{text}\n"))?;
                }
            }
            Ok(Candidate {
                abs: repo.join(rel),
                sha: item
                    .amended
                    .as_deref()
                    .map(|sha| sha.chars().take(7).collect())
                    .unwrap_or_default(),
                patch,
            })
        }
    }
}

/// Adds `run.items[run.next]`, or says what happens to it instead.
fn adopt_next(ctx: &mut Ctx<'_>, run: &mut BranchRun) -> Result<Step> {
    let repo = ctx.repo;
    let queue = read_queue_file(repo)?;
    let index = run.next;
    let internal_only = run.items[index].internal_only();
    let Candidate {
        mut patch,
        abs,
        sha,
    } = candidate(ctx, run, &queue)?;
    let id = patch.id.clone();
    let label = if sha.is_empty() {
        format!("{id} - {}", patch.title)
    } else {
        format!("{id} - {sha} - {}", patch.title)
    };
    ctx.lines.start(label);
    let test_branch = (run.keep_test_branches && !internal_only)
        .then(|| format!("{ADOPT_TEST_BRANCH_PREFIX}{}-{id}", index + 1));
    let mut done = BranchAdopted {
        id: id.clone(),
        title: patch.title.clone(),
        intent: if internal_only {
            PatchIntent::InternalOnly
        } else {
            PatchIntent::Upstream
        },
        depends_on: Vec::new(),
        test_branch: None,
        preflight: None,
        preflight_ignored: false,
        amended: run.items[index].amended().is_some(),
    };
    let checked = check_item(
        ctx,
        &queue,
        run,
        &mut patch,
        &mut done,
        &abs,
        &sha,
        test_branch.as_deref(),
    );
    let step = match checked {
        Ok(Checked::Add { stack_tip }) => {
            add(ctx, queue, run, patch, done, internal_only, stack_tip).map(|()| Step::Added)
        }
        Ok(Checked::Instead(step)) => Ok(step),
        Err(err) => Err(err),
    };
    if !matches!(step, Ok(Step::Added)) {
        if run.kind == RunKind::Restack {
            // The start state keeps the file; the queue checked out now has no entry for it.
            let _ = fs::remove_file(&abs);
        } else {
            remove_patch_files(repo, &[id]);
        }
        if let Some(branch) = &test_branch {
            let _ = git(repo, &["branch", "-D", branch], GitOpts::allow_fail());
        }
    }
    step
}

/// Adds the checked patch to the queue in its own uplink/state commit.
fn add(
    ctx: &mut Ctx<'_>,
    mut queue: QueueState,
    run: &mut BranchRun,
    mut patch: Patch,
    done: BranchAdopted,
    internal_only: bool,
    stack_tip: Option<String>,
) -> Result<()> {
    let repo = ctx.repo;
    let id = patch.id.clone();
    let message = match &run.items[run.next] {
        RunItem::Group(_) => format!("uplink: adopt {id} from {}", run.branch),
        RunItem::Queued(item) => {
            // Reports and extras travel with the patch.
            let (reports, _, _) = report_paths(&id)?;
            let listed = git_ok(
                repo,
                &["ls-tree", "-r", "--name-only", &item.source, "--", &reports],
            )?;
            let paths: Vec<String> = listed
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect();
            restore_paths_from(repo, &item.source, &paths)?;
            add_event(
                &mut patch,
                "restacked",
                "Added again on top of origin's uplink/state",
            );
            format!("uplink: restack {id} onto {}", run.branch)
        }
    };
    queue.push_patch(patch, internal_only);
    write_queue_file(repo, &queue)?;
    commit_queue(repo, &message)?;
    if let (Some(stack), Some(tip)) = (&run.stack, &stack_tip) {
        git(
            repo,
            &["update-ref", &format!("refs/heads/{stack}"), tip],
            GitOpts::default(),
        )?;
    }
    let mut detail = vec!["added".to_string()];
    if internal_only {
        detail.push("internal-only".into());
    }
    if !done.depends_on.is_empty() {
        detail.push(format!("depends on {}", done.depends_on.join(", ")));
    }
    if done.amended {
        detail.push("amended".into());
    }
    if done.preflight_ignored {
        detail.push("preflight failure ignored".into());
    }
    ctx.lines.line(StepOutcome::pass(detail.join(", ")));
    run.adopted.push(done);
    Ok(())
}

enum Checked {
    /// Add it; a restack moves its stack branch to `stack_tip`.
    Add {
        stack_tip: Option<String>,
    },
    Instead(Step),
}

/// What applying the candidate on top of the restack branch gives.
enum OnStack {
    Applied(String),
    Empty,
    Conflict,
}

fn apply_on_stack(repo: &Path, stack: &str, candidate_abs: &Path, title: &str) -> Result<OnStack> {
    let worktree = TempWorktree::add(repo, "uplink-restack", stack)?;
    Ok(match apply_abs(&worktree.dir, candidate_abs, title)? {
        ApplyOutcome::Applied => OnStack::Applied(rev_parse(&worktree.dir, "HEAD")?),
        ApplyOutcome::Empty => OnStack::Empty,
        ApplyOutcome::Conflict => OnStack::Conflict,
    })
}

/// Checks the written patch of the next item.
#[allow(clippy::too_many_arguments)]
fn check_item(
    ctx: &mut Ctx<'_>,
    queue: &QueueState,
    run: &mut BranchRun,
    patch: &mut Patch,
    done: &mut BranchAdopted,
    candidate_abs: &Path,
    sha: &str,
    test_branch: Option<&str>,
) -> Result<Checked> {
    let repo = ctx.repo;
    let index = run.next;
    let item = run.items[index].clone();
    let rel = patch_path(&patch.id)?;
    patch.patch_id_stable = Some(stable_patch_id(repo, &rel.to_string_lossy())?);
    if let Some(existing) = queue.all_patches().find(|other| {
        other.patch_id_stable.is_some() && other.patch_id_stable == patch.patch_id_stable
    }) {
        ctx.lines.line(StepOutcome::skip(format!(
            "already queued as {} \"{}\"",
            existing.id, existing.title
        )));
        run.already_queued.push(patch.id.clone());
        return Ok(Checked::Instead(Step::AlreadyQueued));
    }
    if item.amended().is_some() {
        add_event(patch, "amended", "Amended inline");
    }
    let patch_id = patch.id.clone();
    let item_label = run.item_label(index);
    let can_leave_out = run.kind.can_leave_out();
    let failure = |kind, message: String, output: String| AdoptFailure {
        kind,
        item: item_label.clone(),
        group: index + 1,
        id: patch_id.clone(),
        sha: sha.to_string(),
        title: item.title().to_string(),
        message,
        output,
        can_amend: false,
        can_leave_out,
    };

    let mut stack_tip = None;
    if let Some(stack) = run.stack.clone() {
        match apply_on_stack(repo, &stack, candidate_abs, &patch.title)? {
            OnStack::Applied(tip) => stack_tip = Some(tip),
            OnStack::Empty => {
                ctx.lines
                    .line(StepOutcome::skip("its change is in origin's queue already"));
                run.already_queued.push(patch.id.clone());
                return Ok(Checked::Instead(Step::AlreadyQueued));
            }
            OnStack::Conflict => {
                let info = failure(
                    AdoptFailureKind::Stack,
                    "does not apply on top of origin's queue".into(),
                    format!(
                        "\"{}\" does not apply on {stack}: origin's queue on public upstream, with the patches added again so far.",
                        patch.title
                    ),
                );
                let at = PauseAt::Stack(stack);
                return on_failure(ctx, queue, run, info, None, candidate_abs, &at)
                    .map(|step| Checked::Instead(step.unwrap_or(Step::Again)));
            }
        }
    }
    if item.internal_only() {
        return Ok(Checked::Add { stack_tip });
    }

    let Some(found) = detect_depends_on(repo, queue, candidate_abs, &patch.title, test_branch)?
    else {
        let info = failure(
            AdoptFailureKind::Apply,
            "does not apply onto public upstream".into(),
            format!(
                "\"{}\" does not apply onto public upstream, alone or on top of the queued upstream patches.",
                item.title()
            ),
        );
        let all: Vec<String> = active_upstream(queue)
            .into_iter()
            .map(|p| p.id.clone())
            .collect();
        let at = PauseAt::Upstream(all);
        return on_failure(ctx, queue, run, info, None, candidate_abs, &at)
            .map(|step| Checked::Instead(step.unwrap_or(Step::Again)));
    };
    done.test_branch = test_branch.map(str::to_string);
    if !found.is_empty() {
        add_event(
            patch,
            "depends-on",
            format!("Needs {} to apply onto public upstream", found.join(", ")),
        );
    }
    let mut deps = patch.depends_on.clone();
    for id in found {
        if !deps.contains(&id) {
            deps.push(id);
        }
    }
    patch.depends_on = deps;
    if run.preflight {
        let failed = match export_preflight_with(
            repo,
            queue,
            patch,
            candidate_abs,
            None,
            ctx.verdict,
            run.preflight_deps,
        ) {
            Ok(token) => {
                done.preflight = token;
                None
            }
            Err(Error::Preflight(err)) if is_script_failure(&err) => {
                with_preflight_deps(repo, queue, patch, done, candidate_abs, ctx.verdict, err)?
            }
            Err(err) => return Err(err),
        };
        if let Some(err) = failed {
            if !item.ignore_preflight() {
                let info = failure(
                    AdoptFailureKind::Preflight,
                    "preflight.sh failed on public upstream".into(),
                    err.to_string(),
                );
                let at = PauseAt::Upstream(patch.depends_on.clone());
                if let Some(step) =
                    on_failure(ctx, queue, run, info, Some(err), candidate_abs, &at)?
                {
                    return Ok(Checked::Instead(step));
                }
            }
            ignore_failure(queue, patch, done);
        }
    }
    done.depends_on = patch.depends_on.clone();
    Ok(Checked::Add { stack_tip })
}

/// When `preflight.sh` passes with queued upstream patches the item does
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

/// Adds the patch although `preflight.sh` fails on it. It is exported
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
        format!("Added although preflight.sh failed on public upstream{after}"),
    );
    done.preflight_ignored = true;
}

/// Shows the failure, asks what to do with the item and does it. `None`
/// when the preflight failure is to be ignored.
fn on_failure(
    ctx: &mut Ctx<'_>,
    queue: &QueueState,
    run: &mut BranchRun,
    mut info: AdoptFailure,
    error: Option<PreflightError>,
    candidate_abs: &Path,
    at: &PauseAt,
) -> Result<Option<Step>> {
    let repo = ctx.repo;
    let index = run.next;
    ctx.lines.line(StepOutcome::fail(info.message.clone()));
    info.can_amend = pause::checkout_clean(repo)?;
    let Some(decide) = ctx.decide else {
        return Err(unattended(run, &info, error));
    };
    let decision = decide(&info)?;
    let leave_out = run.kind.can_leave_out();
    let step = match decision {
        AdoptDecision::IgnorePreflight if info.kind == AdoptFailureKind::Preflight => {
            run.items[index].set_ignore_preflight();
            run.write_groups()?;
            ctx.lines
                .line(StepOutcome::warn("ignoring the preflight failure"));
            return Ok(None);
        }
        AdoptDecision::InternalOnly => {
            run.items[index].make_internal_only();
            ctx.lines
                .line(StepOutcome::warn("made internal-only; adding it again"));
            Step::Again
        }
        AdoptDecision::Amend if info.can_amend => {
            ctx.lines.line(StepOutcome::warn("paused to amend"));
            Step::Paused(Box::new(pause::pause(
                repo,
                queue,
                run,
                &info,
                candidate_abs,
                at,
            )?))
        }
        AdoptDecision::Skip if leave_out => {
            ctx.lines.line(StepOutcome::skip("skipped"));
            Step::Skip
        }
        AdoptDecision::Stop if leave_out => {
            run.write_groups()?;
            ctx.lines.line(StepOutcome::skip(format!(
                "stopped; the {} patch(es) added so far are kept",
                run.adopted.len()
            )));
            Step::Stop
        }
        _ => {
            ctx.lines
                .line(StepOutcome::fail("aborted; uplink/state is reset"));
            let groups = run
                .groups_file
                .as_ref()
                .map(|path| format!("\nGroups file: {}", path.display()))
                .unwrap_or_default();
            return Err(Error::msg(format!(
                "{} aborted on {} (\"{}\"); uplink/state is back where it started.{groups}",
                run.noun(),
                info.item,
                info.title,
            )));
        }
    };
    Ok(Some(step))
}

/// The error for a failing item when nobody can be asked.
fn unattended(run: &BranchRun, info: &AdoptFailure, error: Option<PreflightError>) -> Error {
    let cmd = run.kind.command();
    let hint = match &run.groups_file {
        Some(path) => {
            let file = path.display();
            let n = info.group;
            let rerun = if run.kind == RunKind::Init {
                format!("re-run git uplink init with --adopt-groups {file}")
            } else {
                format!("re-run with --groups {file}")
            };
            match info.kind {
                AdoptFailureKind::Preflight => format!(
                    "To adopt it anyway, set \"ignorePreflight\": true (or \"intent\": \"internal-only\") on group {n} in {file} \
and {rerun}, or run git uplink {cmd} in a terminal to amend it."
                ),
                AdoptFailureKind::Apply | AdoptFailureKind::Stack => format!(
                    "Rebase those commits onto current upstream, set \"intent\": \"internal-only\" on group {n} in {file} \
and {rerun}, or run git uplink {cmd} in a terminal to amend it."
                ),
            }
        }
        None => format!("Run git uplink {cmd} in a terminal to amend, skip or ignore it."),
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
        None => Error::msg(format!("{} {hint}", info.output)),
    }
}

/// Takes up the run in progress as `resume` says: settles a pause and puts
/// the checkout back. `--abort` resets uplink/state to where the run
/// started and `--abort-keep` keeps what it added; both end the run.
/// Otherwise the run is returned to go on.
pub fn resume_branch(repo: &Path, resume: AdoptResume) -> Result<BranchRun> {
    let Some(mut run) = read_run(repo)? else {
        return Err(Error::msg(
            "no adopt or restack run is in progress; start one with git uplink adopt --branch <ref> or git uplink restack",
        ));
    };
    if !run.kind.can_leave_out() && matches!(resume, AdoptResume::Skip | AdoptResume::AbortKeep) {
        return Err(Error::msg(format!(
            "init adopts every commit of company main, so it cannot leave one out.\n\
Continue:  git uplink {cmd} --continue\n\
Abort:     git uplink {cmd} --abort",
            cmd = run.kind.command()
        )));
    }
    if let Some(paused) = run.paused.clone() {
        if resume == AdoptResume::Continue
            && let Some(again) = pause::continue_paused(repo, &mut run, &paused)?
        {
            // Stopped again on a commit still to pick.
            let instructions = again.instructions(&run);
            run.paused = Some(again);
            run.save(repo)?;
            return Err(Error::msg(instructions));
        }
        paused.restore(repo)?;
        run.paused = None;
    }
    match resume {
        AdoptResume::Abort => {
            reset_state(repo, &run)?;
            return Ok(run);
        }
        AdoptResume::AbortKeep => {
            end_run(repo, &run, false)?;
            run.write_groups()?;
            return Ok(run);
        }
        AdoptResume::Skip if run.next < run.items.len() => {
            run.items.remove(run.next);
        }
        _ => {}
    }
    run.save(repo)?;
    run.write_groups()?;
    Ok(run)
}
