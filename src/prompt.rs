//! Line prompts for `init`. Questions go to stderr so stdout stays
//! machine-readable.

use std::io::{BufRead, IsTerminal, Write};

use ratatui::crossterm::{
    cursor::MoveToColumn,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};

use crate::edit::{TextEdit, width};
use crate::error::{Error, Result};

/// Columns the answer needs beside the question before it moves to a line of
/// its own.
const MIN_ANSWER_WIDTH: usize = 20;

/// Ask one question. In a terminal the answer starts as `default` and is
/// edited in place, so clearing it answers empty. Elsewhere an empty line
/// keeps `default`.
pub fn ask_line(question: &str, default: &str) -> Result<String> {
    if std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && terminal::enable_raw_mode().is_ok()
    {
        let _raw = RawMode;
        return edit_line(question, default);
    }
    read_line(question, default)
}

struct RawMode;

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

fn edit_line(question: &str, default: &str) -> Result<String> {
    let mut err = std::io::stderr();
    let mut edit = TextEdit::new(default);
    let mut label = format!("{question}: ");
    if columns() < width(&label) + MIN_ANSWER_WIDTH {
        write!(err, "{question}:\r\n")?;
        label = "> ".to_string();
    }
    let label_width = width(&label);
    let answer = loop {
        // The last column stays free: a cursor there would wrap the line.
        let room = columns().saturating_sub(label_width + 1).max(1);
        let view = edit.view(room, 1);
        queue!(
            err,
            MoveToColumn(0),
            Print(&label),
            Print(view.lines.first().map_or("", String::as_str)),
            Clear(ClearType::UntilNewLine),
            MoveToColumn(label_width as u16 + view.cursor.0),
        )?;
        err.flush()?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => break Ok(edit.text().trim().to_string()),
            KeyCode::Char('c') if ctrl => break Err(Error::msg("interrupted")),
            KeyCode::Char('d') if ctrl && edit.text().is_empty() => {
                break Err(Error::msg("interrupted"));
            }
            _ => {
                edit.handle_key(key);
            }
        }
    };
    write!(err, "\r\n")?;
    err.flush()?;
    answer
}

/// One button of [`choose`]. `key` picks it directly and is shown underlined
/// when it is a letter of `label`.
#[derive(Debug, Clone, Copy)]
pub struct Choice {
    pub label: &'static str,
    pub key: char,
}

/// What a key does to a row of buttons. Up and Down are left to the caller,
/// which may scroll with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Move the highlight to this button.
    Move(usize),
    /// This button is picked.
    Picked(usize),
    /// Esc or Ctrl+c: nothing is picked.
    Cancel,
    Ignore,
}

/// What `key` does with `selected` highlighted among `choices`. ←/→ and
/// Tab move and wrap around, Enter picks the highlighted button, a button's
/// key picks it directly.
pub fn next_selection(choices: &[Choice], selected: usize, code: KeyCode, ctrl: bool) -> Selection {
    let n = choices.len();
    if n == 0 {
        return Selection::Cancel;
    }
    match code {
        KeyCode::Char('c') if ctrl => Selection::Cancel,
        KeyCode::Esc => Selection::Cancel,
        KeyCode::Enter => Selection::Picked(selected.min(n - 1)),
        KeyCode::Left | KeyCode::BackTab => Selection::Move((selected + n - 1) % n),
        KeyCode::Right | KeyCode::Tab => Selection::Move((selected + 1) % n),
        KeyCode::Char(c) => choices
            .iter()
            .position(|choice| choice.key.eq_ignore_ascii_case(&c))
            .map_or(Selection::Ignore, Selection::Picked),
        _ => Selection::Ignore,
    }
}

fn columns() -> usize {
    terminal::size().map_or(80, |(columns, _)| columns as usize)
}

fn read_line(question: &str, default: &str) -> Result<String> {
    let mut err = std::io::stderr();
    if default.is_empty() {
        write!(err, "{question}: ")?;
    } else {
        write!(err, "{question} [{default}]: ")?;
    }
    err.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
}

#[cfg(test)]
mod tests {
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
}
