use super::*;

/// A new commit message for a patch, as from a merged gated PR (amend,
/// conflict or transfer): the PR title, a blank line, and the PR body.
#[derive(Debug, Clone)]
pub struct PatchMessage {
    pub message: String,
}

/// `patch` with the message of `change`, and the raw message to assess: the
/// new message, or the stored one without a change.
pub(super) fn with_message(
    patch: &Patch,
    change: Option<&PatchMessage>,
) -> Result<(Patch, String)> {
    let mut updated = patch.clone();
    let Some(change) = change else {
        return Ok((updated, patch.commit_message.clone()));
    };
    let raw = change.message.trim().to_string();
    if first_line(&strip_html_comments(&raw)).is_empty() {
        return Err(Error::msg("The patch message cannot be empty"));
    }
    updated.commit_message = strip_html_comments(&raw);
    // The gated PR body lists every dependency (see `patch_body`), so the
    // merged message replaces them, as the gate's preflight tested.
    updated.depends_on = crate::assess::parse_depends_on(&raw);
    Ok((updated, raw))
}

/// Refuses `depends_on` for `id` in its layer (internal when `internal`):
/// an unknown patch, `id` itself, a patch it cannot depend on, or a cycle.
pub(super) fn check_message_dependencies(
    queue: &QueueState,
    id: &str,
    internal: bool,
    depends_on: &[String],
) -> Result<()> {
    for dep_id in depends_on {
        if dep_id == id {
            return Err(Error::msg(format!("{id} cannot depend on itself")));
        }
        get_patch(queue, dep_id)?;
        if cannot_depend_on(queue, internal, dep_id) {
            let what = if queue.is_tooling(dep_id) {
                "tooling"
            } else {
                "internal-only"
            };
            return Err(Error::msg(format!(
                "Upstream-bound patch {id} cannot depend on {what} patch {dep_id}."
            )));
        }
    }
    let mut probe = queue.clone();
    get_patch_mut(&mut probe, id)?.depends_on = depends_on.to_vec();
    crate::queue::dependency_closure(&probe, &[id.to_string()]).map(|_| ())
}

/// With a change, rewrites HEAD's message to the new message of `updated`.
/// Without one, HEAD already has it.
pub(super) fn commit_message_change(
    repo: &Path,
    queue: &QueueState,
    updated: &Patch,
    change: Option<&PatchMessage>,
) -> Result<()> {
    if change.is_none() {
        return Ok(());
    }
    git(
        repo,
        &[
            "commit",
            "--amend",
            "--quiet",
            "-m",
            &company_commit_message(queue, updated),
        ],
        GitOpts::default(),
    )?;
    Ok(())
}

/// The message a gated PR stores on merge, beside the one it replaces.
#[derive(Debug, Clone)]
pub struct MessagePreview {
    pub id: String,
    pub gate: GateKind,
    /// The stored message, or for a merge the one its PR opened with.
    pub original: String,
    /// One per patch the merge leaves; a split has one per part.
    pub results: Vec<MessageResult>,
}

#[derive(Debug, Clone)]
pub struct MessageResult {
    /// None for a split part that gets a new id.
    pub id: Option<String>,
    pub message: String,
}

/// What `--complete` of the gated PR for `id` would store as the message:
/// `change` (the PR title and body) for every kind but a split, and for a
/// split the message of each commit of `from..head`, which must be a range
/// `--complete` takes: no merge commit, at least two. Changes nothing.
pub fn preview_message(
    repo: &Path,
    id: &str,
    gate: GateKind,
    change: Option<&PatchMessage>,
    expect: &[String],
    split: Option<(&str, &str)>,
) -> Result<MessagePreview> {
    let queue = read_queue_file(repo)?;
    let patch = get_patch(&queue, id)?;
    let (original, results) = match gate {
        GateKind::Split => {
            let Some((from, head)) = split else {
                return Err(Error::msg("A split needs --from and --head"));
            };
            // The commits `--complete` turns into parts, refused as it
            // refuses them.
            let commits = super::split::split_commits(repo, id, from, head)?;
            let mut results = Vec::new();
            for (index, sha) in commits.iter().enumerate() {
                let raw = super::split::part_message(repo, sha)?;
                results.push(MessageResult {
                    id: (index == 0).then(|| id.to_string()),
                    message: strip_html_comments(&raw),
                });
            }
            (patch.commit_message.clone(), results)
        }
        _ => {
            let Some(change) = change else {
                return Err(Error::msg(
                    "The PR title and body are needed: pass --message-file",
                ));
            };
            let original = if gate == GateKind::Merge {
                let ids: Vec<String> = super::expect::parse_expect(expect)?
                    .into_iter()
                    .map(|(member, _)| member)
                    .collect();
                if !ids.iter().any(|member| member == id) {
                    return Err(Error::msg(format!(
                        "The merge into {id} needs --expect <id>=<stable patch id> for every patch of it, from the PR"
                    )));
                }
                let members = super::merge::ordered_members(&queue, &ids)?;
                strip_html_comments(&super::merge::proposed_message(patch, &members))
            } else {
                patch.commit_message.clone()
            };
            let (updated, _) = with_message(patch, Some(change))?;
            let results = vec![MessageResult {
                id: Some(id.to_string()),
                message: updated.commit_message,
            }];
            (original, results)
        }
    };
    Ok(MessagePreview {
        id: id.to_string(),
        gate,
        original,
        results,
    })
}
