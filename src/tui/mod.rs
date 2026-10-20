//! Terminal windows: the adopt tree (`init` and `adopt --branch`) and the
//! window a failing adopt group opens. Both take keys and the mouse.

use std::io::stdout;
use std::path::Path;

use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
            KeyModifiers, MouseButton, MouseEventKind,
        },
        execute,
    },
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

use crate::adopt::{AdoptDecision, AdoptFailure, AdoptFailureKind, AdoptGroup, AheadAnalysis};
use crate::edit::width;
use crate::error::Result;
use crate::prompt::{Choice, Selection, next_selection};
use crate::types::QueueState;

mod adopt;
mod tree;

/// How the operator left the adopt UI.
pub enum TuiOutcome {
    /// Adopt these groups now.
    Adopt(Vec<AdoptGroup>),
    /// Only write the groups file (adopting from a branch).
    WriteAndExit(Vec<AdoptGroup>),
}

impl TuiOutcome {
    pub fn into_groups(self) -> Vec<AdoptGroup> {
        match self {
            Self::Adopt(groups) | Self::WriteAndExit(groups) => groups,
        }
    }
}

/// Groups the commits of `analysis` in a tree beside a preview of each
/// commit. `analysis.allow_skip` tells `adopt --branch` (commits can be
/// skipped, and the groups written to `groups_file` without adopting) from
/// `init` (every commit is adopted).
pub fn run_adopt(
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
    groups_file: Option<&Path>,
) -> Result<TuiOutcome> {
    if analysis.commits.is_empty() {
        return Err(crate::error::Error::msg(
            "no unique first-parent commits to adopt",
        ));
    }
    let mut terminal = init_terminal();
    let result = adopt::run(&mut terminal, repo, queue, analysis, groups_file);
    restore_terminal();
    result
}

/// The full-screen terminal, with mouse events.
fn init_terminal() -> DefaultTerminal {
    let terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture);
    terminal
}

fn restore_terminal() {
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
}

/// A row of `[ Label ]` buttons with `selected` reversed and each key
/// underlined, and the column span of each button from the line's start.
fn buttons(choices: &[Choice], selected: usize) -> (Line<'static>, Vec<(u16, u16)>) {
    let mut spans = Vec::new();
    let mut spots = Vec::new();
    let mut col = 0;
    for (i, choice) in choices.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
            col += 2;
        }
        let base = if i == selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let label_width = width(choice.label) as u16 + 4;
        spots.push((col, label_width));
        col += label_width;
        spans.push(Span::styled("[ ", base));
        let mut underlined = false;
        for c in choice.label.chars() {
            let style = if !underlined && c.eq_ignore_ascii_case(&choice.key) {
                underlined = true;
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            };
            spans.push(Span::styled(c.to_string(), style));
        }
        spans.push(Span::styled(" ]", base));
    }
    (Line::from(spans), spots)
}

/// The button of `spots` (from [`buttons`] drawn at column `left`) under column `x`.
fn button_at(spots: &[(u16, u16)], left: u16, x: u16) -> Option<usize> {
    let x = x.checked_sub(left)?;
    spots
        .iter()
        .position(|(start, w)| x >= *start && x < start + w)
}

/// A `width` by `height` rectangle in the middle of `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
}

/// The first of `len` output lines shown in `height` rows when `top` is
/// asked for: never past the point where the last line reaches the bottom.
fn clamp_top(top: usize, len: usize, height: usize) -> usize {
    top.min(len.saturating_sub(height))
}

/// Shows a failing adopt group, with what `preflight.sh` printed or why it
/// does not apply, and asks what to do with it. Esc aborts.
pub fn choose_on_failure(
    failure: &AdoptFailure,
    options: &[(Choice, AdoptDecision)],
) -> Result<AdoptDecision> {
    let mut terminal = init_terminal();
    let result = failure_loop(&mut terminal, failure, options);
    restore_terminal();
    result
}

/// Where the failure window drew what can be clicked or scrolled.
#[derive(Default)]
struct FailureSpots {
    output: Rect,
    buttons_at: (u16, u16),
    buttons: Vec<(u16, u16)>,
}

fn failure_loop(
    terminal: &mut DefaultTerminal,
    failure: &AdoptFailure,
    options: &[(Choice, AdoptDecision)],
) -> Result<AdoptDecision> {
    let output: Vec<&str> = failure.output.lines().collect();
    let choices: Vec<Choice> = options.iter().map(|(choice, _)| *choice).collect();
    let mut selected = 0;
    // Starts at the end, where a failing script says why.
    let mut top = usize::MAX;
    let mut spots = FailureSpots::default();
    loop {
        terminal.draw(|frame| {
            spots = draw_failure(frame, failure, &output, &choices, selected, &mut top);
        })?;
        let height = spots.output.height as usize;
        let shown = clamp_top(top, output.len(), height);
        match event::read()? {
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => top = shown.saturating_sub(3),
                MouseEventKind::ScrollDown => top = clamp_top(shown + 3, output.len(), height),
                MouseEventKind::Down(MouseButton::Left) if mouse.row == spots.buttons_at.1 => {
                    if let Some(i) = button_at(&spots.buttons, spots.buttons_at.0, mouse.column) {
                        return Ok(options[i].1);
                    }
                }
                _ => {}
            },
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let page = height.saturating_sub(1).max(1);
                match key.code {
                    KeyCode::Up => top = shown.saturating_sub(1),
                    KeyCode::Down => top = clamp_top(shown + 1, output.len(), height),
                    KeyCode::PageUp => top = shown.saturating_sub(page),
                    KeyCode::PageDown => top = clamp_top(shown + page, output.len(), height),
                    KeyCode::Home => top = 0,
                    KeyCode::End => top = usize::MAX,
                    code => {
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        match next_selection(&choices, selected, code, ctrl) {
                            Selection::Move(i) => selected = i,
                            Selection::Picked(i) => return Ok(options[i].1),
                            Selection::Cancel => return Ok(AdoptDecision::Abort),
                            Selection::Ignore => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn draw_failure(
    frame: &mut Frame,
    failure: &AdoptFailure,
    output: &[&str],
    choices: &[Choice],
    selected: usize,
    top: &mut usize,
) -> FailureSpots {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(frame.area());
    let what = match failure.kind {
        AdoptFailureKind::Apply => "Does not apply",
        AdoptFailureKind::Preflight => "Preflight failed",
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{} - {} - {}\n{}",
            failure.id, failure.sha, failure.title, failure.message
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{what}: group {}", failure.group))
                .border_style(Style::default().fg(Color::Red)),
        ),
        chunks[0],
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(match failure.kind {
            AdoptFailureKind::Apply => "Why",
            AdoptFailureKind::Preflight => "preflight.sh output",
        });
    let inner = block.inner(chunks[1]);
    let height = inner.height as usize;
    *top = clamp_top(*top, output.len(), height);
    let shown = output
        .iter()
        .skip(*top)
        .take(height)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(Paragraph::new(shown).block(block), chunks[1]);

    let (line, spots) = buttons(choices, selected);
    let mut hint =
        "←/→ choose · Enter or click confirm · ↑/↓ PgUp/PgDn or wheel scroll · Esc abort"
            .to_string();
    if !choices.iter().any(|c| c.label == "Amend") {
        hint.push_str(" · Amend needs a checkout without uncommitted changes");
    }
    let block = Block::default().borders(Borders::ALL).title("What now?");
    let inner_buttons = block.inner(chunks[2]);
    frame.render_widget(
        Paragraph::new(vec![
            line,
            Line::styled(hint, Style::default().add_modifier(Modifier::DIM)),
        ])
        .block(block),
        chunks[2],
    );
    FailureSpots {
        output: inner,
        buttons_at: (inner_buttons.x, inner_buttons.y),
        buttons: spots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
