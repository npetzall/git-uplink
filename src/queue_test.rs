use super::*;
use crate::types::{Patch, QueueConfig, QueueState};

#[test]
fn format_utc_millis_writes_rfc3339_in_utc() {
    let at =
        |secs: u64, millis: u64| format_utc_millis(Duration::from_millis(secs * 1000 + millis));
    assert_eq!(at(0, 0), "1970-01-01T00:00:00.000Z");
    assert_eq!(at(951_782_400, 7), "2000-02-29T00:00:00.007Z");
    assert_eq!(at(1_709_164_800, 0), "2024-02-29T00:00:00.000Z");
    assert_eq!(at(1_709_251_199, 999), "2024-02-29T23:59:59.999Z");
    assert_eq!(at(1_798_761_600, 120), "2027-01-01T00:00:00.120Z");
    assert_eq!(at(4_102_444_799, 0), "2099-12-31T23:59:59.000Z");
}

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

#[test]
fn parse_queue_refuses_option_and_shell_names() {
    let queue = QueueState::empty(QueueConfig::default());
    let raw = serde_json::to_string(&queue).unwrap();
    parse_queue(&raw).unwrap();
    for (field, bad) in [
        ("upstreamRemote", "--upload-pack=touch X;"),
        ("contribRemote", "-c"),
        ("contribRemote", "a b"),
        ("internalBranch", "main; curl x|sh"),
        ("internalBranch", "--orphan"),
        ("upstreamBranch", "a..b"),
        ("upstreamBranch", "$(id)"),
        ("internalBranch", ""),
    ] {
        let mut value = serde_json::to_value(&queue).unwrap();
        value["config"][field] = bad.into();
        let err = parse_queue(&value.to_string()).unwrap_err().to_string();
        assert!(err.contains(field), "{field}={bad:?}: {err}");
    }
    for (field, good) in [
        ("internalBranch", "release/1.0"),
        ("upstreamRemote", "up_2.x"),
    ] {
        let mut value = serde_json::to_value(&queue).unwrap();
        value["config"][field] = good.into();
        parse_queue(&value.to_string()).unwrap();
    }
}

#[test]
fn parse_queue_refuses_a_newer_version() {
    let queue = QueueState::empty(QueueConfig::default());
    let mut value = serde_json::to_value(&queue).unwrap();
    value["version"] = (crate::types::QUEUE_VERSION + 1).into();
    let err = parse_queue(&value.to_string()).unwrap_err().to_string();
    assert!(err.contains("Upgrade git uplink"), "{err}");
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
    let (kept, removed) = transitive_reduction(&queue, &ids(&["upl_a", "upl_b", "upl_a"])).unwrap();
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
