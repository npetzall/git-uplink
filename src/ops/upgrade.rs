//! What `init --upgrade` asks, and how it publishes: a preview instead of a
//! rebuild of company main, and pushes with a lease once it is answered Yes.

use std::sync::Arc;

use super::init::upgrade_preflight;
use super::*;
use crate::prompt::Choice;
use crate::tui::ChoiceWindow;
use ratatui::style::Color;

/// The branch `init --upgrade` builds company main on with the new pack.
pub const UPGRADE_PREVIEW: &str = "uplink/preview/upgrade";

/// How often the upgrade starts over when origin's `uplink/state` moves
/// before its push.
pub(super) const UPGRADE_ATTEMPTS: u32 = 3;

/// A first-parent commit on origin's company main that `uplink/state` lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrecordedCommit {
    pub sha: String,
    pub short: String,
    pub subject: String,
}

impl UnrecordedCommit {
    /// The pull request a GitHub merge commit names.
    pub fn merged_pr(&self) -> Option<&str> {
        let rest = self.subject.strip_prefix("Merge pull request #")?;
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    }
}

/// What `init --upgrade` asks.
#[derive(Debug, Clone)]
pub enum UpgradeQuestion {
    /// Local work is pending. Yes resets to origin and cleans the checkout,
    /// which throws it away, and the upgrade goes on. No refuses.
    Reset { problems: Vec<String> },
    /// Origin's company main, `main` as named in a git command, has commits
    /// `uplink/state` lacks. Adopt, Overwrite or Abort.
    Unrecorded {
        main: String,
        commits: Vec<UnrecordedCommit>,
    },
    /// Publish the upgrade? `summary` is what it changes. `changed` when it
    /// changes product code, or cannot tell.
    Publish { summary: String, changed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeAnswer {
    Yes,
    No,
    /// Queue the unrecorded commits as patches.
    Adopt,
    /// Push company main over the unrecorded commits.
    Overwrite,
    Abort,
}

/// Answers [`UpgradeQuestion`]s, in a terminal window or in a test.
pub type UpgradeAsker = Arc<dyn Fn(&UpgradeQuestion) -> Result<UpgradeAnswer> + Send + Sync>;

/// What `init --upgrade` published, or why it did not.
#[derive(Debug, Clone)]
pub enum UpgradePublish {
    /// `uplink/state` was pushed; `hooks` and `main` say what happened to
    /// them. `main` is `None` when the upgrade did not rebuild it.
    Pushed {
        hooks: HooksPushAction,
        main: Option<MainPush>,
    },
    /// Answered No, or not asked: local `uplink/state` (and `uplink/hooks`)
    /// hold the upgrade, and `preview` shows company main with it.
    Kept { preview: Option<String> },
    /// The preview could not be built; nothing was pushed.
    PreviewFailed { error: String },
    /// Aborted on `commits` of origin's company main (`main`) that
    /// `uplink/state` lacks: local `uplink/state` and `uplink/hooks` are
    /// origin's again.
    Aborted {
        main: String,
        commits: Vec<UnrecordedCommit>,
    },
}

fn ask(
    opts: &InitOpts,
    question: &UpgradeQuestion,
    unasked: UpgradeAnswer,
) -> Result<UpgradeAnswer> {
    match &opts.ask {
        Some(ask) => ask(question),
        None => Ok(unasked),
    }
}

/// Refuses an upgrade that would sweep unrelated work into its commits or
/// throw it away, unless the operator has it reset: `git uplink reset` and
/// `git clean -fd`.
pub(super) fn preflight_step(
    repo: &Path,
    opts: &InitOpts,
    progress: &mut StepProgress,
) -> Result<()> {
    let pending = progress.run_step(
        "upgrade-preflight",
        "Check nothing unrelated is pending",
        || {
            let pending = upgrade_preflight(repo)?;
            let outcome = if pending.is_empty() {
                StepOutcome::pass("nothing unrelated to the upgrade")
            } else {
                StepOutcome::warn("local work is pending")
            };
            Ok((pending, outcome))
        },
    )?;
    if pending.is_empty() {
        return Ok(());
    }
    if pending.blocker.is_some() {
        return Err(pending.refusal());
    }
    let answer = if opts.allow_reset {
        UpgradeAnswer::Yes
    } else {
        let question = UpgradeQuestion::Reset {
            problems: pending.problems.clone(),
        };
        ask(opts, &question, UpgradeAnswer::No)?
    };
    if answer != UpgradeAnswer::Yes {
        return Err(pending.refusal());
    }
    progress.run_step(
        "upgrade-reset",
        "Reset to origin and clean the checkout",
        || {
            reset_from_origin(repo)?;
            git(repo, &["clean", "-fd", "--quiet"], GitOpts::default())?;
            let left = upgrade_preflight(repo)?;
            if !left.is_empty() {
                return Err(left.refusal());
            }
            Ok(((), StepOutcome::pass("local work discarded")))
        },
    )
}

/// How [`publish`] ended.
pub(super) enum Published {
    Done(Box<InitResult>),
    /// Origin's `uplink/state` moved since the upgrade synced it: upgrade
    /// again on the state that won.
    Raced,
}

/// Moves local refs back to origin's before the upgrade runs again.
pub(super) fn start_over(repo: &Path, progress: &mut StepProgress) -> Result<()> {
    progress.run_step("upgrade-restart", "Upgrade again on origin's state", || {
        reset_from_origin(repo)?;
        delete_preview(repo)?;
        Ok((
            (),
            StepOutcome::warn(format!("origin's {STATE_BRANCH} moved")),
        ))
    })
}

fn delete_preview(repo: &Path) -> Result<()> {
    if has_ref(repo, UPGRADE_PREVIEW)? {
        git(
            repo,
            &["branch", "-D", "--quiet", UPGRADE_PREVIEW],
            GitOpts::default(),
        )?;
    }
    Ok(())
}

/// Builds the preview, asks, and publishes what the upgrade recorded.
pub(super) fn publish(
    repo: &Path,
    opts: &InitOpts,
    progress: &mut StepProgress,
    mut result: InitResult,
) -> Result<Published> {
    let remote = COMPANY_REMOTE;
    let company = result.queue.config.internal_branch.clone();
    if !git_succeeds(repo, &["remote", "get-url", remote])? {
        let kept = keep_without_remote(repo, progress, result)?;
        return Ok(Published::Done(Box::new(kept)));
    }
    let origin_state = fetch_state_tracking(repo, remote, STATE_BRANCH)?;
    let local_state = rev_parse(repo, STATE_BRANCH)?;
    if let Some(origin) = &origin_state
        && !is_ancestor(repo, origin, &local_state)?
    {
        return Ok(Published::Raced);
    }

    if !result.tooling_changed {
        let mut summary = Vec::new();
        if origin_state.as_deref() != Some(local_state.as_str()) {
            summary.push(format!("{STATE_BRANCH}: has commits origin lacks"));
        }
        if hooks_ahead(repo, remote)? {
            summary.push(format!("{HOOKS_BRANCH}: has commits origin lacks"));
        }
        if summary.is_empty() {
            return Ok(Published::Done(Box::new(result)));
        }
        let question = UpgradeQuestion::Publish {
            summary: summary.join("\n"),
            changed: false,
        };
        let answer = if opts.push_if_unchanged {
            UpgradeAnswer::Yes
        } else {
            ask(opts, &question, UpgradeAnswer::No)?
        };
        if answer != UpgradeAnswer::Yes {
            result.publish = Some(UpgradePublish::Kept { preview: None });
            return Ok(Published::Done(Box::new(finish(result, progress))));
        }
        return push_upgrade(repo, progress, result, origin_state.as_deref(), None);
    }

    if let Err(error) = build_preview(repo, progress)? {
        result.publish = Some(UpgradePublish::PreviewFailed { error });
        return Ok(Published::Done(Box::new(finish(result, progress))));
    }
    let base = origin_main(repo, remote, &company)?;
    let mut unrecorded = false;
    if let Some(base) = &base {
        let upstream = rev_parse(repo, UPSTREAM_REF)?;
        let commits = unrecorded_main_commits(repo, &base.sha, &local_state, &upstream)?;
        if !commits.is_empty() {
            // An import that just landed records them: upgrade on its state.
            if fetch_state_tracking(repo, remote, STATE_BRANCH)? != origin_state {
                return Ok(Published::Raced);
            }
            let question = UpgradeQuestion::Unrecorded {
                main: base.rev.clone(),
                commits: commits.clone(),
            };
            match ask(opts, &question, UpgradeAnswer::Abort)? {
                UpgradeAnswer::Adopt => {
                    adopt_unrecorded(repo, opts, &company, &commits)?;
                    if let Err(error) = build_preview(repo, progress)? {
                        result.publish = Some(UpgradePublish::PreviewFailed { error });
                        return Ok(Published::Done(Box::new(finish(result, progress))));
                    }
                    result.queue = read_queue_file(repo)?;
                }
                UpgradeAnswer::Overwrite => {}
                _ => {
                    progress.run_step("upgrade-abort", "Abort the upgrade", || {
                        reset_from_origin(repo)?;
                        delete_preview(repo)?;
                        Ok(((), StepOutcome::skip("nothing was pushed")))
                    })?;
                    result.queue = read_queue_file(repo)?;
                    result.publish = Some(UpgradePublish::Aborted {
                        main: base.rev.clone(),
                        commits,
                    });
                    return Ok(Published::Done(Box::new(finish(result, progress))));
                }
            }
            unrecorded = true;
        }
    }
    result.changes = match &base {
        Some(base) => Some(main_changes(repo, &result.queue, base, UPGRADE_PREVIEW)?),
        None => None,
    };
    let changed = result
        .changes
        .as_ref()
        .is_none_or(|changes| !changes.product.is_empty());
    let question = UpgradeQuestion::Publish {
        summary: format_upgrade_changes(&result),
        changed,
    };
    let answer = if opts.push_if_unchanged && !changed && !unrecorded {
        UpgradeAnswer::Yes
    } else {
        ask(opts, &question, UpgradeAnswer::No)?
    };
    if answer != UpgradeAnswer::Yes {
        result.publish = Some(UpgradePublish::Kept {
            preview: Some(UPGRADE_PREVIEW.to_string()),
        });
        return Ok(Published::Done(Box::new(finish(result, progress))));
    }
    push_upgrade(repo, progress, result, origin_state.as_deref(), Some(base))
}

/// Without a company remote there is nothing to publish to: the upgrade
/// builds the preview and compares it with company main as it is.
fn keep_without_remote(
    repo: &Path,
    progress: &mut StepProgress,
    mut result: InitResult,
) -> Result<InitResult> {
    if !result.tooling_changed {
        return Ok(result);
    }
    if let Err(error) = build_preview(repo, progress)? {
        result.publish = Some(UpgradePublish::PreviewFailed { error });
        return Ok(finish(result, progress));
    }
    let company = &result.queue.config.internal_branch;
    result.changes = match ChangeBase::of_company(repo, company)? {
        Some(base) => Some(main_changes(repo, &result.queue, &base, UPGRADE_PREVIEW)?),
        None => None,
    };
    result.publish = Some(UpgradePublish::Kept {
        preview: Some(UPGRADE_PREVIEW.to_string()),
    });
    Ok(finish(result, progress))
}

fn finish(mut result: InitResult, progress: &StepProgress) -> InitResult {
    result.report = InitReport::from_checks(progress.checks().to_vec());
    result
}

/// Local `uplink/hooks` has commits origin's lacks, or origin has none.
fn hooks_ahead(repo: &Path, remote: &str) -> Result<bool> {
    if !has_ref(repo, HOOKS_BRANCH)? {
        return Ok(false);
    }
    let local = rev_parse(repo, HOOKS_BRANCH)?;
    Ok(match fetch_state_tracking(repo, remote, HOOKS_BRANCH)? {
        None => true,
        Some(origin) => origin != local && !is_ancestor(repo, &local, &origin)?,
    })
}

/// Origin's company main as just fetched: what the preview is compared
/// with, and the commit the push is leased on.
fn origin_main(repo: &Path, remote: &str, company: &str) -> Result<Option<ChangeBase>> {
    match fetch_state_tracking(repo, remote, company)? {
        Some(_) => Ok(Some(ChangeBase::of(repo, &format!("{remote}/{company}"))?)),
        None => Ok(None),
    }
}

/// Company main as the queue now rebuilds it, on [`UPGRADE_PREVIEW`]. The
/// inner error is why it could not be built.
fn build_preview(
    repo: &Path,
    progress: &mut StepProgress,
) -> Result<std::result::Result<(), String>> {
    progress.run_step("upgrade-preview", "Build the preview", || {
        let built = rebuild_with(
            repo,
            RebuildOpts {
                branch: Some(UPGRADE_PREVIEW.to_string()),
                ..RebuildOpts::default()
            },
        );
        Ok(match built {
            Ok(_) => (Ok(()), StepOutcome::pass(UPGRADE_PREVIEW)),
            Err(err) => (Err(err.to_string()), StepOutcome::warn("not built")),
        })
    })
}

/// Queues `commits` of origin's company main as patches, one group each, in
/// the adopt window when there is a terminal.
fn adopt_unrecorded(
    repo: &Path,
    opts: &InitOpts,
    company: &str,
    commits: &[UnrecordedCommit],
) -> Result<()> {
    let branch = format!("{COMPANY_REMOTE}/{company}");
    let queue = read_queue_file(repo)?;
    let mut analysis = adopt::analyze_branch(repo, &branch, company)?;
    let wanted: HashSet<&str> = commits.iter().map(|commit| commit.sha.as_str()).collect();
    // The commits the rebuild wrote are on main already, so they start skipped.
    for commit in &mut analysis.commits {
        commit.on_main = !wanted.contains(commit.sha.as_str());
    }
    let groups = crate::tui::initial_groups(&queue, &analysis);
    let interactive = opts.interactive.unwrap_or_else(adopt::stdin_is_tty);
    let adopted = adopt_branch(
        repo,
        AdoptBranchOpts {
            branch,
            groups: Some(groups),
            apply: !interactive,
            interactive: Some(interactive),
            decide: opts.decide.clone(),
            no_preview: true,
            progress: opts.progress,
            ..AdoptBranchOpts::default()
        },
    )?;
    match adopted.adoption {
        Some(adoption) if !adoption.stopped && adopted.aborted.is_none() => Ok(()),
        _ => Err(Error::msg(format!(
            "adopting the commits did not finish; nothing was pushed. Local {STATE_BRANCH} \
holds the upgrade. Finish with git uplink adopt --continue, or discard all of it with \
git uplink reset."
        ))),
    }
}

/// Rebuilds company main when the upgrade changed the tooling, and pushes
/// `uplink/state`, `uplink/hooks` and company main, each with a lease.
/// `main` is `None` when there is no main to rebuild, and `Some(None)` when
/// origin has no company main yet.
fn push_upgrade(
    repo: &Path,
    progress: &mut StepProgress,
    mut result: InitResult,
    origin_state: Option<&str>,
    main: Option<Option<ChangeBase>>,
) -> Result<Published> {
    let remote = COMPANY_REMOTE;
    let company = result.queue.config.internal_branch.clone();
    let pushed = progress.run_step("upgrade-publish", "Publish the upgrade", || {
        if main.is_some() {
            with_queue_lock(repo, || rebuild_once(repo, None, false))?;
            if tree_of(repo, &company)? != tree_of(repo, UPGRADE_PREVIEW)? {
                return Err(Error::msg(format!(
                    "company {company} was rebuilt to another tree than {UPGRADE_PREVIEW}; \
nothing was pushed. Run git uplink init --upgrade again."
                )));
            }
        }
        // What uplink/state holds when main is pushed: state goes first.
        let recorded_on = rev_parse(repo, STATE_BRANCH)?;
        match push_branch_leased(repo, remote, STATE_BRANCH, origin_state) {
            Ok(()) => {}
            Err(err) if is_push_lease_rejected(&err) => {
                return Ok((
                    None,
                    StepOutcome::warn(format!("origin's {STATE_BRANCH} moved")),
                ));
            }
            Err(err) => return Err(err),
        }
        let hooks = push_hooks_branch(repo, remote)?;
        let main_push = match &main {
            None => None,
            Some(base) => Some(push_upgraded_main(
                repo,
                remote,
                &company,
                base.as_ref(),
                &recorded_on,
            )?),
        };
        let outcome = match main_push {
            Some(MainPush::Moved) => StepOutcome::warn(format!("origin's {company} moved")),
            _ => StepOutcome::pass(""),
        };
        Ok((
            Some(UpgradePublish::Pushed {
                hooks,
                main: main_push,
            }),
            outcome,
        ))
    })?;
    let Some(pushed) = pushed else {
        return Ok(Published::Raced);
    };
    delete_preview(repo)?;
    result.queue = read_queue_file(repo)?;
    result.publish = Some(pushed);
    Ok(Published::Done(Box::new(finish(result, progress))))
}

/// Pushes the rebuilt company main over origin's, leased on `base`: the
/// commit the operator saw the preview compared with. The operator agreed
/// to what replaces it, so the check that `uplink/state` covers it is not
/// run. Without `base` the lease is that origin has no company main.
fn push_upgraded_main(
    repo: &Path,
    remote: &str,
    company: &str,
    base: Option<&ChangeBase>,
    recorded_on: &str,
) -> Result<MainPush> {
    if let Some(base) = base {
        return push_rebuilt_main(repo, remote, None, &base.sha, recorded_on, None, true);
    }
    match push_branch_leased(repo, remote, company, None) {
        Ok(()) => Ok(MainPush::Pushed),
        Err(err) if is_push_lease_rejected(&err) => Ok(MainPush::Moved),
        Err(err) => Err(err),
    }
}

/// What an `init --upgrade` that refreshed the tooling changes: the files
/// of the pack, product code against origin's company main, and the patches
/// it dropped, amended or left overriding pack files.
pub fn format_upgrade_changes(result: &InitResult) -> String {
    let mut out = String::from("Tooling: updated");
    match &result.changes {
        Some(changes) => {
            if !changes.tooling.is_empty() {
                let _ = write!(out, ", {} changed", count_files(changes.tooling.len()));
            }
            out.push('\n');
            out.push_str(&format_product_changes(changes, UPGRADE_PREVIEW, true));
        }
        None if !matches!(
            result.publish,
            Some(UpgradePublish::Kept { .. } | UpgradePublish::Pushed { .. })
        ) => {}
        None => {
            let company = &result.queue.config.internal_branch;
            let _ = write!(out, "\nProduct code: no {company} to compare with");
        }
    }
    for (id, title) in &result.dropped {
        let _ = write!(
            out,
            "\nDropped: {id} \"{title}\" (still in .uplink/patches/{id}.patch)"
        );
    }
    for (id, title) in &result.amended {
        let _ = write!(out, "\nAmended: {id} \"{title}\"");
    }
    for patch in &result.overrides {
        let _ = write!(
            out,
            "\nOverrides tooling: {} \"{}\" ({})",
            patch.id,
            patch.title,
            patch.files.join(", ")
        );
    }
    out
}

/// The commits of an [`UpgradeQuestion::Unrecorded`], and what each answer does.
pub fn format_unrecorded(main: &str, commits: &[UnrecordedCommit]) -> String {
    let mut out = format!(
        "Origin's company main ({main}) has {} commit(s) {STATE_BRANCH} does not have. \
The upgrade's rebuild would leave them out:",
        commits.len()
    );
    for commit in commits {
        let _ = write!(out, "\n  {} {}", commit.short, commit.subject);
    }
    let prs: Vec<&str> = commits
        .iter()
        .filter_map(UnrecordedCommit::merged_pr)
        .collect();
    if !prs.is_empty() {
        let _ = write!(
            out,
            "\n\nThe import of #{} may still be running. If so, Abort and run the upgrade \
again once it is done.",
            prs.join(", #")
        );
    }
    out.push_str(
        "\n\nAdopt: queue them as patches, then publish.\n\
Overwrite: publish company main without them.\n\
Abort: push nothing; local refs go back to origin's.",
    );
    out
}

/// Asks in a terminal window with buttons, the window a failing adopt
/// group opens.
pub fn terminal_upgrade_asker() -> UpgradeAsker {
    Arc::new(|question: &UpgradeQuestion| {
        let button = |label, key| Choice { label, key };
        let yes_no = [
            (button("Yes", 'y'), UpgradeAnswer::Yes),
            (button("No", 'n'), UpgradeAnswer::No),
        ];
        let (window, options, cancel) = match question {
            UpgradeQuestion::Reset { problems } => (
                ChoiceWindow {
                    title: "Local work is pending".into(),
                    header: "A reset puts uplink/state, uplink/upstream, uplink/hooks and company \
main back to origin's, and deletes changes and untracked files in the checkout.\n\
The upgrade then goes on. No stops it, changing nothing."
                        .into(),
                    border: Color::Yellow,
                    body_title: "What would be lost".into(),
                    body: problems.join("\n\n"),
                    question: "Reset? Local changes will be lost".into(),
                    hint: None,
                    cancel_label: "no",
                },
                yes_no.to_vec(),
                UpgradeAnswer::No,
            ),
            UpgradeQuestion::Unrecorded { main, commits } => (
                ChoiceWindow {
                    title: "Company main has commits uplink/state lacks".into(),
                    header: format!(
                        "Origin's company main ({main}) has {} commit(s) no patch records.",
                        commits.len()
                    ),
                    border: Color::Red,
                    body_title: "Commits".into(),
                    body: format_unrecorded(main, commits),
                    question: "Adopt them, overwrite them, or abort?".into(),
                    hint: None,
                    cancel_label: "abort",
                },
                vec![
                    (button("Adopt", 'a'), UpgradeAnswer::Adopt),
                    (button("Overwrite", 'o'), UpgradeAnswer::Overwrite),
                    (button("Abort", 'q'), UpgradeAnswer::Abort),
                ],
                UpgradeAnswer::Abort,
            ),
            UpgradeQuestion::Publish { summary, changed } => (
                ChoiceWindow {
                    title: "Publish the upgrade".into(),
                    header: if *changed {
                        "The upgrade changes product code on company main, not only tooling.".into()
                    } else {
                        "The upgrade changes tooling only.".into()
                    },
                    border: if *changed {
                        Color::Yellow
                    } else {
                        Color::Green
                    },
                    body_title: "Changes".into(),
                    body: summary.clone(),
                    question: if *changed {
                        "Do you want to update remote with this change?".into()
                    } else {
                        "Update remote?".into()
                    },
                    hint: None,
                    cancel_label: "no",
                },
                yes_no.to_vec(),
                UpgradeAnswer::No,
            ),
        };
        crate::tui::ask(&window, &options, cancel)
    })
}
