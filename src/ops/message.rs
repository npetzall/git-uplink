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
