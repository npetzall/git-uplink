use super::*;

#[test]
fn groups_file_hint_names_the_file_and_the_flag_to_adopt_with() {
    let path = Path::new("/repo/.git/uplink/adopt/main-fork.json");
    let branch = groups_file_hint(path, true);
    assert!(branch.contains("/repo/.git/uplink/adopt/main-fork.json"));
    assert!(branch.contains("`git uplink adopt --branch <ref> --groups`"));
    assert!(branch.contains("--apply"));
    assert!(groups_file_hint(path, false).contains("`git uplink init --adopt-groups`"));
}

#[test]
fn a_failing_group_keeps_its_titles_and_hint() {
    let failure = AdoptFailure {
        kind: AdoptFailureKind::Preflight,
        item: "group 2".into(),
        group: 2,
        id: "upl_1".into(),
        sha: "abc1234".into(),
        title: "Add notes".into(),
        message: "preflight.sh failed".into(),
        output: "boom".into(),
        can_amend: false,
        can_leave_out: true,
        internal_only: false,
    };
    let window = failure_window(&failure, false);
    assert_eq!(window.title, "Preflight failed: group 2");
    assert_eq!(
        window.header,
        "upl_1 - abc1234 - Add notes\npreflight.sh failed"
    );
    assert_eq!(window.body_title, "preflight.sh output");
    assert_eq!(window.question, "What now?");
    assert_eq!(
        choice_hint(&window),
        "←/→ choose · Enter or click confirm · ↑/↓ PgUp/PgDn or wheel scroll · Esc abort \
· Amend needs a checkout without uncommitted changes"
    );
    assert!(failure_window(&failure, true).hint.is_none());
}

#[test]
fn text_wraps_at_spaces_and_keeps_indentation() {
    assert_eq!(wrap_text("short", 10), ["short"]);
    assert_eq!(
        wrap_text("one two three four", 9),
        ["one two", "three", "four"]
    );
    assert_eq!(wrap_text("  ab cd ef", 6), ["  ab", "  cd", "  ef"]);
    assert_eq!(wrap_text("abcdefgh", 3), ["abc", "def", "gh"]);
    assert_eq!(wrap_text("a\n\nb", 5), ["a", "", "b"]);
    assert_eq!(wrap_text("", 5), [""]);
}

#[test]
fn the_cancel_button_is_highlighted_first() {
    let button = |label, key| Choice { label, key };
    let yes_no = [(button("Yes", 'y'), true), (button("No", 'n'), false)];
    assert_eq!(initial_selection(&yes_no, false), 1);
    // A cancel value without a button highlights the first.
    let failure = [
        (button("Skip", 's'), AdoptDecision::Skip),
        (button("Stop", 't'), AdoptDecision::Stop),
    ];
    assert_eq!(initial_selection(&failure, AdoptDecision::Abort), 0);
}

#[test]
fn output_scroll_stops_at_both_ends() {
    assert_eq!(clamp_top(usize::MAX, 50, 10), 40);
    assert_eq!(clamp_top(45, 50, 10), 40);
    assert_eq!(clamp_top(3, 50, 10), 3);
    assert_eq!(clamp_top(7, 5, 10), 0);
}

#[test]
fn clicks_land_on_the_button_under_them() {
    let choices = [
        Choice {
            label: "Ignore",
            key: 'i',
        },
        Choice {
            label: "Skip",
            key: 's',
        },
    ];
    let (_, spots) = buttons(&choices, 0);
    // "[ Ignore ]  [ Skip ]" drawn from column 5.
    assert_eq!(spots, [(0, 10), (12, 8)]);
    assert_eq!(button_at(&spots, 5, 5), Some(0));
    assert_eq!(button_at(&spots, 5, 14), Some(0));
    assert_eq!(button_at(&spots, 5, 16), None);
    assert_eq!(button_at(&spots, 5, 17), Some(1));
    assert_eq!(button_at(&spots, 5, 3), None);
}
