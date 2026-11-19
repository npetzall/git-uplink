use super::*;

fn event(at: &str, kind: &str, detail: &str) -> PatchEvent {
    PatchEvent {
        at: at.into(),
        kind: kind.into(),
        detail: detail.into(),
    }
}

#[test]
fn merge_events_keeps_repeated_approvals() {
    let first = event("2026-01-01T00:00:00Z", "approved", "delta approval");
    let second = event("2026-01-02T00:00:00Z", "approved", "delta approval");
    let mut dest = vec![first.clone()];
    merge_events(&mut dest, vec![first.clone(), second.clone()]);
    assert_eq!(dest, vec![first, second]);
}
