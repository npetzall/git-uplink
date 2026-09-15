use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::{Patch, PatchEvent, QueueConfig, QueueState, PATCH_DIR, QUEUE_PATH};

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn patch_path(id: &str) -> PathBuf {
    PathBuf::from(PATCH_DIR).join(format!("{id}.patch"))
}

pub fn empty_queue(config: QueueConfig) -> QueueState {
    QueueState::empty(config)
}

pub fn read_queue(repo: &Path) -> Result<QueueState> {
    let raw = fs::read_to_string(repo.join(QUEUE_PATH))?;
    Ok(serde_json::from_str(&raw)?)
}

pub fn write_queue(repo: &Path, queue: &QueueState) -> Result<()> {
    fs::create_dir_all(repo.join(PATCH_DIR))?;
    let body = format!("{}\n", serde_json::to_string_pretty(queue)?);
    fs::write(repo.join(QUEUE_PATH), body)?;
    Ok(())
}

pub fn active_patches(queue: &QueueState) -> Vec<&Patch> {
    queue
        .patches
        .iter()
        .filter(|p| p.status != "merged" && p.status != "dropped")
        .collect()
}

pub fn get_patch<'a>(queue: &'a QueueState, id: &str) -> Result<&'a Patch> {
    queue
        .patches
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::msg(format!("Unknown patch {id}")))
}

pub fn get_patch_mut<'a>(queue: &'a mut QueueState, id: &str) -> Result<&'a mut Patch> {
    queue
        .patches
        .iter_mut()
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

pub fn topological_active(queue: &QueueState) -> Result<Vec<Patch>> {
    let active: Vec<Patch> = active_patches(queue).into_iter().cloned().collect();
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
