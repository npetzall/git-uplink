use super::*;

/// Prefix of the branches [`rescue_import`] pushes.
pub const RESCUE_PREFIX: &str = "uplink/rescue/";

/// A merged change an import could not record, and where it came from.
#[derive(Debug, Clone, Default)]
pub struct RescueOpts {
    pub from: String,
    pub head: String,
    pub pr: Option<u64>,
    pub pr_url: Option<String>,
    /// The pull request's title and body.
    pub message: Option<String>,
    /// Whatever the failed run printed, for the operator.
    pub note: Option<String>,
    pub remote: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rescued {
    pub branch: String,
    pub commit: String,
}

/// Keeps a merged change on `uplink/rescue/import-<pr>-<run>` on the
/// remote, so a failed import loses nothing.
///
/// The commit is on top of local `uplink/state` (the queue as the run saw
/// it, with whatever it recorded and could not push), or the remote's when
/// there is no local one. It adds `.uplink/rescue/<name>/` with the change
/// (`git diff --binary <from> <head>`), the message, the note and
/// `rescue.json`. Nothing else is changed, and the commit is pushed to a new
/// branch: never to `uplink/state`, and never forced.
pub fn rescue_import(repo: &Path, opts: &RescueOpts) -> Result<Rescued> {
    let name = rescue_name(opts.pr);
    let base = if has_ref(repo, STATE_BRANCH)? {
        Some(rev_parse(repo, STATE_BRANCH)?)
    } else {
        fetch_state_tracking(repo, &opts.remote, STATE_BRANCH)?
    };
    let mut change = git(
        repo,
        &["diff", "--binary", "--full-index", &opts.from, &opts.head],
        GitOpts::default(),
    )?
    .stdout;
    if !change.is_empty() {
        change.push('\n');
    }
    let info = format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({
            "from": opts.from,
            "head": opts.head,
            "pr": opts.pr,
            "prUrl": opts.pr_url,
            "runUrl": super::lifecycle::github_run_url_opt(),
            "stateBase": base,
            "at": stamp(),
        }))?
    );
    let dir = format!(".uplink/rescue/{name}");
    let paths = [
        format!("{dir}/change.patch"),
        format!("{dir}/message.md"),
        format!("{dir}/note.txt"),
        format!("{dir}/rescue.json"),
    ];
    let message = opts.message.clone().unwrap_or_default();
    let note = opts.note.clone().unwrap_or_default();
    let files = [
        (paths[0].as_str(), change.as_str()),
        (paths[1].as_str(), message.as_str()),
        (paths[2].as_str(), note.as_str()),
        (paths[3].as_str(), info.as_str()),
    ];
    let subject = match opts.pr {
        Some(pr) => format!("uplink: rescue the import of #{pr}"),
        None => "uplink: rescue an import".to_string(),
    };
    let commit = crate::repo::commit_files_on(repo, base.as_deref(), &files, &subject)?;
    let branch = format!("{RESCUE_PREFIX}{name}");
    crate::repo::push_commit_to(repo, &opts.remote, &commit, &branch)?;
    Ok(Rescued { branch, commit })
}

/// `import-<pr>-<run id>-<attempt>` in GitHub Actions, otherwise
/// `import-<pr>-<unix seconds>`.
fn rescue_name(pr: Option<u64>) -> String {
    let pr = pr.map_or_else(|| "nopr".to_string(), |pr| pr.to_string());
    let var = |key: &str| std::env::var(key).ok().filter(|value| !value.is_empty());
    let run = match (var("GITHUB_RUN_ID"), var("GITHUB_RUN_ATTEMPT")) {
        (Some(id), Some(attempt)) => format!("{id}-{attempt}"),
        (Some(id), None) => id,
        _ => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default(),
    };
    format!("import-{pr}-{run}")
}
