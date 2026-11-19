use super::amend::{upstream_ref_or_company, validate_rewrite};
use super::expect::{check_expected, expect_lines, parse_expect};
use super::message::{check_message_dependencies, with_message};
use super::transfer::{PreviewOutcome, apply_queue_preview, not_previewed};
use super::*;

#[derive(Debug, Clone)]
pub struct MergeResult {
    pub queue: QueueState,
    /// The patch the others are merged into. It keeps its id.
    pub target: String,
    /// Every patch of the merge, the target included, in apply order.
    pub members: Vec<String>,
    /// False for the start, which only cuts the gated branches.
    pub completed: bool,
    pub base_branch: Option<String>,
    pub work_branch: Option<String>,
    pub onto: Option<String>,
    /// `id=stable` for the gated PR description (see [`super::expect`]).
    pub expect: Vec<String>,
    /// The merged message the start proposes: the PR title, a blank line,
    /// and the PR body.
    pub message: Option<String>,
    /// Files the start left conflict markers in, for the developer.
    pub conflicts: Vec<String>,
    /// Patches the merge made ready to submit.
    pub ready_to_submit: Vec<String>,
}

/// Starts a merge of `ids` into one patch: cuts `uplink/merge/<target>` at
/// the first of them and `-work` with all of them applied as one commit.
/// `closed` names members whose public PR is closed, so they count as
/// private.
pub fn merge_patches(repo: &Path, ids: &[String], closed: &[String]) -> Result<MergeResult> {
    with_queue_lock(repo, || start_merge(repo, ids, closed))
}

/// Folds the merged merge PR into `target` and drops the other patches
/// `expect` names.
pub fn complete_merge_with(
    repo: &Path,
    target: &str,
    expect: &[String],
    message: Option<PatchMessage>,
    preflight: &ScriptVerdict,
) -> Result<MergeResult> {
    with_queue_lock(repo, || {
        complete_merge(
            repo,
            target,
            expect,
            message.as_ref(),
            Checks::Record(preflight),
        )
        .map(Checked::recorded)
    })
}

/// What `merge --complete` would test, and what `preflight.sh` says about
/// it. The queue is left as it was; the checkout is not, so run it in a
/// clone made for it.
pub fn merge_preflight(
    repo: &Path,
    target: &str,
    expect: &[String],
    message: Option<PatchMessage>,
) -> Result<PreflightReport> {
    with_queue_lock(repo, || {
        Ok(Checked::report(complete_merge(
            repo,
            target,
            expect,
            message.as_ref(),
            Checks::Probe,
        )))
    })
}

/// The members of a merge in apply order, after checking they can be
/// merged: at least two, each active and rewritable, all in one lane, and
/// none depending on a patch between the first and the last of them (the
/// merged patch takes the first one's place, before that patch).
pub(super) fn ordered_members(queue: &QueueState, ids: &[String]) -> Result<Vec<Patch>> {
    let mut unique: Vec<&String> = Vec::new();
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.len() < 2 {
        return Err(Error::msg("A merge needs at least two patches"));
    }
    for id in &unique {
        validate_rewrite(queue, id, "merged")?;
    }
    let upstream = queue.is_upstream(unique[0]);
    if unique.iter().any(|id| queue.is_upstream(id) != upstream) {
        return Err(Error::msg(
            "Only patches in the same lane can be merged; transfer one first",
        ));
    }
    let order = apply_order_active(queue)?;
    let members: Vec<Patch> = order
        .iter()
        .filter(|p| unique.contains(&&p.id))
        .cloned()
        .collect();
    let first = order.iter().position(|p| p.id == members[0].id);
    let last = order
        .iter()
        .position(|p| p.id == members[members.len() - 1].id);
    let between: Vec<&str> = match (first, last) {
        (Some(first), Some(last)) => order[first..=last]
            .iter()
            .filter(|p| !unique.contains(&&p.id))
            .map(|p| p.id.as_str())
            .collect(),
        _ => Vec::new(),
    };
    for member in &members {
        if let Some(dep) = member
            .depends_on
            .iter()
            .find(|dep| between.contains(&dep.as_str()))
        {
            return Err(Error::msg(format!(
                "{} depends on {dep}, which comes before it but after {}; the merged patch takes {}'s place, so merge {dep} too or leave {} out",
                member.id, members[0].id, members[0].id, member.id
            )));
        }
    }
    Ok(members)
}

/// The member that keeps its id: the one with a public PR that is still
/// open, or the first. Two open public PRs are refused: the developer
/// abandons the ones not to keep first.
fn pick_target(members: &[Patch], closed: &[String]) -> Result<String> {
    let public: Vec<&Patch> = members
        .iter()
        .filter(|p| p.upstream.is_some() && !closed.contains(&p.id))
        .collect();
    match public.as_slice() {
        [] => Ok(members[0].id.clone()),
        [one] => Ok(one.id.clone()),
        many => Err(Error::msg(format!(
            "{} each have an open public PR; dispatch Uplink abandon contrib for all but the one to keep, then merge again",
            many.iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// `deps` without `members`, each once.
fn outside(deps: impl IntoIterator<Item = String>, members: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for dep in deps {
        if !members.contains(&dep) && !kept.contains(&dep) {
            kept.push(dep);
        }
    }
    kept
}

/// The message the merge PR opens with: the target's message, then each
/// other member's, without the `Uplink-Depends-On` lines, which follow once
/// for what the merged patch depends on.
pub(super) fn proposed_message(target: &Patch, members: &[Patch]) -> String {
    let ids: Vec<String> = members.iter().map(|p| p.id.clone()).collect();
    let strip = |message: &str| -> String {
        message
            .lines()
            .filter(|line| !line.trim_start().starts_with("Uplink-Depends-On:"))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    };
    let mut message = strip(&target.commit_message);
    for member in members.iter().filter(|p| p.id != target.id) {
        message.push_str("\n\n");
        message.push_str(&strip(&member.commit_message));
    }
    let deps = outside(
        members.iter().flat_map(|p| p.depends_on.iter().cloned()),
        &ids,
    );
    if !deps.is_empty() {
        message.push_str("\n\n");
        for dep in deps {
            message.push_str(&format!("Uplink-Depends-On: {dep}\n"));
        }
    }
    format!("{}\n", message.trim_end())
}

fn start_merge(repo: &Path, ids: &[String], closed: &[String]) -> Result<MergeResult> {
    let queue = read_queue_file(repo)?;
    let members = ordered_members(&queue, ids)?;
    let target_id = pick_target(&members, closed)?;
    let target = get_patch(&queue, &target_id)?.clone();
    let first = members[0].id.clone();
    let upstream_ref = upstream_ref_or_company(repo, &queue)?;
    ensure_clean_worktree(repo, "a merge")?;
    let (original, original_sha) = checkout_identity(repo)?;
    let snapshot = snapshot_uplink(repo)?;
    let message = proposed_message(&target, &members);
    let outcome = (|| -> Result<(String, String, String, Vec<String>)> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", &upstream_ref],
            GitOpts::default(),
        )?;
        let preview = match apply_queue_preview(repo, &queue, &snapshot, &first, true)? {
            PreviewOutcome::Conflict { .. } => {
                return Err(Error::msg(format!(
                    "{first} does not apply on {upstream_ref}; run sync first"
                )));
            }
            PreviewOutcome::Applied(preview) => preview,
        };
        let onto = preview.onto.ok_or_else(|| not_previewed(&first))?;
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", &onto],
            GitOpts::default(),
        )?;
        // A member that needs a patch it now comes before conflicts; the
        // markers are committed for the developer to resolve in the PR.
        let mut conflicts: Vec<String> = Vec::new();
        for member in &members {
            let file = snapshot.join(patch_path(&member.id)?);
            if apply_patch_file(repo, &queue, member, &file, false)? == ApplyOutcome::Conflict {
                let files = conflicted_files(repo)?;
                if files.is_empty() {
                    return Err(Error::msg(format!(
                        "{} does not apply after the other patches of the merge, not even with conflict markers",
                        member.id
                    )));
                }
                git(repo, &["add", "-A"], GitOpts::default())?;
                for file in files {
                    if !conflicts.contains(&file) {
                        conflicts.push(file);
                    }
                }
            }
        }
        git(repo, &["reset", "--soft", &onto], GitOpts::default())?;
        let mut proposed = target.clone();
        proposed.commit_message = strip_html_comments(&message);
        let (base, work) = cut_gated_work(
            repo,
            GateKind::Merge,
            &target_id,
            &onto,
            &company_commit_message(&queue, &proposed),
        )?;
        Ok((base, work, onto, conflicts))
    })();
    let _ = fs::remove_dir_all(&snapshot);
    let restored = restore_checkout(repo, &original, &original_sha)
        .and_then(|_| crate::repo::ensure_state_worktree(repo));
    let (base, work, onto, conflicts) = outcome?;
    restored?;
    let member_ids: Vec<String> = members.iter().map(|p| p.id.clone()).collect();
    let expect = expect_lines(&queue, &member_ids)?;
    Ok(MergeResult {
        queue,
        target: target_id,
        members: member_ids,
        completed: false,
        base_branch: Some(base),
        work_branch: Some(work),
        onto: Some(onto),
        expect,
        message: Some(message),
        conflicts,
        ready_to_submit: Vec::new(),
    })
}

fn complete_merge(
    repo: &Path,
    target: &str,
    expect: &[String],
    change: Option<&PatchMessage>,
    checks: Checks<'_>,
) -> Result<Checked<MergeResult>> {
    let base = GateKind::Merge.base_branch(target);
    let work = GateKind::Merge.work_branch(target);
    let head = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if head != base && head != work {
        return Err(Error::msg(format!(
            "Check out {base} or {work} before completing the merge into {target} (currently on {head})."
        )));
    }
    restore_uplink_from_state(repo)?;
    let mut queue = read_queue_file(repo)?;
    let before_queue = queue.clone();
    let pairs = parse_expect(expect)?;
    if !pairs.iter().any(|(id, _)| id == target) {
        return Err(Error::msg(format!(
            "Completing the merge into {target} needs --expect <id>=<stable patch id> for every patch of it, from the PR"
        )));
    }
    check_expected(&queue, &pairs)?;
    let ids: Vec<String> = pairs.iter().map(|(id, _)| id.clone()).collect();
    let members = ordered_members(&queue, &ids)?;
    let member_ids: Vec<String> = members.iter().map(|p| p.id.clone()).collect();
    assert_resolution_clean(repo)?;
    let upstream_ref = upstream_ref_or_company(repo, &queue)?;
    let onto = recover_onto(repo, GateKind::Merge, target, &head)?;

    let mut patch = get_patch(&queue, target)?.clone();
    for member in members.iter().filter(|p| p.id != target) {
        merge_authors(&mut patch.source.authors, member.source.authors.clone());
    }
    let known = patch.source.authors.len();
    merge_authors(&mut patch.source.authors, resolution_authors(repo, &onto)?);
    let mergers = patch.source.authors[known..].to_vec();
    let fallback = PatchMessage {
        message: proposed_message(&patch, &members),
    };
    let change = change.unwrap_or(&fallback);
    let (mut updated, raw_message) = with_message(&patch, Some(change))?;
    updated.depends_on = outside(updated.depends_on, &member_ids);

    let before = rev_parse(repo, "HEAD")?;
    commit_resolution(repo, &onto, &company_commit_message(&queue, &updated))?;
    if rev_parse(repo, "HEAD")? == onto {
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
        return Err(Error::msg(format!(
            "The merge leaves {target} empty; drop the patches instead"
        )));
    }

    let internal = !queue.is_upstream(target);
    let intent = PatchIntent::from_internal_only(internal);
    let checked = (|| -> Result<AssessReport> {
        check_message_dependencies(&queue, target, internal, &updated.depends_on)?;
        let report = assess_rewrite(repo, &queue, &patch, &onto, "HEAD", &raw_message, intent)?;
        if !internal {
            assert_assess_ok(&report, updated.subject())?;
        }
        Ok(report)
    })();
    let report = match checked {
        Ok(report) => report,
        Err(err) => {
            git(repo, &["reset", "--soft", &before], GitOpts::default())?;
            return Err(err);
        }
    };

    fs::create_dir_all(repo.join(".uplink/patches"))?;
    let patch_abs = repo.join(patch_path(target)?);
    crate::queue::write_atomic(&patch_abs, format_patch_at_head(repo)?)?;
    let others: Vec<String> = member_ids
        .iter()
        .filter(|id| *id != target)
        .cloned()
        .collect();
    // The target takes the place of the first member in its lane; the
    // others are dropped, and what depended on any of them depends on it.
    {
        let lane = if internal {
            &mut queue.internal
        } else {
            &mut queue.upstream
        };
        let from = lane.iter().position(|p| p.id == target);
        let to = lane.iter().position(|p| member_ids.contains(&p.id));
        if let (Some(from), Some(to)) = (from, to)
            && to < from
        {
            let moved = lane.remove(from);
            lane.insert(to, moved);
        }
    }
    {
        let current = get_patch_mut(&mut queue, target)?;
        current.commit_message = updated.commit_message.clone();
        current.source.authors = updated.source.authors.clone();
        current.depends_on = updated.depends_on.clone();
    }
    for id in &others {
        super::lifecycle::mark_dropped(&mut queue, id, &format!("Merged into {target}"))?;
    }
    for other in queue.all_patches_mut() {
        if member_ids.contains(&other.id) {
            continue;
        }
        if other.depends_on.iter().any(|dep| others.contains(dep)) {
            let deps = std::mem::take(&mut other.depends_on);
            other.depends_on = outside(
                deps.into_iter().map(|dep| {
                    if others.contains(&dep) {
                        target.to_string()
                    } else {
                        dep
                    }
                }),
                &[],
            );
        }
    }

    let verdict = checks.verdict();
    let layer_checks = if internal {
        command_preflight(&queue, repo, None, verdict)
    } else {
        export_preflight(
            repo,
            &queue,
            get_patch(&queue, target)?,
            &patch_abs,
            None,
            verdict,
        )
        .and_then(|token| assert_upstream_layer_applies(repo, &queue).map(|_| token))
    };
    if checks.is_probe() {
        let report = probe_report(layer_checks, || {
            probe_rebuild(repo, &queue, &upstream_ref, false)
        });
        restore_uplink_from_state(repo)?;
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
        return Ok(Checked::Probed(report?));
    }
    if layer_checks.is_err() {
        restore_uplink_from_state(repo)?;
        git(repo, &["reset", "--soft", &before], GitOpts::default())?;
    }
    layer_checks?;

    {
        let current = get_patch_mut(&mut queue, target)?;
        current.assess = Some(report);
        let rel = patch_path(target)?.to_string_lossy().into_owned();
        current.patch_id_stable = Some(stable_patch_id(repo, &rel)?);
        current.status = status_after_rewrite(repo, current);
        let detail = rewrite_event_detail(
            current.status,
            &with_people(
                &format!("Merged {} into this patch", others.join(", ")),
                &mergers,
            ),
        );
        add_event(current, "merged-in", detail);
    }
    write_queue_file(repo, &queue)?;
    commit_queue(
        repo,
        &format!("uplink: merge {} into {target}", others.join(", ")),
    )?;
    let queue = rebuild_checked(repo, verdict)?;
    let ready_to_submit = super::submit::newly_ready_to_submit(&before_queue, &queue);
    Ok(Checked::Recorded(MergeResult {
        queue,
        target: target.into(),
        members: member_ids,
        completed: true,
        base_branch: None,
        work_branch: None,
        onto: None,
        expect: Vec::new(),
        message: None,
        conflicts: Vec::new(),
        ready_to_submit,
    }))
}
