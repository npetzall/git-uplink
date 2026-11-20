use std::fs;
use std::path::Path;

use crate::assess::{append_patch_id_trailer, assess_from_message, strip_html_comments};
use crate::embedded::{PACK_GITHUB, PACK_GITHUB_HOOKS, PACK_TRY_IT_ON_GITHUB};
use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::queue::add_event;
use crate::repo::{
    TempWorktree, ensure_upstream_ref, has_ref, new_patch_id, patch_substance,
    stable_patch_id_from_contents, stamp,
};
use crate::state::StateTree;
use crate::types::{
    AssessReport, DEFAULT_CUTOFF, Forge, ForgeFamily, Patch, PatchIntent, PatchSource, PatchStatus,
    QueueState, TOOLING_PATCH_KIND, TOOLING_PATCH_TITLE,
};

/// The action every job installs the binary with, and the placeholder in it
/// that `composed_files` replaces with this binary's version: pack and binary
/// are one release.
const INSTALL_ACTION: &str = ".github/actions/install-git-uplink/action.yml";
const VERSION_PLACEHOLDER: &str = "@GIT_UPLINK_VERSION@";

pub struct ToolingRefresh {
    pub changed: bool,
}

/// A tooling patch synthesized from the embedded pack that differs from the
/// stored one, not written yet.
pub struct ToolingPlan {
    pub id: String,
    /// The queue has no tooling patch yet.
    pub created: bool,
    /// The patch file.
    pub formatted: String,
    forge: Forge,
    message: String,
    from_sha: String,
    head_sha: String,
}

/// Synthesizes the tooling patch from the embedded pack. `None` when the
/// stored patch already holds it. Writes nothing.
pub fn plan_tooling_refresh(repo: &Path) -> Result<Option<ToolingPlan>> {
    ensure_upstream_ref(repo)?;
    if !has_ref(repo, "uplink/upstream")? {
        return Err(Error::msg(
            "uplink/upstream is missing; cannot install forge tooling until upstream is seeded.",
        ));
    }
    let state = StateTree::head(repo)?;
    let queue = state.queue()?;
    let forge = queue.config.forge.ok_or_else(|| {
        Error::msg(
            "queue.json has no forge. Re-run `git uplink init --upgrade --forge github` \
(or --forge try-it-on-github) to record it.",
        )
    })?;
    let files = composed_files(forge)?;
    if files.is_empty() {
        return Err(Error::msg(format!(
            "embedded forge pack for {forge} is empty"
        )));
    }

    let existing_id = find_tooling_patch(&queue, &state)?;
    let (id, created) = if let Some(id) = existing_id {
        (id, false)
    } else {
        (new_patch_id(), true)
    };
    let message = tooling_commit_message();
    let file_message = append_patch_id_trailer(&strip_html_comments(&message), &id);
    let SynthesizedPatch {
        formatted,
        from_sha,
        head_sha,
    } = synthesize_pack_patch(repo, &files, &file_message)?;

    if !created {
        let stored = state.patch(&id)?.unwrap_or_default();
        if patch_substance(&stored) == patch_substance(&formatted) {
            return Ok(None);
        }
    }
    Ok(Some(ToolingPlan {
        id,
        created,
        formatted,
        forge,
        message,
        from_sha,
        head_sha,
    }))
}

/// Commits `plan` on uplink/state, together with what `also` changes in the
/// queue and the state.
pub fn commit_tooling_refresh(
    repo: &Path,
    plan: ToolingPlan,
    also: impl FnOnce(&mut StateTree, &mut QueueState) -> Result<()>,
) -> Result<()> {
    let ToolingPlan {
        id,
        created,
        formatted,
        forge,
        message,
        from_sha,
        head_sha,
    } = plan;
    let new_stable = stable_patch_id_from_contents(repo, &formatted)?;
    let mut state = StateTree::head(repo)?;
    let mut queue = state.queue()?;
    state.write_patch(&id, formatted)?;

    let assess = assess_from_message(
        repo,
        &queue,
        &from_sha,
        &head_sha,
        &message,
        PatchIntent::InternalOnly,
    )?;

    let message = strip_html_comments(&message);
    if created {
        queue.tooling = Some(new_tooling_patch(&id, forge, new_stable, message, assess));
    } else {
        upgrade_tooling_patch(&mut queue, &id, forge, new_stable, message, assess);
    }
    also(&mut state, &mut queue)?;
    state.write_queue(&queue)?;
    let verb = if created { "add" } else { "upgrade" };
    state.commit_to_branch(&format!("uplink: {verb} {id} {TOOLING_PATCH_TITLE}"))?;
    Ok(())
}

pub fn refresh_tooling_patch(repo: &Path) -> Result<ToolingRefresh> {
    let Some(plan) = plan_tooling_refresh(repo)? else {
        return Ok(ToolingRefresh { changed: false });
    };
    commit_tooling_refresh(repo, plan, |_, _| Ok(()))?;
    Ok(ToolingRefresh { changed: true })
}

fn new_tooling_patch(
    id: &str,
    forge: Forge,
    stable: String,
    message: String,
    assess: AssessReport,
) -> Patch {
    let created_at = stamp();
    let mut patch = Patch {
        id: id.to_string(),
        commit_message: message,
        status: PatchStatus::Queued,
        depends_on: Vec::new(),
        created_at: created_at.clone(),
        updated_at: created_at,
        patch_id_stable: Some(stable),
        source: PatchSource {
            note: Some(format!("forge {forge}")),
            ..Default::default()
        },
        assess: Some(assess),
        upstream: None,
        merged: None,
        conflict: None,
        approvals: Vec::new(),
        extras: None,
        events: Vec::new(),
        kind: Some(TOOLING_PATCH_KIND.into()),
    };
    add_event(
        &mut patch,
        "created",
        format!("Installed {forge} forge pack as internal-only tooling"),
    );
    patch
}

/// Moves patch `id` into the tooling layer if needed and records the refreshed pack.
fn upgrade_tooling_patch(
    queue: &mut QueueState,
    id: &str,
    forge: Forge,
    stable: String,
    message: String,
    assess: AssessReport,
) {
    if queue.tooling.as_ref().map(|p| p.id.as_str()) != Some(id) {
        if let Some(idx) = queue.internal.iter().position(|p| p.id == id) {
            queue.tooling = Some(queue.internal.remove(idx));
        } else if let Some(idx) = queue.upstream.iter().position(|p| p.id == id) {
            queue.tooling = Some(queue.upstream.remove(idx));
        }
    }
    if let Some(patch) = queue.tooling.as_mut() {
        patch.kind = Some(TOOLING_PATCH_KIND.into());
        patch.status = PatchStatus::Queued;
        patch.conflict = None;
        patch.patch_id_stable = Some(stable);
        patch.commit_message = message;
        patch.assess = Some(assess);
        add_event(
            patch,
            "upgraded",
            format!("Refreshed {forge} forge pack from embedded templates"),
        );
    }
}

/// The forge's overlay laid over the family base pack. Both hold repo-relative
/// paths. A path in both comes from the overlay; `Forge::omitted_paths` are
/// dropped.
pub fn composed_files(forge: Forge) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    match forge {
        Forge::Github => {}
        Forge::TryItOnGithub => collect_pack(PACK_TRY_IT_ON_GITHUB, &mut files),
    }
    match forge.family() {
        ForgeFamily::Github => collect_pack(PACK_GITHUB, &mut files),
    }
    // The sort is stable, so the overlay's copy of a path stays first.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    files.retain(|(path, _)| !forge.omitted_paths().contains(&path.as_str()));
    stamp_version(&mut files)?;
    Ok(files)
}

fn stamp_version(files: &mut [(String, Vec<u8>)]) -> Result<()> {
    let Some((_, data)) = files.iter_mut().find(|(path, _)| path == INSTALL_ACTION) else {
        return Ok(());
    };
    let text = String::from_utf8_lossy(data);
    if !text.contains(VERSION_PLACEHOLDER) {
        return Err(Error::msg(format!(
            "embedded {INSTALL_ACTION} lacks {VERSION_PLACEHOLDER}"
        )));
    }
    *data = text
        .replace(VERSION_PLACEHOLDER, env!("CARGO_PKG_VERSION"))
        .into_bytes();
    Ok(())
}

/// Files for the orphan branch `uplink/hooks`, which `init` creates locally.
pub fn hooks_files(forge: Forge) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    match forge.family() {
        ForgeFamily::Github => collect_pack(PACK_GITHUB_HOOKS, &mut files),
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Pack documentation for operators. It is rendered on the site and must not
/// land in the product repository, where it would replace the product README.
const PACK_README: &str = "README.md";

fn collect_pack(pack: &[(&str, &[u8])], files: &mut Vec<(String, Vec<u8>)>) {
    for (path, data) in pack {
        if *path != PACK_README {
            files.push((path.to_string(), data.to_vec()));
        }
    }
}

fn tooling_commit_message() -> String {
    format!(
        "{TOOLING_PATCH_TITLE}\n\n\
Workflows and the GitHub pull request template for this repository.\n\n\
{DEFAULT_CUTOFF}\n\n\
internal-only; not submitted upstream.\n"
    )
}

fn find_tooling_patch(queue: &QueueState, state: &StateTree) -> Result<Option<String>> {
    if let Some(patch) = &queue.tooling {
        return Ok(Some(patch.id.clone()));
    }
    if let Some(patch) = queue
        .all_patches()
        .find(|p| p.kind.as_deref() == Some(TOOLING_PATCH_KIND))
    {
        return Ok(Some(patch.id.clone()));
    }
    // Legacy queues kept the tooling patch in internal[] without a kind. Only
    // that patch is migrated; an ordinary patch that edits an uplink workflow
    // is left where it is.
    for patch in queue
        .internal
        .iter()
        .filter(|p| p.subject() == TOOLING_PATCH_TITLE)
    {
        let Some(contents) = state.patch(&patch.id)? else {
            continue;
        };
        if contents.contains(".github/workflows/uplink-") {
            return Ok(Some(patch.id.clone()));
        }
    }
    Ok(None)
}

struct SynthesizedPatch {
    formatted: String,
    from_sha: String,
    head_sha: String,
}

fn synthesize_pack_patch(
    repo: &Path,
    files: &[(String, Vec<u8>)],
    message: &str,
) -> Result<SynthesizedPatch> {
    let worktree = TempWorktree::add(repo, "uplink-tooling", "uplink/upstream")?;
    let dir = worktree.dir.as_path();
    let from_sha = git_ok(dir, &["rev-parse", "HEAD"])?;
    for (rel, bytes) in files {
        let dest = dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, bytes)?;
        git(dir, &["add", "-f", "--", rel], GitOpts::default())?;
    }
    if git_succeeds(dir, &["diff", "--cached", "--quiet"])? {
        return Err(Error::msg(
            "forge pack produced no product changes against uplink/upstream",
        ));
    }
    git(dir, &["commit", "-m", message], GitOpts::default())?;
    let head_sha = git_ok(dir, &["rev-parse", "HEAD"])?;
    let formatted = git_ok(dir, &["format-patch", "--full-index", "-1", "--stdout"])?;
    let formatted = if formatted.ends_with('\n') {
        formatted
    } else {
        format!("{formatted}\n")
    };
    Ok(SynthesizedPatch {
        formatted,
        from_sha,
        head_sha,
    })
}

#[cfg(test)]
#[path = "tooling_test.rs"]
mod tests;
