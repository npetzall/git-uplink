use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::{PATCH_DIR, Patch, PatchEvent, PatchLayer, PatchStatus, QUEUE_PATH, QueueState};

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn require_path_component(name: &str) -> Result<&str> {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(Error::msg(format!("invalid path component {name}")));
    }
    Ok(name)
}

pub fn patch_file_name(id: &str) -> Result<String> {
    Ok(format!("{}.patch", require_path_component(id)?))
}

pub fn patch_path(id: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(PATCH_DIR).join(patch_file_name(id)?))
}

pub fn is_active(patch: &Patch) -> bool {
    patch.status.is_active()
}

pub fn read_queue(repo: &Path) -> Result<QueueState> {
    if !repo.join(QUEUE_PATH).is_file() {
        crate::repo::ensure_state_worktree(repo)?;
    }
    let raw = fs::read_to_string(repo.join(QUEUE_PATH))?;
    let queue: QueueState = serde_json::from_str(&raw)?;
    Ok(queue)
}

/// Temp file for [`write_queue`]; `commit_queue` never stages `*.tmp`.
const QUEUE_TMP_PATH: &str = ".uplink/queue.json.tmp";

/// Writes `queue.json` via a temp file and rename so a crash never leaves it truncated.
pub fn write_queue(repo: &Path, queue: &QueueState) -> Result<()> {
    fs::create_dir_all(repo.join(PATCH_DIR))?;
    let body = format!("{}\n", serde_json::to_string_pretty(queue)?);
    let tmp = repo.join(QUEUE_TMP_PATH);
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, repo.join(QUEUE_PATH))
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(written?)
}

pub fn active_upstream(queue: &QueueState) -> Vec<&Patch> {
    queue
        .upstream
        .iter()
        .filter(|p| is_active(p) && p.status != PatchStatus::Conflict)
        .collect()
}

pub fn get_patch<'a>(queue: &'a QueueState, id: &str) -> Result<&'a Patch> {
    queue
        .all_patches()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::msg(format!("Unknown patch {id}")))
}

pub fn get_patch_mut<'a>(queue: &'a mut QueueState, id: &str) -> Result<&'a mut Patch> {
    queue
        .all_patches_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::msg(format!("Unknown patch {id}")))
}

pub fn add_event(patch: &mut Patch, kind: &str, detail: impl Into<String>) {
    let at = now_iso();
    patch.events.push(PatchEvent {
        at: at.clone(),
        kind: kind.into(),
        detail: detail.into(),
    });
    patch.updated_at = at;
}

/// Tooling, then upstream, then internal: the order patches apply on company main.
pub fn apply_order_active(queue: &QueueState) -> Result<Vec<Patch>> {
    let mut ordered = apply_order_upstream_layer(queue)?;
    ordered.extend(topological_layer(active_in(&queue.internal))?);
    Ok(ordered)
}

/// Tooling, then upstream: what exports on top of public upstream.
pub fn apply_order_upstream_layer(queue: &QueueState) -> Result<Vec<Patch>> {
    let mut ordered: Vec<Patch> = queue
        .tooling
        .iter()
        .filter(|p| is_active(p))
        .cloned()
        .collect();
    ordered.extend(topological_layer(active_in(&queue.upstream))?);
    Ok(ordered)
}

fn active_in(layer: &[Patch]) -> Vec<Patch> {
    layer.iter().filter(|p| is_active(p)).cloned().collect()
}

pub fn layer_label(queue: &QueueState, id: &str) -> &'static str {
    queue
        .layer_of(id)
        .map(PatchLayer::as_str)
        .unwrap_or("unknown")
}

fn topological_layer(active: Vec<Patch>) -> Result<Vec<Patch>> {
    if active.iter().all(|p| p.depends_on.is_empty()) {
        return Ok(active);
    }

    let ids: Vec<String> = active.iter().map(|p| p.id.clone()).collect();
    let mut visiting = Vec::new();
    let mut visited = Vec::new();
    let mut ordered = Vec::new();

    fn visit(
        id: &str,
        active: &[Patch],
        visiting: &mut Vec<String>,
        visited: &mut Vec<String>,
        ordered: &mut Vec<Patch>,
    ) -> Result<()> {
        if visited.iter().any(|x| x == id) || !active.iter().any(|p| p.id == id) {
            return Ok(());
        }
        if visiting.iter().any(|x| x == id) {
            return Err(Error::msg(format!("Dependency cycle at {id}")));
        }
        visiting.push(id.into());
        if let Some(patch) = active.iter().find(|p| p.id == id) {
            for dep in &patch.depends_on {
                visit(dep, active, visiting, visited, ordered)?;
            }
            ordered.push(patch.clone());
        }
        visiting.retain(|x| x != id);
        visited.push(id.into());
        Ok(())
    }

    for id in ids {
        visit(&id, &active, &mut visiting, &mut visited, &mut ordered)?;
    }
    Ok(ordered)
}

pub fn take_patch(queue: &mut QueueState, id: &str) -> Result<Patch> {
    if let Some(idx) = queue.upstream.iter().position(|p| p.id == id) {
        return Ok(queue.upstream.remove(idx));
    }
    if let Some(idx) = queue.internal.iter().position(|p| p.id == id) {
        return Ok(queue.internal.remove(idx));
    }
    if queue.tooling.as_ref().is_some_and(|p| p.id == id) {
        return Err(Error::msg(format!("{id} is tooling and cannot be moved")));
    }
    Err(Error::msg(format!("Unknown patch {id}")))
}

pub fn move_patch(queue: &mut QueueState, id: &str, to_internal: bool) -> Result<Patch> {
    let patch = take_patch(queue, id)?;
    queue.push_patch(patch.clone(), to_internal);
    Ok(get_patch(queue, id)?.clone())
}

pub fn cannot_depend_on(queue: &QueueState, from_internal: bool, dep_id: &str) -> bool {
    if queue.is_tooling(dep_id) {
        return true;
    }
    if from_internal {
        return false;
    }
    queue.is_internal(dep_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Patch, QueueConfig, QueueState};

    fn patch(id: &str) -> Patch {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": id,
            "status": "queued",
            "dependsOn": [],
            "createdAt": "t",
            "updatedAt": "t",
            "source": {},
            "events": []
        }))
        .unwrap()
    }

    fn patch_deps(id: &str, deps: &[&str]) -> Patch {
        let mut patch = patch(id);
        patch.depends_on = deps.iter().map(|s| (*s).to_string()).collect();
        patch
    }

    #[test]
    fn apply_order_runs_internal_after_later_upstream() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.internal.push(patch("upl_internal"));
        queue.upstream.push(patch("upl_upstream"));
        let order: Vec<_> = apply_order_active(&queue)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(order, vec!["upl_upstream", "upl_internal"]);
    }

    #[test]
    fn layer_topo_ignores_depends_on_from_an_earlier_layer() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_up"));
        queue.internal.push(patch_deps("upl_int", &["upl_up"]));
        let order: Vec<_> = apply_order_active(&queue)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(order, vec!["upl_up", "upl_int"]);
    }

    #[test]
    fn upstream_cannot_depend_on_internal_or_tooling() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.tooling = Some(patch("upl_tool"));
        queue.internal.push(patch("upl_int"));
        queue.upstream.push(patch("upl_up"));
        assert!(cannot_depend_on(&queue, false, "upl_int"));
        assert!(cannot_depend_on(&queue, false, "upl_tool"));
        assert!(!cannot_depend_on(&queue, false, "upl_up"));
        assert!(!cannot_depend_on(&queue, true, "upl_up"));
        assert!(cannot_depend_on(&queue, true, "upl_tool"));
    }

    #[test]
    fn write_queue_replaces_file_without_leaving_temp() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let mut queue = QueueState::empty(QueueConfig::default());
        write_queue(repo, &queue).unwrap();
        queue.upstream.push(patch("upl_one"));
        write_queue(repo, &queue).unwrap();

        let raw = fs::read_to_string(repo.join(QUEUE_PATH)).unwrap();
        let read: QueueState = serde_json::from_str(&raw).unwrap();
        assert_eq!(read.upstream.len(), 1);
        assert!(!repo.join(QUEUE_TMP_PATH).exists());
    }

    #[test]
    fn require_path_component_rejects_traversal() {
        assert!(require_path_component("upl_abcdefghij").is_ok());
        assert!(require_path_component("..").is_err());
        assert!(require_path_component("foo/bar").is_err());
        assert!(require_path_component("foo\\bar").is_err());
        assert!(require_path_component("upl_ab/../cd").is_err());
    }

    #[test]
    fn patch_path_stays_under_patches_dir() {
        let path = patch_path("upl_abcdefghij").unwrap();
        assert_eq!(path, PathBuf::from(PATCH_DIR).join("upl_abcdefghij.patch"));
        assert!(patch_path("../etc").is_err());
        assert!(patch_path("a/b").is_err());
    }
}
