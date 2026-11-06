use super::*;

/// A new title and raw message for a patch, as from a merged gated PR
/// (amend, conflict or transfer).
#[derive(Debug, Clone)]
pub struct PatchMessage {
    pub title: String,
    pub message: Option<String>,
}

/// `patch` with the title of `change`, and the raw message to assess: the
/// new message (the title when it is empty), or the stored one without a
/// change.
pub(super) fn with_message(
    patch: &Patch,
    change: Option<&PatchMessage>,
) -> Result<(Patch, String)> {
    let mut updated = patch.clone();
    let Some(change) = change else {
        return Ok((updated, stored_commit_message(patch)));
    };
    let title = change.title.trim();
    if title.is_empty() {
        return Err(Error::msg("The patch title cannot be empty"));
    }
    updated.title = title.to_string();
    let raw = change
        .message
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(title)
        .to_string();
    Ok((updated, raw))
}

/// With a change, records the assessed message on `updated` and rewrites
/// HEAD's message to match. Without one, HEAD already has it.
pub(super) fn commit_message_change(
    repo: &Path,
    updated: &mut Patch,
    change: Option<&PatchMessage>,
    report: &AssessReport,
) -> Result<()> {
    if change.is_none() {
        return Ok(());
    }
    updated.commit_message = report.commit_message.clone();
    git(
        repo,
        &[
            "commit",
            "--amend",
            "--quiet",
            "-m",
            &company_commit_message(updated),
        ],
        GitOpts::default(),
    )?;
    Ok(())
}
