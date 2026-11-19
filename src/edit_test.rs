use super::*;

fn press(edit: &mut TextEdit, code: KeyCode) -> bool {
    edit.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(edit: &mut TextEdit, c: char) -> bool {
    edit.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn alt(edit: &mut TextEdit, c: char) -> bool {
    edit.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
}

#[test]
fn typing_inserts_at_the_cursor() {
    let mut edit = TextEdit::new("bc");
    press(&mut edit, KeyCode::Home);
    press(&mut edit, KeyCode::Char('a'));
    assert_eq!(edit.text(), "abc");
    assert_eq!(edit.cursor_line_col(), (0, 1));
    press(&mut edit, KeyCode::Right);
    press(&mut edit, KeyCode::Char('x'));
    assert_eq!(edit.text(), "abxc");
}

#[test]
fn backspace_and_delete_stop_at_the_ends() {
    let mut edit = TextEdit::new("ab");
    press(&mut edit, KeyCode::Delete);
    assert_eq!(edit.text(), "ab");
    press(&mut edit, KeyCode::Home);
    press(&mut edit, KeyCode::Backspace);
    assert_eq!(edit.text(), "ab");
    press(&mut edit, KeyCode::Delete);
    assert_eq!(edit.text(), "b");
    press(&mut edit, KeyCode::End);
    press(&mut edit, KeyCode::Backspace);
    assert_eq!(edit.text(), "");
}

#[test]
fn moves_over_multi_byte_and_wide_chars() {
    let mut edit = TextEdit::new("åäö");
    press(&mut edit, KeyCode::Left);
    press(&mut edit, KeyCode::Backspace);
    assert_eq!(edit.text(), "åö");
    assert_eq!(edit.cursor_line_col(), (0, 1));

    let edit = TextEdit::new("日本");
    assert_eq!(edit.cursor_line_col(), (0, 4));
}

#[test]
fn up_and_down_keep_the_column_where_the_line_allows() {
    let mut edit = TextEdit::multiline("abcdef\nxy\n12345");
    assert_eq!(edit.cursor_line_col(), (2, 5));
    press(&mut edit, KeyCode::Up);
    assert_eq!(edit.cursor_line_col(), (1, 2));
    press(&mut edit, KeyCode::Up);
    assert_eq!(edit.cursor_line_col(), (0, 2));
    press(&mut edit, KeyCode::Up);
    assert_eq!(edit.cursor_line_col(), (0, 2));
    press(&mut edit, KeyCode::End);
    press(&mut edit, KeyCode::Down);
    press(&mut edit, KeyCode::Down);
    assert_eq!(edit.cursor_line_col(), (2, 2));
    press(&mut edit, KeyCode::Down);
    assert_eq!(edit.cursor_line_col(), (2, 2));
}

#[test]
fn home_end_and_kills_stay_on_the_line() {
    let mut edit = TextEdit::multiline("ab\ncd");
    press(&mut edit, KeyCode::Home);
    assert_eq!(edit.cursor_line_col(), (1, 0));
    press(&mut edit, KeyCode::End);
    assert_eq!(edit.cursor_line_col(), (1, 2));
    press(&mut edit, KeyCode::Left);
    ctrl(&mut edit, 'u');
    assert_eq!(edit.text(), "ab\nd");
    press(&mut edit, KeyCode::Up);
    ctrl(&mut edit, 'k');
    assert_eq!(edit.text(), "\nd");
}

#[test]
fn enter_splits_the_line_only_when_multiline() {
    let mut edit = TextEdit::multiline("abcd");
    press(&mut edit, KeyCode::Left);
    press(&mut edit, KeyCode::Left);
    assert!(press(&mut edit, KeyCode::Enter));
    assert_eq!(edit.text(), "ab\ncd");
    assert_eq!(edit.cursor_line_col(), (1, 0));

    let mut edit = TextEdit::new("abcd");
    assert!(!press(&mut edit, KeyCode::Enter));
    assert!(!press(&mut edit, KeyCode::Up));
    assert_eq!(edit.text(), "abcd");
}

#[test]
fn word_keys_move_and_delete_by_word() {
    let mut edit = TextEdit::new("foo bar  baz");
    alt(&mut edit, 'b');
    assert_eq!(edit.cursor_line_col(), (0, 9));
    alt(&mut edit, 'b');
    assert_eq!(edit.cursor_line_col(), (0, 4));
    alt(&mut edit, 'f');
    assert_eq!(edit.cursor_line_col(), (0, 7));
    alt(&mut edit, 'f');
    assert_eq!(edit.cursor_line_col(), (0, 12));
    ctrl(&mut edit, 'w');
    assert_eq!(edit.text(), "foo bar  ");
    ctrl(&mut edit, 'w');
    assert_eq!(edit.text(), "foo ");
}

#[test]
fn unbound_keys_are_left_to_the_caller() {
    let mut edit = TextEdit::new("a");
    assert!(!ctrl(&mut edit, 's'));
    assert!(!alt(&mut edit, 'x'));
    assert!(!press(&mut edit, KeyCode::Tab));
    assert!(!press(&mut edit, KeyCode::Esc));
    assert_eq!(edit.text(), "a");
}

#[test]
fn view_scrolls_sideways_to_the_cursor() {
    let mut edit = TextEdit::new("0123456789");
    let view = edit.view(5, 1);
    assert_eq!(view.lines, ["6789"]);
    assert_eq!(view.cursor, (4, 0));
    press(&mut edit, KeyCode::Left);
    let view = edit.view(5, 1);
    assert_eq!(view.lines, ["6789"]);
    assert_eq!(view.cursor, (3, 0));
    press(&mut edit, KeyCode::Home);
    let view = edit.view(5, 1);
    assert_eq!(view.lines, ["01234"]);
    assert_eq!(view.cursor, (0, 0));
}

#[test]
fn view_scrolls_down_to_the_cursor() {
    let mut edit = TextEdit::multiline("a\nb\nc\nd");
    let view = edit.view(10, 2);
    assert_eq!(view.lines, ["c", "d"]);
    assert_eq!(view.cursor, (1, 1));
    for _ in 0..3 {
        press(&mut edit, KeyCode::Up);
    }
    let view = edit.view(10, 2);
    assert_eq!(view.lines, ["a", "b"]);
    assert_eq!(view.cursor, (1, 0));
    assert!(edit.view(0, 2).lines.is_empty());
}

#[test]
fn on_first_line_is_where_up_has_nowhere_to_go() {
    let mut edit = TextEdit::multiline("ab\ncd");
    assert!(!edit.on_first_line());
    press(&mut edit, KeyCode::Up);
    assert!(edit.on_first_line());
    assert!(TextEdit::new("one line").on_first_line());
}

#[test]
fn place_cursor_maps_a_click_through_the_scroll() {
    let mut edit = TextEdit::multiline("first\nsecond\nthird\nfourth");
    edit.view(10, 2);
    // The view shows "third" and "fourth"; a click on row 0, col 2 is "ir|d".
    edit.place_cursor(2, 0);
    assert_eq!(edit.cursor_line_col(), (2, 2));
    edit.place_cursor(40, 1);
    assert_eq!(edit.cursor_line_col(), (3, 6));
    edit.place_cursor(0, 9);
    assert_eq!(edit.cursor_line_col(), (3, 0));

    let mut edit = TextEdit::new("0123456789");
    edit.view(5, 1);
    edit.place_cursor(1, 0);
    assert_eq!(edit.cursor_line_col(), (0, 7));
}

#[test]
fn clip_pads_a_wide_char_cut_by_the_left_edge() {
    assert_eq!(clip("日本語", 1, 4), " 本");
    assert_eq!(clip("abcdef", 2, 3), "cde");
}
