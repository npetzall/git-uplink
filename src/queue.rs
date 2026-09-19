use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::{
    PATCH_DIR, Patch, PatchEvent, PatchLayer, QUEUE_PATH, QueueConfig, QueueState,
    TOOLING_PATCH_KIND,
};

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn patch_path(id: &str) -> PathBuf {
    PathBuf::from(PATCH_DIR).join(format!("{id}.patch"))
}

pub fn empty_queue(config: QueueConfig) -> QueueState {
    QueueState::empty(config)
}

pub fn is_active(patch: &Patch) -> bool {
    patch.status != "merged" && patch.status != "dropped"
}

pub fn read_queue(repo: &Path) -> Result<QueueState> {
    if !repo.join(QUEUE_PATH).is_file() {
        crate::repo::ensure_state_worktree(repo)?;
    }
    let raw = fs::read_to_string(repo.join(QUEUE_PATH))?;
    let mut queue: QueueState = serde_json::from_str(&raw)?;
    lift_legacy_tooling(&mut queue, repo)?;
    Ok(queue)
}

pub fn write_queue(repo: &Path, queue: &QueueState) -> Result<()> {
    fs::create_dir_all(repo.join(PATCH_DIR))?;
    let body = format!("{}\n", serde_json::to_string_pretty(queue)?);
    fs::write(repo.join(QUEUE_PATH), body)?;
    Ok(())
}

fn lift_legacy_tooling(queue: &mut QueueState, repo: &Path) -> Result<()> {
    if queue.tooling.is_some() {
        return Ok(());
    }
    let id = match find_legacy_tooling_id(queue, repo)? {
        Some(id) => id,
        None => return Ok(()),
    };
    if let Some(idx) = queue.internal.iter().position(|p| p.id == id) {
        queue.tooling = Some(queue.internal.remove(idx));
    } else if let Some(idx) = queue.upstream.iter().position(|p| p.id == id) {
        queue.tooling = Some(queue.upstream.remove(idx));
    }
    Ok(())
}

fn find_legacy_tooling_id(queue: &QueueState, repo: &Path) -> Result<Option<String>> {
    if let Some(patch) = queue
        .all_patches()
        .find(|p| p.kind.as_deref() == Some(TOOLING_PATCH_KIND))
    {
        return Ok(Some(patch.id.clone()));
    }
    for patch in queue.all_patches() {
        let path = repo.join(format!("{PATCH_DIR}/{}.patch", patch.id));
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        if contents.contains(".github/workflows/uplink-") {
            return Ok(Some(patch.id.clone()));
        }
    }
    Ok(None)
}

pub fn active_upstream(queue: &QueueState) -> Vec<&Patch> {
    queue
        .upstream
        .iter()
        .filter(|p| is_active(p) && p.status != "conflict")
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

pub fn apply_order_active(queue: &QueueState) -> Result<Vec<Patch>> {
    let mut ordered = Vec::new();
    if let Some(tooling) = &queue.tooling {
        if is_active(tooling) {
            ordered.push(tooling.clone());
        }
    }
    ordered.extend(topological_layer(
        queue
            .upstream
            .iter()
            .filter(|p| is_active(p))
            .cloned()
            .collect(),
    )?);
    ordered.extend(topological_layer(
        queue
            .internal
            .iter()
            .filter(|p| is_active(p))
            .cloned()
            .collect(),
    )?);
    Ok(ordered)
}

pub fn apply_order_upstream_layer(queue: &QueueState) -> Result<Vec<Patch>> {
    let mut ordered = Vec::new();
    if let Some(tooling) = &queue.tooling {
        if is_active(tooling) {
            ordered.push(tooling.clone());
        }
    }
    ordered.extend(topological_layer(
        queue
            .upstream
            .iter()
            .filter(|p| is_active(p))
            .cloned()
            .collect(),
    )?);
    Ok(ordered)
}

pub fn topological_active(queue: &QueueState) -> Result<Vec<Patch>> {
    apply_order_active(queue)
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
}
