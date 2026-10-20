use std::path::Path;

use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::adopt::{
    AdoptCommit, AdoptDecision, AdoptFailure, AdoptFailureKind, AdoptGroup, AheadAnalysis,
    groups_from_numbers, resolve_groups,
};
use crate::assess::{assert_assess_ok, assess_from_message};
use crate::edit::TextEdit;
use crate::error::{Error, Result};
use crate::prompt::{Choice, Selection, next_selection};
use crate::types::{PatchIntent, QueueState};

enum Phase {
    Assign,
    Details { index: usize },
}

struct Form {
    title: TextEdit,
    intent: PatchIntent,
    message: TextEdit,
}

enum Field {
    Title,
    Intent,
    Message,
}

struct AdoptUi {
    commits: Vec<AdoptCommit>,
    numbers: Vec<u32>,
    selected: usize,
    typing: TextEdit,
    entering_number: bool,
    status: String,
    phase: Phase,
    forms: Vec<Form>,
    groups: Vec<AdoptGroup>,
    field: Field,
    /// `0` leaves a commit out (adopting from a branch).
    allow_skip: bool,
}

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

pub fn run_adopt(repo: &Path, queue: &QueueState, analysis: &AheadAnalysis) -> Result<TuiOutcome> {
    if analysis.commits.is_empty() {
        return Err(Error::msg("no unique first-parent commits to adopt"));
    }
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, repo, queue, analysis);
    ratatui::restore();
    result
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
) -> Result<TuiOutcome> {
    let allow_skip = analysis.allow_skip;
    let mut next = 0;
    let numbers = analysis
        .commits
        .iter()
        .map(|commit| {
            if allow_skip && commit.on_main {
                0
            } else {
                next += 1;
                next
            }
        })
        .collect();
    let status = if allow_skip {
        "1-9 set group, s skip, [ ] bump, g then digits, Enter to continue, q abort"
    } else {
        "1-9 set group, [ ] bump, g then digits, Enter to continue, q abort"
    };
    let mut ui = AdoptUi {
        commits: analysis.commits.clone(),
        numbers,
        selected: 0,
        typing: TextEdit::new(""),
        entering_number: false,
        status: status.into(),
        phase: Phase::Assign,
        forms: Vec::new(),
        groups: Vec::new(),
        field: Field::Title,
        allow_skip,
    };
    loop {
        terminal.draw(|frame| draw(frame, &mut ui))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match ui.phase {
            Phase::Assign => handle_assign(&mut ui, key)?,
            Phase::Details { .. } => {
                if let Some(outcome) = handle_details(&mut ui, repo, queue, analysis, key)? {
                    return Ok(outcome);
                }
            }
        }
    }
}

fn handle_assign(ui: &mut AdoptUi, key: KeyEvent) -> Result<()> {
    if ui.entering_number {
        match key.code {
            KeyCode::Enter => {
                if let Ok(n) = ui.typing.text().parse::<u32>() {
                    if n == 0 && !ui.allow_skip {
                        ui.status = "group numbers start at 1".into();
                    } else {
                        ui.numbers[ui.selected] = n;
                    }
                }
                ui.entering_number = false;
            }
            KeyCode::Esc => ui.entering_number = false,
            KeyCode::Char(c)
                if !c.is_ascii_digit()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {}
            _ => {
                ui.typing.handle_key(key);
            }
        }
        return Ok(());
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            return Err(Error::msg(if ui.allow_skip {
                "adopt aborted; nothing was changed"
            } else {
                "adopt grouping aborted; re-run git uplink init to continue"
            }));
        }
        KeyCode::Char('s' | '0') if ui.allow_skip => {
            ui.numbers[ui.selected] = 0;
        }
        KeyCode::Up => {
            if ui.selected > 0 {
                ui.selected -= 1;
            }
        }
        KeyCode::Down => {
            if ui.selected + 1 < ui.commits.len() {
                ui.selected += 1;
            }
        }
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            ui.numbers[ui.selected] = c.to_digit(10).unwrap_or(1);
        }
        KeyCode::Char('[') => {
            if ui.numbers[ui.selected] > 1 || (ui.allow_skip && ui.numbers[ui.selected] == 1) {
                ui.numbers[ui.selected] -= 1;
            }
        }
        KeyCode::Char(']') => {
            ui.numbers[ui.selected] = ui.numbers[ui.selected].saturating_add(1);
        }
        KeyCode::Char('g') => {
            ui.entering_number = true;
            ui.typing = TextEdit::new("");
            ui.status = "type group number, Enter to set".into();
        }
        KeyCode::Enter => match groups_from_numbers(&ui.commits, &ui.numbers, ui.allow_skip) {
            Ok(groups) => {
                ui.forms = groups
                    .iter()
                    .map(|g| Form {
                        title: TextEdit::new(g.title.as_str()),
                        intent: g.intent,
                        message: TextEdit::multiline(
                            g.message.as_deref().unwrap_or(g.title.as_str()),
                        ),
                    })
                    .collect();
                ui.groups = groups;
                ui.field = Field::Title;
                ui.phase = Phase::Details { index: 0 };
                ui.status = details_status(ui.allow_skip).into();
            }
            Err(err) => ui.status = err.to_string(),
        },
        _ => {}
    }
    Ok(())
}

fn details_status(allow_skip: bool) -> &'static str {
    if allow_skip {
        "Tab fields, arrows move, i intent, Ctrl+n/p groups, Ctrl+s adopt, Ctrl+w write groups and exit, Esc back, q abort"
    } else {
        "Tab fields, arrows move, i intent, Ctrl+n/p groups, Ctrl+s finish, Esc back, q abort"
    }
}

fn handle_details(
    ui: &mut AdoptUi,
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
    key: KeyEvent,
) -> Result<Option<TuiOutcome>> {
    let Phase::Details { index } = ui.phase else {
        return Ok(None);
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('s') => {
                return Ok(finish(ui, repo, queue, analysis)?.map(TuiOutcome::Adopt));
            }
            KeyCode::Char('w') if ui.allow_skip => {
                return Ok(finish(ui, repo, queue, analysis)?.map(TuiOutcome::WriteAndExit));
            }
            KeyCode::Char('n') => {
                if index + 1 < ui.forms.len() {
                    ui.phase = Phase::Details { index: index + 1 };
                    ui.field = Field::Title;
                }
                return Ok(None);
            }
            KeyCode::Char('p') => {
                if index > 0 {
                    ui.phase = Phase::Details { index: index - 1 };
                    ui.field = Field::Title;
                }
                return Ok(None);
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::Char('q') if !matches!(ui.field, Field::Title | Field::Message) => {
            return Err(Error::msg(
                "adopt grouping aborted; re-run git uplink init to continue",
            ));
        }
        KeyCode::Esc => {
            ui.phase = Phase::Assign;
            ui.status = "1-9 set group, [ ] bump, g then digits, Enter to continue, q abort".into();
        }
        KeyCode::Tab => {
            ui.field = match ui.field {
                Field::Title => Field::Intent,
                Field::Intent => Field::Message,
                Field::Message => Field::Title,
            };
        }
        KeyCode::BackTab => {
            ui.field = match ui.field {
                Field::Title => Field::Message,
                Field::Intent => Field::Title,
                Field::Message => Field::Intent,
            };
        }
        KeyCode::Char('i') if matches!(ui.field, Field::Intent) => {
            let form = &mut ui.forms[index];
            form.intent = if form.intent.is_internal_only() {
                PatchIntent::Upstream
            } else {
                PatchIntent::InternalOnly
            };
        }
        KeyCode::Char('q') if matches!(ui.field, Field::Intent) => {
            return Err(Error::msg(
                "adopt grouping aborted; re-run git uplink init to continue",
            ));
        }
        _ => apply_field_input(&mut ui.forms[index], &ui.field, key),
    }
    Ok(None)
}

fn apply_field_input(form: &mut Form, field: &Field, key: KeyEvent) {
    let target = match field {
        Field::Title => &mut form.title,
        Field::Message => &mut form.message,
        Field::Intent => return,
    };
    target.handle_key(key);
}

fn finish(
    ui: &mut AdoptUi,
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
) -> Result<Option<Vec<AdoptGroup>>> {
    for (i, form) in ui.forms.iter().enumerate() {
        ui.groups[i].title = form.title.text().trim().to_string();
        ui.groups[i].intent = form.intent;
        ui.groups[i].message = Some(form.message.text().to_string());
    }
    match resolve_groups(analysis, &ui.groups) {
        Ok(groups) => {
            for (i, group) in groups.iter().enumerate() {
                let from = analysis
                    .commits
                    .iter()
                    .find(|c| c.sha == group.commits[0])
                    .map(|c| c.first_parent.as_str())
                    .unwrap_or("");
                let head = group.commits.last().map(String::as_str).unwrap_or("");
                let message = group
                    .message
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(group.title.as_str());
                match assess_from_message(
                    repo,
                    queue,
                    from,
                    head,
                    message,
                    Some(&group.title),
                    group.intent,
                ) {
                    Ok(report) if !group.intent.is_internal_only() => {
                        if let Err(err) = assert_assess_ok(&report, &group.title) {
                            ui.phase = Phase::Details { index: i };
                            ui.status = err.to_string();
                            return Ok(None);
                        }
                    }
                    Ok(_) => {}
                    Err(err) => {
                        ui.phase = Phase::Details { index: i };
                        ui.status = err.to_string();
                        return Ok(None);
                    }
                }
            }
            Ok(Some(groups))
        }
        Err(err) => {
            ui.status = err.to_string();
            Ok(None)
        }
    }
}

fn draw(frame: &mut Frame, ui: &mut AdoptUi) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(area);
    match ui.phase {
        Phase::Assign => draw_assign(frame, ui, chunks[0], chunks[1]),
        Phase::Details { index } => draw_details(frame, ui, index, chunks[0], chunks[1]),
    }
    let block = Block::default().borders(Borders::ALL).title("Status");
    let inner = block.inner(chunks[2]);
    let status = if ui.entering_number {
        let label = "group number: ";
        let room = (inner.width as usize).saturating_sub(label.len());
        let view = ui.typing.view(room, inner.height.min(1) as usize);
        if let Some(line) = view.lines.first() {
            frame.set_cursor_position((inner.x + label.len() as u16 + view.cursor.0, inner.y));
            format!("{label}{line}")
        } else {
            label.to_string()
        }
    } else {
        ui.status.clone()
    };
    frame.render_widget(Paragraph::new(status).block(block), chunks[2]);
}

fn draw_assign(frame: &mut Frame, ui: &mut AdoptUi, list_area: Rect, help_area: Rect) {
    let items: Vec<ListItem> = ui
        .commits
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            let marker = if commit.is_merge {
                format!(" M{}", commit.parent_count)
            } else {
                String::new()
            };
            let on_main = if commit.on_main { "  (on main)" } else { "" };
            let number = match ui.numbers[i] {
                0 => "skip".to_string(),
                n => n.to_string(),
            };
            let item = ListItem::new(format!(
                "{number:>4}  {}{marker}  {}{on_main}",
                commit.short, commit.subject
            ));
            if ui.numbers[i] == 0 {
                item.style(Style::default().add_modifier(Modifier::DIM))
            } else {
                item
            }
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(ui.selected));
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Assign group numbers (first-parent)"),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, list_area, &mut state);
    frame.render_widget(
        Paragraph::new(if ui.allow_skip {
            "Each merge is one row. Related commits share a number; s skips a commit.\n\
A group cannot span a skipped commit. Interleaved numbers (1, 2, 1) are rejected."
        } else {
            "Each merge is one row. Related rebase commits share a number.\n\
Interleaved numbers (1, 2, 1) are rejected."
        })
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("Help")),
        help_area,
    );
}

fn draw_details(
    frame: &mut Frame,
    ui: &mut AdoptUi,
    index: usize,
    form_area: Rect,
    commits_area: Rect,
) {
    let warn = index > 0
        && ui.forms[index - 1].intent.is_internal_only()
        && !ui.forms[index].intent.is_internal_only();
    let count = ui.forms.len();
    let form = &mut ui.forms[index];
    let group = &ui.groups[index];
    let mark = |focused: bool| if focused { ">" } else { " " };
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Group details");
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(if warn { 2 } else { 0 }),
        ])
        .split(block.inner(form_area));
    frame.render_widget(block, form_area);
    frame.render_widget(
        Paragraph::new(format!("Group {} of {count}", index + 1)),
        rows[0],
    );

    let title_focused = matches!(ui.field, Field::Title);
    let label = format!("{} Title: ", mark(title_focused));
    let room = (rows[1].width as usize).saturating_sub(label.len());
    let view = form.title.view(room, rows[1].height as usize);
    if let Some(line) = view.lines.first() {
        if title_focused {
            frame.set_cursor_position((rows[1].x + label.len() as u16 + view.cursor.0, rows[1].y));
        }
        frame.render_widget(Paragraph::new(format!("{label}{line}")), rows[1]);
    }
    frame.render_widget(
        Paragraph::new(format!(
            "{} Intent: {} (i to toggle)",
            mark(matches!(ui.field, Field::Intent)),
            form.intent
        )),
        rows[2],
    );
    let message_focused = matches!(ui.field, Field::Message);
    frame.render_widget(
        Paragraph::new(format!("{} Message:", mark(message_focused))),
        rows[3],
    );
    let view = form
        .message
        .view(rows[4].width as usize, rows[4].height as usize);
    if message_focused && !view.lines.is_empty() {
        frame.set_cursor_position((rows[4].x + view.cursor.0, rows[4].y + view.cursor.1));
    }
    frame.render_widget(Paragraph::new(view.lines.join("\n")), rows[4]);
    if warn {
        frame.render_widget(
            Paragraph::new(
                "Warning: previous group is internal-only; this upstream patch cannot depend on it.",
            )
            .wrap(Wrap { trim: true }),
            rows[5],
        );
    }
    let one_liners: Vec<String> = group
        .commits
        .iter()
        .filter_map(|sha| ui.commits.iter().find(|c| &c.sha == sha))
        .map(|c| {
            let marker = if c.is_merge { "M " } else { "" };
            format!("  {marker}{}  {}", c.short, c.subject)
        })
        .collect();
    frame.render_widget(
        Paragraph::new(one_liners.join("\n"))
            .block(Block::default().borders(Borders::ALL).title("Commits")),
        commits_area,
    );
}

/// Shows a failing adopt group, with what `preflight.sh` printed or why it
/// does not apply, and asks what to do with it. Esc aborts.
pub fn choose_on_failure(
    failure: &AdoptFailure,
    options: &[(Choice, AdoptDecision)],
) -> Result<AdoptDecision> {
    let mut terminal = ratatui::init();
    let result = failure_loop(&mut terminal, failure, options);
    ratatui::restore();
    result
}

/// The first of `len` output lines shown in `height` rows when `top` is
/// asked for: never past the point where the last line reaches the bottom.
fn clamp_top(top: usize, len: usize, height: usize) -> usize {
    top.min(len.saturating_sub(height))
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
    let mut height = 1;
    loop {
        terminal.draw(|frame| {
            height = draw_failure(frame, failure, &output, &choices, selected, &mut top);
        })?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let page = height.saturating_sub(1).max(1);
        let shown = clamp_top(top, output.len(), height);
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
}

/// Draws the failure window; returns the rows the output has.
fn draw_failure(
    frame: &mut Frame,
    failure: &AdoptFailure,
    output: &[&str],
    choices: &[Choice],
    selected: usize,
    top: &mut usize,
) -> usize {
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
    let height = block.inner(chunks[1]).height as usize;
    *top = clamp_top(*top, output.len(), height);
    let shown = output
        .iter()
        .skip(*top)
        .take(height)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(Paragraph::new(shown).block(block), chunks[1]);

    let mut buttons = Vec::new();
    for (i, choice) in choices.iter().enumerate() {
        if i > 0 {
            buttons.push(Span::raw("  "));
        }
        let base = if i == selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        buttons.push(Span::styled("[ ", base));
        let mut underlined = false;
        for c in choice.label.chars() {
            let style = if !underlined && c.eq_ignore_ascii_case(&choice.key) {
                underlined = true;
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            };
            buttons.push(Span::styled(c.to_string(), style));
        }
        buttons.push(Span::styled(" ]", base));
    }
    let mut hint = "←/→ choose · Enter confirm · ↑/↓ PgUp/PgDn scroll · Esc abort".to_string();
    if !choices.iter().any(|c| c.label == "Amend") {
        hint.push_str(" · Amend needs a checkout without uncommitted changes");
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(buttons),
            Line::styled(hint, Style::default().add_modifier(Modifier::DIM)),
        ])
        .block(Block::default().borders(Borders::ALL).title("What now?")),
        chunks[2],
    );
    height
}

#[cfg(test)]
mod tests {
    use super::clamp_top;

    #[test]
    fn output_scroll_stops_at_both_ends() {
        assert_eq!(clamp_top(usize::MAX, 50, 10), 40);
        assert_eq!(clamp_top(45, 50, 10), 40);
        assert_eq!(clamp_top(3, 50, 10), 3);
        assert_eq!(clamp_top(7, 5, 10), 0);
    }
}
