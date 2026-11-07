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
    Ok((updated, raw))
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
