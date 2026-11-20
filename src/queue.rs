use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::types::{
    MergeVia, PATCH_DIR, Patch, PatchEvent, PatchLayer, PatchMerged, PatchStatus, QueueState,
};

/// Now in UTC, as RFC 3339 with milliseconds: `2026-11-17T09:30:00.123Z`.
pub fn now_iso() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_utc_millis(since)
}

fn format_utc_millis(since_epoch: Duration) -> String {
    let secs = since_epoch.as_secs();
    let (year, month, day) = civil_from_days(secs / 86_400);
    let time = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        time / 3600,
        time / 60 % 60,
        time % 60,
        since_epoch.subsec_millis()
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01, after Howard
/// Hinnant's `civil_from_days`.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
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

/// The queue on local `uplink/state` (created from origin's when missing).
pub fn read_queue(repo: &Path) -> Result<QueueState> {
    crate::state::StateTree::head(repo)?.queue()
}

/// `queue.json` as written by this or an earlier binary.
pub fn parse_queue(raw: &str) -> Result<QueueState> {
    let mut value: serde_json::Value = serde_json::from_str(raw)?;
    migrate_legacy_messages(&mut value);
    let queue: QueueState = serde_json::from_value(value)?;
    queue.config.validate()?;
    Ok(queue)
}

/// Rewrites state written before a patch had only a commit message:
/// a patch's `title` becomes its `commitMessage` when that is empty, an
/// adopt group's `title` its `message`, and an assessment's
/// `publicSubject` and `publicBody` become `publicMessage`.
pub fn migrate_legacy_messages(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Array(items) => items.iter_mut().for_each(migrate_legacy_messages),
        Value::Object(map) => {
            // An adopt group: `title` with an optional `message`.
            if map.get("commits").is_some_and(Value::is_array)
                && let Some(Value::String(title)) = map.remove("title")
            {
                let empty = map
                    .get("message")
                    .and_then(Value::as_str)
                    .is_none_or(|m| m.trim().is_empty());
                if empty {
                    map.insert("message".into(), Value::String(title));
                }
            }
            if map.contains_key("createdAt")
                && map.contains_key("status")
                && let Some(Value::String(title)) = map.remove("title")
            {
                let empty = map
                    .get("commitMessage")
                    .and_then(Value::as_str)
                    .is_none_or(|m| m.trim().is_empty());
                if empty {
                    map.insert("commitMessage".into(), Value::String(title));
                }
            }
            if map.contains_key("checks") && map.contains_key("publicSubject") {
                let take =
                    |map: &mut serde_json::Map<String, Value>, key: &str| match map.remove(key) {
                        Some(Value::String(s)) => s.trim().to_string(),
                        _ => String::new(),
                    };
                let subject = take(map, "publicSubject");
                let body = take(map, "publicBody");
                map.remove("commitMessage");
                let message = if body.is_empty() {
                    subject
                } else {
                    format!("{subject}\n\n{body}")
                };
                map.insert("publicMessage".into(), Value::String(message));
            }
            map.values_mut().for_each(migrate_legacy_messages);
        }
        _ => {}
    }
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

/// Marks `id` merged and removes it from every other patch's `depends_on`:
/// a merged patch is part of upstream, so nothing waits on it any more.
/// The caller has verified `merged.via`.
pub fn set_merged(
    queue: &mut QueueState,
    id: &str,
    merged: PatchMerged,
    detail: impl Into<String>,
) -> Result<()> {
    let patch = queue
        .all_patches_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::msg(format!("unknown patch {id}")))?;
    patch.status = PatchStatus::Merged;
    patch.conflict = None;
    patch.merged = Some(merged);
    add_event(patch, "merged", detail);
    for other in queue.all_patches_mut() {
        if other.id != id && other.depends_on.iter().any(|dep| dep == id) {
            other.depends_on.retain(|dep| dep != id);
            add_event(
                other,
                "dependency",
                format!("Dropped dependency {id}: merged"),
            );
        }
    }
    Ok(())
}

/// The event detail for a merge detected via `via` at `upstream_sha`.
pub fn merged_detail(via: MergeVia, upstream_sha: Option<&str>) -> String {
    let at = upstream_sha
        .map(|sha| format!(" {}", &sha[..sha.len().min(7)]))
        .unwrap_or_default();
    match via {
        MergeVia::StablePatchId => format!("Stable patch-id matches upstream{at}"),
        MergeVia::Trailer => format!("Trailer and stable patch-id match upstream{at}"),
        MergeVia::EmptyRebase => "Empty rebase, superseded by upstream".into(),
        MergeVia::Pr | MergeVia::Manual => format!("Detected via {}", via.as_str()),
    }
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

/// Orders one layer so every patch follows the dependencies it lists in that
/// layer. Keeps queue order otherwise; dependencies outside the layer are ignored.
fn topological_layer(active: Vec<Patch>) -> Result<Vec<Patch>> {
    if active.iter().all(|p| p.depends_on.is_empty()) {
        return Ok(active);
    }

    struct Walk<'a> {
        by_id: HashMap<&'a str, &'a Patch>,
        visiting: HashSet<&'a str>,
        visited: HashSet<&'a str>,
        ordered: Vec<Patch>,
    }

    fn visit<'a>(walk: &mut Walk<'a>, id: &'a str) -> Result<()> {
        let Some(patch) = walk.by_id.get(id).copied() else {
            return Ok(());
        };
        if walk.visited.contains(id) {
            return Ok(());
        }
        if !walk.visiting.insert(id) {
            return Err(Error::msg(format!("Dependency cycle at {id}")));
        }
        for dep in &patch.depends_on {
            visit(walk, dep)?;
        }
        walk.ordered.push(patch.clone());
        walk.visiting.remove(id);
        walk.visited.insert(id);
        Ok(())
    }

    let mut walk = Walk {
        by_id: active.iter().map(|p| (p.id.as_str(), p)).collect(),
        visiting: HashSet::new(),
        visited: HashSet::new(),
        ordered: Vec::with_capacity(active.len()),
    };
    for patch in &active {
        visit(&mut walk, &patch.id)?;
    }
    Ok(walk.ordered)
}

/// `ids` and every patch they depend on, directly or through another
/// dependency, dependencies first. `depends_on` lists direct dependencies
/// only; this is what applies before a patch. Dependencies are followed
/// through every patch in the queue, whatever its status, so dropping a
/// patch does not drop what it depended on. Unknown ids are kept, for the
/// caller to report.
pub fn dependency_closure(queue: &QueueState, ids: &[String]) -> Result<Vec<String>> {
    struct Walk<'a> {
        queue: &'a QueueState,
        visiting: HashSet<String>,
        visited: HashSet<String>,
        ordered: Vec<String>,
    }

    fn visit(walk: &mut Walk<'_>, id: &str) -> Result<()> {
        if walk.visited.contains(id) {
            return Ok(());
        }
        if !walk.visiting.insert(id.to_string()) {
            return Err(Error::msg(format!("Dependency cycle at {id}")));
        }
        let deps = walk
            .queue
            .all_patches()
            .find(|p| p.id == id)
            .map(|p| p.depends_on.clone())
            .unwrap_or_default();
        for dep in &deps {
            visit(walk, dep)?;
        }
        walk.visiting.remove(id);
        walk.visited.insert(id.to_string());
        walk.ordered.push(id.to_string());
        Ok(())
    }

    let mut walk = Walk {
        queue,
        visiting: HashSet::new(),
        visited: HashSet::new(),
        ordered: Vec::new(),
    };
    for id in ids {
        visit(&mut walk, id)?;
    }
    Ok(walk.ordered)
}

/// What `id` brings in besides itself: [`dependency_closure`] of its
/// `depends_on`.
fn implied_by(queue: &QueueState, id: &str) -> Result<HashSet<String>> {
    let deps = queue
        .all_patches()
        .find(|p| p.id == id)
        .map(|p| p.depends_on.clone())
        .unwrap_or_default();
    Ok(dependency_closure(queue, &deps)?.into_iter().collect())
}

/// One id [`transitive_reduction`] left out, and an id kept that brings it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpliedDependency {
    pub id: String,
    pub by: String,
}

/// `ids` without the ones another of them already brings in, and without
/// repeats, in their order. The closure, and so what applies, is unchanged.
pub fn transitive_reduction(
    queue: &QueueState,
    ids: &[String],
) -> Result<(Vec<String>, Vec<ImpliedDependency>)> {
    let mut unique: Vec<&String> = Vec::with_capacity(ids.len());
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    let implied: Vec<HashSet<String>> = unique
        .iter()
        .map(|id| implied_by(queue, id))
        .collect::<Result<_>>()?;
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for id in &unique {
        let by = unique
            .iter()
            .zip(&implied)
            .find(|(other, set)| *other != id && set.contains(id.as_str()));
        match by {
            Some((other, _)) => removed.push(ImpliedDependency {
                id: (*id).clone(),
                by: (*other).clone(),
            }),
            None => kept.push((*id).clone()),
        }
    }
    Ok((kept, removed))
}

/// [`transitive_reduction`] without the report of what was left out.
pub fn direct_dependencies(queue: &QueueState, ids: &[String]) -> Result<Vec<String>> {
    Ok(transitive_reduction(queue, ids)?.0)
}

/// One active upstream patch in [`submit_layers`].
#[derive(Debug)]
pub struct StreamNode<'a> {
    pub patch: &'a Patch,
    /// Dependencies that are not active upstream patches (dropped, unknown
    /// or internal). They do not place the patch, but they block its submit.
    pub missing: Vec<String>,
}

/// Active upstream patches in the order they can go upstream. Column 0 has
/// no dependency on another active upstream patch; a patch in column N
/// depends on one in column N-1 and goes once all its dependencies are merged.
/// Column 0 keeps queue order; later columns sort by where their dependencies
/// sit, so edges cross less.
pub fn submit_layers(queue: &QueueState) -> Result<Vec<Vec<StreamNode<'_>>>> {
    let active: Vec<&Patch> = queue.upstream.iter().filter(|p| is_active(p)).collect();
    let index: HashMap<&str, usize> = active
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();

    fn depth(
        i: usize,
        active: &[&Patch],
        index: &HashMap<&str, usize>,
        memo: &mut [Option<usize>],
        visiting: &mut [bool],
    ) -> Result<usize> {
        if let Some(layer) = memo[i] {
            return Ok(layer);
        }
        if visiting[i] {
            return Err(Error::msg(format!("Dependency cycle at {}", active[i].id)));
        }
        visiting[i] = true;
        let mut layer = 0;
        for dep in &active[i].depends_on {
            if let Some(&j) = index.get(dep.as_str()) {
                layer = layer.max(depth(j, active, index, memo, visiting)? + 1);
            }
        }
        visiting[i] = false;
        memo[i] = Some(layer);
        Ok(layer)
    }

    let mut memo = vec![None; active.len()];
    let mut visiting = vec![false; active.len()];
    let mut layers: Vec<Vec<usize>> = Vec::new();
    for i in 0..active.len() {
        let layer = depth(i, &active, &index, &mut memo, &mut visiting)?;
        if layers.len() <= layer {
            layers.resize_with(layer + 1, Vec::new);
        }
        layers[layer].push(i);
    }

    // Row of each placed patch, for the barycenter of the next column.
    let mut row: HashMap<usize, f64> = HashMap::new();
    for column in &mut layers {
        let key = |i: &usize| -> f64 {
            let rows: Vec<f64> = active[*i]
                .depends_on
                .iter()
                .filter_map(|dep| index.get(dep.as_str()).and_then(|j| row.get(j)))
                .copied()
                .collect();
            if rows.is_empty() {
                0.0
            } else {
                rows.iter().sum::<f64>() / rows.len() as f64
            }
        };
        column.sort_by(|a, b| key(a).total_cmp(&key(b)).then(a.cmp(b)));
        for (r, i) in column.iter().enumerate() {
            row.insert(*i, r as f64);
        }
    }

    Ok(layers
        .into_iter()
        .map(|column| {
            column
                .into_iter()
                .map(|i| StreamNode {
                    patch: active[i],
                    missing: active[i]
                        .depends_on
                        .iter()
                        .filter(|dep| !index.contains_key(dep.as_str()))
                        .cloned()
                        .collect(),
                })
                .collect()
        })
        .collect())
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
#[path = "queue_test.rs"]
mod tests;
