use super::amend::{upstream_ref_or_company, validate_rewrite};
use super::expect::{check_expected, expect_lines, parse_expect};
use super::message::check_message_dependencies;
use super::transfer::{PreviewOutcome, apply_queue_preview, not_previewed};
use super::*;

#[derive(Debug, Clone)]
pub struct SplitResult {
    pub queue: QueueState,
    pub id: String,
    /// False for the start, which only cuts the gated branches.
    pub completed: bool,
    pub base_branch: Option<String>,
    pub work_branch: Option<String>,
    pub onto: Option<String>,
    /// `id=stable` for the gated PR description (see [`super::expect`]).
    pub expect: Vec<String>,
    /// The patches the split ended with, in order; the first keeps the id.
    pub parts: Vec<String>,
    /// The parts together do not add and remove what the patch did.
    pub content_changed: bool,
    /// Patches the split made ready to submit.
    pub ready_to_submit: Vec<String>,
}

/// Starts a split (cuts `uplink/split/<id>` before the patch and `-work`
/// with the patch as one commit), or with `head` turns each commit of the
/// merged PR, from the base up to `head`, into a patch.
pub fn split_patch_with(
    repo: &Path,
    id: &str,
    head: Option<&str>,
    expect: &[String],
    preflight: &ScriptVerdict,
) -> Result<SplitResult> {
    with_queue_lock(repo, || match head {
        None => start_split(repo, id),
        Some(head) => {
            complete_split(repo, id, head, expect, Checks::Record(preflight)).map(Checked::recorded)
        }
    })
}

/// What `split --complete` would test, and what `preflight.sh` says about
/// it. The queue is left as it was; the checkout is not, so run it in a
/// clone made for it.
pub fn split_preflight(
    repo: &Path,
    id: &str,
    head: &str,
    expect: &[String],
) -> Result<PreflightReport> {
    with_queue_lock(repo, || {
        Ok(Checked::report(complete_split(
            repo,
            id,
            head,
            expect,
            Checks::Probe,
        )))
    })
}

fn start_split(repo: &Path, id: &str) -> Result<SplitResult> {
    let mut state = StateTree::head(repo)?;
    let queue = state.queue()?;
    validate_rewrite(&queue, id, "split")?;
    let upstream_ref = upstream_ref_or_company(repo, &queue)?;
    ensure_clean_worktree(repo, "a split")?;
    let (original, original_sha) = checkout_identity(repo)?;
    let outcome = (|| -> Result<(String, String, String)> {
        git(
            repo,
            &["checkout", "-f", "--quiet", "--detach", &upstream_ref],
            GitOpts::default(),
        )?;
        let target = match apply_queue_preview(repo, &queue, &mut state, id, true)? {
            PreviewOutcome::Conflict { .. } => {
                return Err(Error::msg(format!(
                    "{id} does not apply on {upstream_ref}; run sync first"
                )));
            }
            PreviewOutcome::Applied(target) => target,
        };
        let onto = target.onto.ok_or_else(|| not_previewed(id))?;
        let after = target.after.ok_or_else(|| not_previewed(id))?;
        if onto == after {
            return Err(Error::msg(format!(
                "{id} is already part of {upstream_ref}; nothing to split"
            )));
        }
        let base = GateKind::Split.base_branch(id);
        let work = GateKind::Split.work_branch(id);
        git(repo, &["branch", "-f", &base, &onto], GitOpts::default())?;
        git(repo, &["branch", "-f", &work, &after], GitOpts::default())?;
        Ok((base, work, onto))
    })();
    let restored = restore_checkout(repo, &original, &original_sha)
        .and_then(|_| crate::repo::ensure_state_branch(repo));
    let (base, work, onto) = outcome?;
    restored?;
    let expect = expect_lines(&queue, &[id.to_string()])?;
    Ok(SplitResult {
        queue,
        id: id.into(),
        completed: false,
        base_branch: Some(base),
        work_branch: Some(work),
        onto: Some(onto),
        expect,
        parts: Vec::new(),
        content_changed: false,
        ready_to_submit: Vec::new(),
    })
}

/// The commits of `onto..head`, oldest first. Refuses a merge commit and a
/// range of fewer than two: one commit is an amend.
fn split_commits(repo: &Path, id: &str, onto: &str, head: &str) -> Result<Vec<String>> {
    if !is_ancestor(repo, onto, head)? {
        return Err(Error::msg(format!(
            "{head} does not build on the split base of {id}; was the work branch cut by git uplink split?"
        )));
    }
    let listed = git_ok(
        repo,
        &[
            "rev-list",
            "--reverse",
            "--parents",
            &format!("{onto}..{head}"),
        ],
    )?;
    let mut commits = Vec::new();
    for line in listed.lines().filter(|line| !line.is_empty()) {
        let mut shas = line.split_whitespace();
        let sha = shas.next().unwrap_or_default().to_string();
        if shas.count() > 1 {
            return Err(Error::msg(format!(
                "The split of {id} has a merge commit ({sha}); each commit must be one part"
            )));
        }
        commits.push(sha);
    }
    if commits.len() < 2 {
        return Err(Error::msg(format!(
            "The split of {id} needs at least two commits, one per part; to change it in place, amend it"
        )));
    }
    Ok(commits)
}

/// A part's message from its commit: the commit message without the
/// `Uplink-Patch-Id` trailer the company commit had.
pub(super) fn part_message(repo: &Path, sha: &str) -> Result<String> {
    let raw = git_ok(repo, &["log", "-1", "--format=%B", sha])?;
    let kept: Vec<&str> = raw
        .lines()
        .filter(|line| !line.trim_start().starts_with("Uplink-Patch-Id:"))
        .collect();
    let message = kept.join("\n").trim().to_string();
    if first_line(&strip_html_comments(&message)).is_empty() {
        return Err(Error::msg(format!(
            "Commit {sha} has no message; each part needs a subject"
        )));
    }
    Ok(message)
}

/// The vec of the lane `id` is in.
fn lane_mut<'a>(queue: &'a mut QueueState, id: &str) -> Result<&'a mut Vec<Patch>> {
    if queue.upstream.iter().any(|p| p.id == id) {
        Ok(&mut queue.upstream)
    } else if queue.internal.iter().any(|p| p.id == id) {
        Ok(&mut queue.internal)
    } else {
        Err(Error::msg(format!("Unknown patch {id}")))
    }
}

fn complete_split(
    repo: &Path,
    id: &str,
    head: &str,
    expect: &[String],
    checks: Checks<'_>,
) -> Result<Checked<SplitResult>> {
    let base = GateKind::Split.base_branch(id);
    let work = GateKind::Split.work_branch(id);
    let branch = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch != base && branch != work {
        return Err(Error::msg(format!(
            "Check out {base} or {work} before completing the split of {id} (currently on {branch})."
        )));
    }
    let mut state = StateTree::head(repo)?;
    let mut queue = state.queue()?;
    let before_queue = queue.clone();
    let patch = validate_rewrite(&queue, id, "split")?;
    let pairs = parse_expect(expect)?;
    if pairs.len() != 1 || pairs[0].0 != id {
        return Err(Error::msg(format!(
            "Completing the split of {id} needs --expect {id}=<stable patch id> from the PR"
        )));
    }
    check_expected(&queue, &pairs)?;
    let upstream_ref = upstream_ref_or_company(repo, &queue)?;
    let onto = recover_onto(repo, GateKind::Split, id, &branch)?;
    let head = rev_parse(repo, head)?;
    let commits = split_commits(repo, id, &onto, &head)?;
    let internal = !queue.is_upstream(id);
    let intent = PatchIntent::from_internal_only(internal);
    let mut authors = patch.source.authors.clone();
    merge_authors(&mut authors, patch_authors(repo, &onto, &head)?);

    // Each commit becomes a part, squashed onto the part before it. The
    // checkout is detached throughout, so the branch is left as it was.
    let built = (|| -> Result<Vec<Patch>> {
        let mut parts: Vec<Patch> = Vec::new();
        let mut prev = onto.clone();
        for (index, sha) in commits.iter().enumerate() {
            git(
                repo,
                &["checkout", "-f", "--quiet", "--detach", sha],
                GitOpts::default(),
            )?;
            let raw = part_message(repo, sha)?;
            commit_resolution(repo, &prev, &raw)?;
            if rev_parse(repo, "HEAD")? == prev {
                return Err(Error::msg(format!(
                    "Part {} of the split of {id} ({sha}) changes nothing",
                    index + 1
                )));
            }
            let mut part = patch.clone();
            if index > 0 {
                let created_at = stamp();
                part.id = new_patch_id();
                part.status = PatchStatus::Queued;
                part.created_at = created_at.clone();
                part.updated_at = created_at;
                part.upstream = None;
                part.approvals = Vec::new();
                part.extras = None;
                part.events = Vec::new();
            }
            part.commit_message = strip_html_comments(&raw);
            part.source.authors = authors.clone();
            let mut depends_on = crate::assess::parse_depends_on(&raw);
            for dep in &patch.depends_on {
                if !depends_on.contains(dep) {
                    depends_on.push(dep.clone());
                }
            }
            if let Some(last) = parts.last()
                && !depends_on.contains(&last.id)
            {
                depends_on.push(last.id.clone());
            }
            depends_on.retain(|dep| *dep != part.id);
            part.depends_on = depends_on;
            let report = assess_rewrite(repo, &queue, &part, &prev, "HEAD", &raw, intent)?;
            if !internal {
                assert_assess_ok(&report, part.subject())?;
            }
            part.assess = Some(report);
            git(
                repo,
                &[
                    "commit",
                    "--amend",
                    "--quiet",
                    "-m",
                    &company_commit_message(&queue, &part),
                ],
                GitOpts::default(),
            )?;
            let text = format_patch_at_head(repo)?;
            part.patch_id_stable = Some(stable_patch_id_from_contents(repo, &text)?);
            state.write_patch(&part.id, text)?;
            prev = rev_parse(repo, "HEAD")?;
            parts.push(part);
        }
        Ok(parts)
    })();
    let put_back = |repo: &Path| -> Result<()> {
        git(
            repo,
            &["checkout", "-f", "--quiet", &branch],
            GitOpts::default(),
        )?;
        Ok(())
    };
    let mut parts = match built {
        Ok(parts) => parts,
        Err(err) => {
            put_back(repo)?;
            return Err(err);
        }
    };

    let combined = git_ok(repo, &["diff", "--full-index", &onto, "HEAD"])?;
    let content_changed = patch.patch_id_stable.as_deref()
        != Some(stable_patch_id_from_contents(repo, &combined)?.as_str());
    let part_ids: Vec<String> = parts.iter().map(|p| p.id.clone()).collect();
    let last_id = part_ids.last().cloned().unwrap_or_else(|| id.to_string());
    let how = format!("Split into {}", part_ids.join(", "));
    for part in parts.iter_mut() {
        if part.id == id {
            part.status = status_after_rewrite(repo, part, &state.require_patch(id)?);
            let detail = rewrite_event_detail(part.status, &how);
            add_event(part, "split", detail);
        } else {
            add_event(part, "split", format!("Split from {id}"));
        }
    }

    // The first part takes the patch's place; the others follow it. A
    // patch that depended on the whole patch depends on the last part.
    {
        let lane = lane_mut(&mut queue, id)?;
        let at = lane.iter().position(|p| p.id == id).unwrap_or(0);
        lane.splice(at..=at, parts.iter().cloned());
    }
    for other in queue.all_patches_mut() {
        if part_ids.contains(&other.id) {
            continue;
        }
        if other.depends_on.iter().any(|dep| dep == id) {
            other.depends_on.retain(|dep| dep != id && *dep != last_id);
            other.depends_on.push(last_id.clone());
        }
    }
    let checked = parts.iter().try_for_each(|part| {
        check_message_dependencies(&queue, &part.id, internal, &part.depends_on)
    });
    if let Err(err) = checked {
        put_back(repo)?;
        return Err(err);
    }

    // Each part depends on the one before it, so the last part's export
    // holds them all: one export preflight tests the whole split.
    let verdict = checks.verdict();
    let layer_checks = if internal {
        command_preflight(&queue, repo, None, verdict)
    } else {
        let last = get_patch(&queue, &last_id)?;
        export_preflight(
            &state,
            &queue,
            last,
            &state.require_patch(&last_id)?,
            None,
            verdict,
        )
        .and_then(|token| assert_upstream_layer_applies(&state, &queue).map(|_| token))
    };
    if checks.is_probe() {
        let report = probe_report(layer_checks, || {
            probe_rebuild(repo, &queue, &state, &upstream_ref, false)
        });
        put_back(repo)?;
        return Ok(Checked::Probed(report?));
    }
    if layer_checks.is_err() {
        put_back(repo)?;
    }
    layer_checks?;

    state.write_queue(&queue)?;
    state.commit_to_branch(&format!("uplink: split {id} into {}", part_ids.join(", ")))?;
    let queue = rebuild_checked(repo, verdict)?;
    let ready_to_submit = super::submit::newly_ready_to_submit(&before_queue, &queue);
    Ok(Checked::Recorded(SplitResult {
        queue,
        id: id.into(),
        completed: true,
        base_branch: None,
        work_branch: None,
        onto: None,
        expect: Vec::new(),
        parts: part_ids,
        content_changed,
        ready_to_submit,
    }))
}
