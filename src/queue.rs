use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::{
    MergeVia, PATCH_DIR, PREVIOUS_MAIN_PATH, Patch, PatchEvent, PatchLayer, PatchMerged,
    PatchStatus, PreviousMain, QUEUE_PATH, QueueState,
};

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
    let mut queue = parse_queue(&raw)?;
    queue.settings = crate::settings::load_settings(repo, queue.config.forge);
    Ok(queue)
}

/// `queue.json` as written by this or an earlier binary.
pub fn parse_queue(raw: &str) -> Result<QueueState> {
    let mut value: serde_json::Value = serde_json::from_str(raw)?;
    migrate_legacy_messages(&mut value);
    Ok(serde_json::from_value(value)?)
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

pub fn write_previous_main(repo: &Path, previous: &PreviousMain) -> Result<()> {
    let body = format!("{}\n", serde_json::to_string_pretty(previous)?);
    Ok(fs::write(repo.join(PREVIOUS_MAIN_PATH), body)?)
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
mod tests {
    use super::*;
    use crate::types::{Patch, QueueConfig, QueueState};

    fn patch(id: &str) -> Patch {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "commitMessage": id,
            "status": "queued",
            "dependsOn": [],
            "createdAt": "t",
            "updatedAt": "t",
            "source": {},
            "events": []
        }))
        .unwrap()
    }

    #[test]
    fn legacy_title_and_public_parts_become_messages() {
        let mut value = serde_json::json!({
            "upstream": [{
                "id": "upl_a",
                "title": "Use SHA-256",
                "status": "queued",
                "dependsOn": [],
                "createdAt": "t",
                "updatedAt": "t",
                "source": {},
                "events": [],
                "assess": {
                    "at": "t",
                    "ok": true,
                    "commitMessage": "Use SHA-256\n\nWhy.",
                    "publicSubject": "Use SHA-256",
                    "publicBody": "Why.",
                    "cutoffFound": false,
                    "checks": []
                }
            }, {
                "id": "upl_b",
                "title": "Old title",
                "commitMessage": "Kept subject\n\nBody",
                "status": "queued",
                "dependsOn": [],
                "createdAt": "t",
                "updatedAt": "t",
                "source": {},
                "events": []
            }]
        });
        migrate_legacy_messages(&mut value);
        let a: Patch = serde_json::from_value(value["upstream"][0].clone()).unwrap();
        assert_eq!(a.commit_message, "Use SHA-256");
        assert_eq!(
            a.assess.as_ref().unwrap().public_message,
            "Use SHA-256\n\nWhy."
        );
        let b: Patch = serde_json::from_value(value["upstream"][1].clone()).unwrap();
        assert_eq!(b.subject(), "Kept subject");

        let written = serde_json::to_value(&a).unwrap();
        assert!(written.get("title").is_none(), "{written}");
        let assess = &written["assess"];
        assert!(assess.get("publicSubject").is_none(), "{assess}");
        assert!(assess.get("commitMessage").is_none(), "{assess}");
        assert_eq!(assess["publicMessage"], "Use SHA-256\n\nWhy.");
    }

    fn patch_deps(id: &str, deps: &[&str]) -> Patch {
        let mut patch = patch(id);
        patch.depends_on = deps.iter().map(|s| (*s).to_string()).collect();
        patch
    }

    #[test]
    fn set_merged_drops_the_patch_from_every_dependency_list() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_a"));
        queue
            .upstream
            .push(patch_deps("upl_b", &["upl_a", "upl_x"]));
        queue.internal.push(patch_deps("upl_c", &["upl_a"]));
        let merged = PatchMerged {
            via: MergeVia::EmptyRebase,
            at: "t".into(),
            upstream_sha: None,
        };
        set_merged(&mut queue, "upl_a", merged, "done").unwrap();

        let get = |id: &str| queue.all_patches().find(|p| p.id == id).unwrap();
        assert_eq!(get("upl_a").status, PatchStatus::Merged);
        assert_eq!(get("upl_b").depends_on, vec!["upl_x".to_string()]);
        assert!(get("upl_c").depends_on.is_empty());
        let event = get("upl_c").events.last().unwrap();
        assert_eq!(event.kind, "dependency");
        assert_eq!(event.detail, "Dropped dependency upl_a: merged");
    }

    #[test]
    fn merged_via_reads_the_former_patch_id_name() {
        let merged: PatchMerged = serde_json::from_str(r#"{"via":"patch-id","at":"t"}"#).unwrap();
        assert_eq!(merged.via, MergeVia::StablePatchId);
        let written = serde_json::to_string(&merged).unwrap();
        assert!(written.contains(r#""via":"stable-patch-id""#), "{written}");
    }

    #[test]
    fn merged_detail_names_the_evidence() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            merged_detail(MergeVia::StablePatchId, Some(sha)),
            "Stable patch-id matches upstream 0123456"
        );
        assert_eq!(
            merged_detail(MergeVia::EmptyRebase, Some(sha)),
            "Empty rebase, superseded by upstream"
        );
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
    fn layer_topo_puts_dependencies_first() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch_deps("upl_c", &["upl_b"]));
        queue.upstream.push(patch_deps("upl_b", &["upl_a"]));
        queue.upstream.push(patch("upl_a"));
        queue.upstream.push(patch("upl_d"));
        let order: Vec<_> = apply_order_active(&queue)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(order, vec!["upl_a", "upl_b", "upl_c", "upl_d"]);
    }

    #[test]
    fn layer_topo_rejects_cycles() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch_deps("upl_a", &["upl_b"]));
        queue.upstream.push(patch_deps("upl_b", &["upl_a"]));
        let err = apply_order_active(&queue).unwrap_err();
        assert!(err.to_string().contains("Dependency cycle"), "{err}");
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn dependency_closure_puts_dependencies_first() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_a"));
        queue.upstream.push(patch_deps("upl_b", &["upl_a"]));
        queue.upstream.push(patch_deps("upl_c", &["upl_b"]));
        queue.upstream.push(patch("upl_d"));
        assert_eq!(
            dependency_closure(&queue, &ids(&["upl_c", "upl_d"])).unwrap(),
            ids(&["upl_a", "upl_b", "upl_c", "upl_d"])
        );
        assert_eq!(
            dependency_closure(&queue, &ids(&["upl_a", "upl_b", "upl_c"])).unwrap(),
            ids(&["upl_a", "upl_b", "upl_c"])
        );
    }

    #[test]
    fn dependency_closure_follows_dropped_patches_and_keeps_unknown_ids() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_a"));
        let mut dropped = patch_deps("upl_b", &["upl_a"]);
        dropped.status = PatchStatus::Dropped;
        queue.upstream.push(dropped);
        assert_eq!(
            dependency_closure(&queue, &ids(&["upl_b", "upl_x"])).unwrap(),
            ids(&["upl_a", "upl_b", "upl_x"])
        );
    }

    #[test]
    fn dependency_closure_rejects_cycles() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch_deps("upl_a", &["upl_b"]));
        queue.upstream.push(patch_deps("upl_b", &["upl_a"]));
        let err = dependency_closure(&queue, &ids(&["upl_a"])).unwrap_err();
        assert!(err.to_string().contains("Dependency cycle"), "{err}");
    }

    #[test]
    fn transitive_reduction_keeps_the_last_patch_of_a_chain() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_a"));
        queue.upstream.push(patch_deps("upl_b", &["upl_a"]));
        let (kept, removed) =
            transitive_reduction(&queue, &ids(&["upl_a", "upl_b", "upl_a"])).unwrap();
        assert_eq!(kept, ids(&["upl_b"]));
        assert_eq!(
            removed,
            vec![ImpliedDependency {
                id: "upl_a".into(),
                by: "upl_b".into()
            }]
        );
    }

    #[test]
    fn transitive_reduction_of_a_diamond_keeps_both_sides() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_a"));
        queue.upstream.push(patch_deps("upl_b", &["upl_a"]));
        queue.upstream.push(patch_deps("upl_c", &["upl_a"]));
        let list = ids(&["upl_a", "upl_b", "upl_c"]);
        let kept = direct_dependencies(&queue, &list).unwrap();
        assert_eq!(kept, ids(&["upl_b", "upl_c"]));
        assert_eq!(
            dependency_closure(&queue, &kept).unwrap(),
            dependency_closure(&queue, &list).unwrap()
        );
    }

    fn stream_ids(queue: &QueueState) -> Vec<Vec<&str>> {
        submit_layers(queue)
            .unwrap()
            .iter()
            .map(|column| column.iter().map(|n| n.patch.id.as_str()).collect())
            .collect()
    }

    #[test]
    fn submit_layers_puts_independent_patches_in_one_column() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_a"));
        queue.upstream.push(patch("upl_b"));
        assert_eq!(stream_ids(&queue), vec![vec!["upl_a", "upl_b"]]);
        assert!(
            submit_layers(&QueueState::empty(QueueConfig::default()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn submit_layers_places_a_patch_after_its_deepest_dependency() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue
            .upstream
            .push(patch_deps("upl_d", &["upl_a", "upl_c"]));
        queue
            .upstream
            .push(patch_deps("upl_c", &["upl_a", "upl_b"]));
        queue.upstream.push(patch("upl_a"));
        queue.upstream.push(patch("upl_b"));
        assert_eq!(
            stream_ids(&queue),
            vec![vec!["upl_a", "upl_b"], vec!["upl_c"], vec!["upl_d"]]
        );
    }

    #[test]
    fn submit_layers_orders_columns_by_their_dependencies() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch("upl_a"));
        queue.upstream.push(patch("upl_b"));
        queue.upstream.push(patch_deps("upl_on_b", &["upl_b"]));
        queue.upstream.push(patch_deps("upl_on_a", &["upl_a"]));
        assert_eq!(
            stream_ids(&queue),
            vec![vec!["upl_a", "upl_b"], vec!["upl_on_a", "upl_on_b"]]
        );
    }

    #[test]
    fn submit_layers_skips_inactive_and_reports_missing_dependencies() {
        let mut queue = QueueState::empty(QueueConfig::default());
        let mut dropped = patch("upl_dropped");
        dropped.status = PatchStatus::Dropped;
        let mut merged = patch("upl_merged");
        merged.status = PatchStatus::Merged;
        queue.upstream.push(dropped);
        queue.upstream.push(merged);
        queue
            .upstream
            .push(patch_deps("upl_a", &["upl_dropped", "upl_int"]));
        let mut conflict = patch("upl_conflict");
        conflict.status = PatchStatus::Conflict;
        queue.upstream.push(conflict);
        queue.internal.push(patch("upl_int"));
        let layers = submit_layers(&queue).unwrap();
        assert_eq!(layers.len(), 1);
        let ids: Vec<_> = layers[0].iter().map(|n| n.patch.id.as_str()).collect();
        assert_eq!(ids, vec!["upl_a", "upl_conflict"]);
        assert_eq!(layers[0][0].missing, vec!["upl_dropped", "upl_int"]);
        assert!(layers[0][1].missing.is_empty());
    }

    #[test]
    fn submit_layers_rejects_cycles() {
        let mut queue = QueueState::empty(QueueConfig::default());
        queue.upstream.push(patch_deps("upl_a", &["upl_b"]));
        queue.upstream.push(patch_deps("upl_b", &["upl_a"]));
        let err = submit_layers(&queue).unwrap_err();
        assert!(err.to_string().contains("Dependency cycle"), "{err}");
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
