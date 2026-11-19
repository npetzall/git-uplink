use super::*;

const CHOICES: [Choice; 3] = [
    Choice {
        label: "Ignore",
        key: 'i',
    },
    Choice {
        label: "Amend",
        key: 'a',
    },
    Choice {
        label: "Abort",
        key: 'q',
    },
];

#[test]
fn arrows_and_tab_move_and_wrap() {
    let next = |selected, code| next_selection(&CHOICES, selected, code, false);
    assert_eq!(next(0, KeyCode::Right), Selection::Move(1));
    assert_eq!(next(2, KeyCode::Tab), Selection::Move(0));
    assert_eq!(next(0, KeyCode::Left), Selection::Move(2));
    assert_eq!(next(1, KeyCode::BackTab), Selection::Move(0));
}

#[test]
fn enter_and_keys_pick() {
    let next = |selected, code| next_selection(&CHOICES, selected, code, false);
    assert_eq!(next(1, KeyCode::Enter), Selection::Picked(1));
    assert_eq!(next(0, KeyCode::Char('q')), Selection::Picked(2));
    assert_eq!(next(0, KeyCode::Char('A')), Selection::Picked(1));
    assert_eq!(next(0, KeyCode::Char('x')), Selection::Ignore);
}

#[test]
fn esc_and_ctrl_c_cancel() {
    assert_eq!(
        next_selection(&CHOICES, 0, KeyCode::Esc, false),
        Selection::Cancel
    );
    assert_eq!(
        next_selection(&CHOICES, 0, KeyCode::Char('c'), true),
        Selection::Cancel
    );
}
