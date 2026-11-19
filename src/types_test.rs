use super::{CheckStatus, PatchIntent, PatchStatus};

#[test]
fn an_old_patch_source_author_is_ignored() {
    let source: super::PatchSource =
        serde_json::from_str(r#"{"author": "Jane", "note": "n"}"#).unwrap();
    assert!(source.authors.is_empty());
    assert_eq!(serde_json::to_string(&source).unwrap(), r#"{"note":"n"}"#);
}

#[test]
fn patch_intent_uses_kebab_case_json() {
    assert_eq!(
        serde_json::from_str::<PatchIntent>("\"internal-only\"").unwrap(),
        PatchIntent::InternalOnly
    );
    assert_eq!(
        serde_json::from_str::<PatchIntent>("\"upstream\"").unwrap(),
        PatchIntent::Upstream
    );
    assert_eq!(
        serde_json::to_string(&PatchIntent::InternalOnly).unwrap(),
        "\"internal-only\""
    );
    assert!(serde_json::from_str::<PatchIntent>("\"internal\"").is_err());
    assert_eq!(PatchIntent::default(), PatchIntent::Upstream);
}

#[test]
fn adopt_group_intent_defaults_to_upstream() {
    let group: crate::AdoptGroup =
        serde_json::from_str(r#"{"commits": ["abc"], "message": "Metrics"}"#).unwrap();
    assert_eq!(group.intent, PatchIntent::Upstream);
    let err = serde_json::from_str::<crate::AdoptGroup>(
        r#"{"commits": ["abc"], "message": "Metrics", "intent": "internal"}"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("internal"), "{err}");
}

#[test]
fn check_status_keeps_lowercase_json() {
    assert_eq!(
        serde_json::to_string(&CheckStatus::Fail).unwrap(),
        "\"fail\""
    );
    assert_eq!(
        serde_json::from_str::<CheckStatus>("\"warn\"").unwrap(),
        CheckStatus::Warn
    );
}

#[test]
fn patch_status_keeps_lowercase_json() {
    assert_eq!(
        serde_json::to_string(&PatchStatus::Approved).unwrap(),
        "\"approved\""
    );
    assert_eq!(
        serde_json::from_str::<PatchStatus>("\"conflict\"").unwrap(),
        PatchStatus::Conflict
    );
    assert!(serde_json::from_str::<PatchStatus>("\"bogus\"").is_err());
    for status in [PatchStatus::Waiting, PatchStatus::Rejected] {
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, format!("\"{status}\""));
        assert_eq!(serde_json::from_str::<PatchStatus>(&json).unwrap(), status);
    }
}

#[test]
fn patch_status_reads_amended_as_queued() {
    assert_eq!(
        serde_json::from_str::<PatchStatus>("\"amended\"").unwrap(),
        PatchStatus::Queued
    );
}
