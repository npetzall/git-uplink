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
