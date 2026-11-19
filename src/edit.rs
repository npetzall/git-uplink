//! Text being typed, with a cursor: the editing behind the line prompts and
//! the adopt form. Movement is per `char`, columns are terminal cells.

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    text::Span,
};

pub struct TextEdit {
    text: String,
    /// Byte offset into `text`, always on a char boundary.
    cursor: usize,
    multiline: bool,
    /// First line and first column shown by the last `view`.
    top: usize,
    left: usize,
}

/// What fits in the window, and where the cursor is inside it.
pub struct View {
    pub lines: Vec<String>,
    /// Column and row, relative to the window.
    pub cursor: (u16, u16),
}

impl TextEdit {
    /// One line. The cursor starts at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            multiline: false,
            top: 0,
            left: 0,
        }
    }

    /// Several lines: Enter inserts a newline and Up/Down move between lines.
    pub fn multiline(text: impl Into<String>) -> Self {
        Self {
            multiline: true,
            ..Self::new(text)
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Applies an editing key. False when the key is not one, so the caller
    /// can give it another meaning.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if ctrl && !alt => match c {
                'a' => self.home(),
                'e' => self.end(),
                'b' => self.left(),
                'f' => self.right(),
                'd' => self.delete(),
                'h' => self.backspace(),
                'u' => self.kill_to_start(),
                'k' => self.kill_to_end(),
                'w' => self.kill_word_back(),
                _ => return false,
            },
            KeyCode::Char(c) if alt && !ctrl => match c {
                'b' => self.word_left(),
                'f' => self.word_right(),
                _ => return false,
            },
            KeyCode::Char(c) if !c.is_control() => self.insert(c),
            KeyCode::Enter if self.multiline => self.insert('\n'),
            KeyCode::Left if ctrl || alt => self.word_left(),
            KeyCode::Right if ctrl || alt => self.word_right(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Up if self.multiline => self.up(),
            KeyCode::Down if self.multiline => self.down(),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            KeyCode::Backspace if alt => self.kill_word_back(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            _ => return false,
        }
        true
    }

    /// Scrolls just enough to keep the cursor inside a `width` by `height`
    /// window and returns what that window shows.
    pub fn view(&mut self, width: usize, height: usize) -> View {
        if width == 0 || height == 0 {
            return View {
                lines: Vec::new(),
                cursor: (0, 0),
            };
        }
        let (row, col) = self.cursor_line_col();
        if row < self.top {
            self.top = row;
        } else if row >= self.top + height {
            self.top = row + 1 - height;
        }
        if col < self.left {
            self.left = col;
        } else if col >= self.left + width {
            self.left = col + 1 - width;
        }
        let lines = self
            .lines()
            .into_iter()
            .skip(self.top)
            .take(height)
            .map(|line| clip(line, self.left, width))
            .collect();
        View {
            lines,
            cursor: ((col - self.left) as u16, (row - self.top) as u16),
        }
    }

    /// The cursor is on the first line, where Up has nowhere to go.
    pub fn on_first_line(&self) -> bool {
        self.line_start() == 0
    }

    /// Moves the cursor to column `col` of row `row` of the window the last
    /// [`Self::view`] showed, as a mouse click there does. A row past the
    /// last line goes to the last line, a column past its end to its end.
    pub fn place_cursor(&mut self, col: usize, row: usize) {
        let lines = self.lines();
        let line = (self.top + row).min(lines.len().saturating_sub(1));
        let start: usize = lines[..line].iter().map(|l| l.len() + 1).sum();
        let at = offset_at(lines[line], self.left + col);
        self.cursor = start + at;
    }

    fn lines(&self) -> Vec<&str> {
        if self.multiline {
            self.text.split('\n').collect()
        } else {
            vec![&self.text]
        }
    }

    fn cursor_line_col(&self) -> (usize, usize) {
        let row = if self.multiline {
            self.text[..self.cursor].matches('\n').count()
        } else {
            0
        };
        (row, width(&self.text[self.line_start()..self.cursor]))
    }

    fn line_start(&self) -> usize {
        if !self.multiline {
            return 0;
        }
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        if !self.multiline {
            return self.text.len();
        }
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }

    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        let at = self.cursor;
        self.left();
        self.text.drain(self.cursor..at);
    }

    fn delete(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.text.drain(self.cursor..self.cursor + c.len_utf8());
        }
    }

    fn left(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    fn home(&mut self) {
        self.cursor = self.line_start();
    }

    fn end(&mut self) {
        self.cursor = self.line_end();
    }

    fn up(&mut self) {
        let start = self.line_start();
        if start == 0 {
            return;
        }
        let col = width(&self.text[start..self.cursor]);
        let prev_end = start - 1;
        let prev_start = self.text[..prev_end].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = prev_start + offset_at(&self.text[prev_start..prev_end], col);
    }

    fn down(&mut self) {
        let end = self.line_end();
        if end == self.text.len() {
            return;
        }
        let col = width(&self.text[self.line_start()..self.cursor]);
        let next_start = end + 1;
        let next_end = self.text[next_start..]
            .find('\n')
            .map_or(self.text.len(), |i| next_start + i);
        self.cursor = next_start + offset_at(&self.text[next_start..next_end], col);
    }

    /// To the start of the word before the cursor.
    fn word_left(&mut self) {
        let before = &self.text[..self.cursor];
        let word_end = before.trim_end().len();
        self.cursor = before[..word_end]
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
    }

    /// To the end of the word after the cursor.
    fn word_right(&mut self) {
        let after = &self.text[self.cursor..];
        let gap = after.len() - after.trim_start().len();
        self.cursor += after[gap..]
            .find(char::is_whitespace)
            .map_or(after.len(), |i| gap + i);
    }

    fn kill_to_start(&mut self) {
        let start = self.line_start();
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    fn kill_to_end(&mut self) {
        self.text.drain(self.cursor..self.line_end());
    }

    fn kill_word_back(&mut self) {
        let at = self.cursor;
        self.word_left();
        self.text.drain(self.cursor..at);
    }
}

/// Terminal cells `text` takes.
pub fn width(text: &str) -> usize {
    Span::raw(text).width()
}

fn char_width(c: char) -> usize {
    width(c.encode_utf8(&mut [0; 4]))
}

/// Byte offset in `line` of the char at column `col`, or its end.
fn offset_at(line: &str, col: usize) -> usize {
    let mut at = 0;
    for (i, c) in line.char_indices() {
        if at >= col {
            return i;
        }
        at += char_width(c);
    }
    line.len()
}

/// The part of `line` in columns `left..left + width`. A wide char cut by the
/// left edge becomes spaces so the columns after it stay in place.
pub(crate) fn clip(line: &str, left: usize, width: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in line.chars() {
        let w = char_width(c);
        if col + w > left + width {
            break;
        }
        if col >= left {
            out.push(c);
        } else if col + w > left {
            out.extend(std::iter::repeat_n(' ', col + w - left));
        }
        col += w;
    }
    out
}

#[cfg(test)]
#[path = "edit_test.rs"]
mod tests;
