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
use crate::assess::cutoff_marker;
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
///
/// `groups` (from a groups file) are shown as they are; without them each
/// commit starts in a group of its own. The groups as first shown are
/// written to `groups_file` when the window opens. When the operator quits,
/// its path is printed on the way out, so it can be edited and passed back.
pub fn run_adopt(
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
    groups_file: &Path,
    groups: Option<&[AdoptGroup]>,
) -> Result<TuiOutcome> {
    if analysis.commits.is_empty() {
        return Err(crate::error::Error::msg(
            "no unique first-parent commits to adopt",
        ));
    }
    // Before the terminal is taken, so a bad file is reported plainly.
    let tree = match groups {
        Some(groups) => {
            let resolved = crate::adopt::resolve_groups(analysis, groups)?;
            tree::AdoptTree::from_groups(
                analysis.commits.clone(),
                analysis.allow_skip,
                &cutoff_marker(queue),
                &resolved,
            )
            .map_err(crate::error::Error::msg)?
        }
        None => initial_tree(queue, analysis),
    };
    let mut terminal = init_terminal();
    let result = adopt::run(&mut terminal, repo, queue, analysis, groups_file, tree);
    restore_terminal();
    // Adopt and Ctrl+w report the file themselves; quitting does not.
    if result.is_err() && groups_file.is_file() {
        eprintln!("{}", groups_file_hint(groups_file, analysis.allow_skip));
    }
    result
}

/// The groups the adopt UI first shows: each commit in a group of its own,
/// except that, adopting from a branch, a commit already on main is skipped.
pub fn initial_groups(queue: &QueueState, analysis: &AheadAnalysis) -> Vec<AdoptGroup> {
    initial_tree(queue, analysis).to_groups()
}

fn initial_tree(queue: &QueueState, analysis: &AheadAnalysis) -> tree::AdoptTree {
    tree::AdoptTree::new(
        analysis.commits.clone(),
        analysis.allow_skip,
        &cutoff_marker(queue),
    )
}

/// Where the groups file is, and how to open or adopt it after editing.
fn groups_file_hint(groups_file: &Path, from_branch: bool) -> String {
    let flag = if from_branch {
        "git uplink adopt --branch <ref> --groups"
    } else {
        "git uplink init --adopt-groups"
    };
    format!(
        "Groups file: {}\nEdit it, then `{flag}` opens it again; add `--apply` to adopt it without the UI.",
        groups_file.display()
    )
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

/// A window that asks one question: a header box, a body that scrolls,
/// and a row of buttons under it.
#[derive(Debug, Clone)]
pub struct ChoiceWindow {
    /// Title of the header box.
    pub title: String,
    pub header: String,
    /// Border color of the header box.
    pub border: Color,
    /// Title of the body box.
    pub body_title: String,
    pub body: String,
    /// Title of the button box.
    pub question: String,
    /// Added to the key help under the buttons.
    pub hint: Option<String>,
    /// What Esc does, as the key help names it.
    pub cancel_label: &'static str,
}

/// Shows `window` and asks to pick one of `options`. Esc and Ctrl+c
/// return `cancel`, which is also highlighted first when it is a button.
pub fn ask<T: Copy + PartialEq>(
    window: &ChoiceWindow,
    options: &[(Choice, T)],
    cancel: T,
) -> Result<T> {
    let mut terminal = init_terminal();
    let result = choice_loop(&mut terminal, window, options, cancel);
    restore_terminal();
    result
}

/// Shows a failing adopt group, with what `preflight.sh` printed or why it
/// does not apply, and asks what to do with it. Esc aborts.
pub fn choose_on_failure(
    failure: &AdoptFailure,
    options: &[(Choice, AdoptDecision)],
) -> Result<AdoptDecision> {
    let can_amend = options.iter().any(|(choice, _)| choice.label == "Amend");
    ask(
        &failure_window(failure, can_amend),
        options,
        AdoptDecision::Abort,
    )
}

fn failure_window(failure: &AdoptFailure, can_amend: bool) -> ChoiceWindow {
    let what = match failure.kind {
        AdoptFailureKind::Apply => "Does not apply",
        AdoptFailureKind::Preflight => "Preflight failed",
        AdoptFailureKind::Stack if failure.internal_only => "Does not apply on company main",
        AdoptFailureKind::Stack => "Does not apply on origin's queue",
        AdoptFailureKind::CompanyPreflight => "Preflight failed on company main",
        AdoptFailureKind::Planned => "Stopped to amend",
    };
    let body_title = match failure.kind {
        AdoptFailureKind::Apply | AdoptFailureKind::Stack | AdoptFailureKind::Planned => "Why",
        AdoptFailureKind::Preflight | AdoptFailureKind::CompanyPreflight => "preflight.sh output",
    };
    ChoiceWindow {
        title: format!("{what}: {}", failure.item),
        header: format!(
            "{} - {} - {}\n{}",
            failure.id, failure.sha, failure.title, failure.message
        ),
        border: Color::Red,
        body_title: body_title.to_string(),
        body: failure.output.clone(),
        question: "What now?".to_string(),
        hint: (!can_amend)
            .then(|| "Amend needs a checkout without uncommitted changes".to_string()),
        cancel_label: "abort",
    }
}

/// Where the window drew what can be clicked or scrolled.
#[derive(Default)]
struct ChoiceSpots {
    body: Rect,
    /// Lines of the body as wrapped to its width.
    body_len: usize,
    buttons_at: (u16, u16),
    buttons: Vec<(u16, u16)>,
}

fn choice_loop<T: Copy + PartialEq>(
    terminal: &mut DefaultTerminal,
    window: &ChoiceWindow,
    options: &[(Choice, T)],
    cancel: T,
) -> Result<T> {
    let choices: Vec<Choice> = options.iter().map(|(choice, _)| *choice).collect();
    let mut selected = initial_selection(options, cancel);
    // Starts at the end, where a failing script says why.
    let mut top = usize::MAX;
    let mut spots = ChoiceSpots::default();
    loop {
        terminal.draw(|frame| {
            spots = draw_choice(frame, window, &choices, selected, &mut top);
        })?;
        let height = spots.body.height as usize;
        let lines = spots.body_len;
        let shown = clamp_top(top, lines, height);
        match event::read()? {
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => top = shown.saturating_sub(3),
                MouseEventKind::ScrollDown => top = clamp_top(shown + 3, lines, height),
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
                    KeyCode::Down => top = clamp_top(shown + 1, lines, height),
                    KeyCode::PageUp => top = shown.saturating_sub(page),
                    KeyCode::PageDown => top = clamp_top(shown + page, lines, height),
                    KeyCode::Home => top = 0,
                    KeyCode::End => top = usize::MAX,
                    code => {
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        match next_selection(&choices, selected, code, ctrl) {
                            Selection::Move(i) => selected = i,
                            Selection::Picked(i) => return Ok(options[i].1),
                            Selection::Cancel => return Ok(cancel),
                            Selection::Ignore => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// The key help under the buttons.
fn choice_hint(window: &ChoiceWindow) -> String {
    let mut hint = format!(
        "←/→ choose · Enter or click confirm · ↑/↓ PgUp/PgDn or wheel scroll · Esc {}",
        window.cancel_label
    );
    if let Some(extra) = &window.hint {
        hint.push_str(" · ");
        hint.push_str(extra);
    }
    hint
}

/// The button highlighted first: the one that cancels, else the first.
fn initial_selection<T: PartialEq>(options: &[(Choice, T)], cancel: T) -> usize {
    options
        .iter()
        .position(|(_, value)| *value == cancel)
        .unwrap_or(0)
}

/// `text` broken into lines of at most `columns`, at spaces where it can
/// be. A line's leading spaces stay; a word wider than `columns` is cut.
fn wrap_text(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut out = Vec::new();
    for line in text.lines() {
        if width(line) <= columns {
            out.push(line.to_string());
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        let mut current = indent.to_string();
        for word in line.trim_start().split(' ') {
            let gap = usize::from(width(&current) > width(indent));
            if width(&current) + gap + width(word) > columns && width(&current) > width(indent) {
                out.push(std::mem::replace(&mut current, indent.to_string()));
            } else if gap == 1 {
                current.push(' ');
            }
            for c in word.chars() {
                if width(&current) + width(&c.to_string()) > columns {
                    out.push(std::mem::take(&mut current));
                }
                current.push(c);
            }
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn draw_choice(
    frame: &mut Frame,
    window: &ChoiceWindow,
    choices: &[Choice],
    selected: usize,
    top: &mut usize,
) -> ChoiceSpots {
    let inner_width = frame.area().width.saturating_sub(2) as usize;
    let header = wrap_text(&window.header, inner_width);
    // The header never takes more than a third of the window.
    let header_lines = (header.len() as u16).min((frame.area().height / 3).max(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_lines + 2),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new(header.join("\n")).block(
            Block::default()
                .borders(Borders::ALL)
                .title(window.title.as_str())
                .border_style(Style::default().fg(window.border)),
        ),
        chunks[0],
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(window.body_title.as_str());
    let inner = block.inner(chunks[1]);
    let height = inner.height as usize;
    let body = wrap_text(&window.body, inner.width as usize);
    *top = clamp_top(*top, body.len(), height);
    let shown = body
        .iter()
        .skip(*top)
        .take(height)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(Paragraph::new(shown).block(block), chunks[1]);

    let (line, spots) = buttons(choices, selected);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(window.question.as_str());
    let inner_buttons = block.inner(chunks[2]);
    frame.render_widget(
        Paragraph::new(vec![
            line,
            Line::styled(
                choice_hint(window),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ])
        .block(block),
        chunks[2],
    );
    ChoiceSpots {
        body: inner,
        body_len: body.len(),
        buttons_at: (inner_buttons.x, inner_buttons.y),
        buttons: spots,
    }
}

#[cfg(test)]
mod tests {
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
}
