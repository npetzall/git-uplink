use std::path::Path;

use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::adopt::{AdoptCommit, AdoptGroup, AheadAnalysis, groups_from_numbers, resolve_groups};
use crate::error::{Error, Result};
use crate::prepare::{assert_prepare_ok, prepare_from_message};
use crate::types::QueueState;

enum Phase {
    Assign,
    Details { index: usize },
}

struct Form {
    title: String,
    intent: String,
    message: String,
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
    typing: String,
    entering_number: bool,
    status: String,
    phase: Phase,
    forms: Vec<Form>,
    groups: Vec<AdoptGroup>,
    field: Field,
}

pub fn run_adopt(
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
) -> Result<Vec<AdoptGroup>> {
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
) -> Result<Vec<AdoptGroup>> {
    let n = analysis.commits.len();
    let mut ui = AdoptUi {
        commits: analysis.commits.clone(),
        numbers: (1..=n as u32).collect(),
        selected: 0,
        typing: String::new(),
        entering_number: false,
        status: "1-9 set group, [ ] bump, g then digits, Enter to continue, q abort".into(),
        phase: Phase::Assign,
        forms: Vec::new(),
        groups: Vec::new(),
        field: Field::Title,
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
                if let Some(groups) = handle_details(&mut ui, repo, queue, analysis, key)? {
                    return Ok(groups);
                }
            }
        }
    }
}

fn handle_assign(ui: &mut AdoptUi, key: KeyEvent) -> Result<()> {
    if ui.entering_number {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() => ui.typing.push(c),
            KeyCode::Backspace => {
                ui.typing.pop();
            }
            KeyCode::Enter => {
                if let Ok(n) = ui.typing.parse::<u32>() {
                    if n == 0 {
                        ui.status = "group numbers start at 1".into();
                    } else {
                        ui.numbers[ui.selected] = n;
                    }
                }
                ui.typing.clear();
                ui.entering_number = false;
            }
            KeyCode::Esc => {
                ui.typing.clear();
                ui.entering_number = false;
            }
            _ => {}
        }
        return Ok(());
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            return Err(Error::msg(
                "adopt grouping aborted; re-run git uplink init to continue",
            ));
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
            if ui.numbers[ui.selected] > 1 {
                ui.numbers[ui.selected] -= 1;
            }
        }
        KeyCode::Char(']') => {
            ui.numbers[ui.selected] = ui.numbers[ui.selected].saturating_add(1);
        }
        KeyCode::Char('g') => {
            ui.entering_number = true;
            ui.typing.clear();
            ui.status = "type group number, Enter to set".into();
        }
        KeyCode::Enter => match groups_from_numbers(&ui.commits, &ui.numbers) {
            Ok(groups) => {
                ui.forms = groups
                    .iter()
                    .map(|g| Form {
                        title: g.title.clone(),
                        intent: g.intent.clone(),
                        message: g.message.clone().unwrap_or_else(|| g.title.clone()),
                    })
                    .collect();
                ui.groups = groups;
                ui.field = Field::Title;
                ui.phase = Phase::Details { index: 0 };
                ui.status =
                    "Tab fields, i intent, Ctrl+n/p groups, Ctrl+s finish, Esc back, q abort"
                        .into();
            }
            Err(err) => ui.status = err.to_string(),
        },
        _ => {}
    }
    Ok(())
}

fn handle_details(
    ui: &mut AdoptUi,
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
    key: KeyEvent,
) -> Result<Option<Vec<AdoptGroup>>> {
    let Phase::Details { index } = ui.phase else {
        return Ok(None);
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('s') => return finish(ui, repo, queue, analysis),
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
            form.intent = if form.intent == "internal-only" {
                "upstream".into()
            } else {
                "internal-only".into()
            };
        }
        KeyCode::Char('q') if matches!(ui.field, Field::Intent) => {
            return Err(Error::msg(
                "adopt grouping aborted; re-run git uplink init to continue",
            ));
        }
        other => apply_field_input(&mut ui.forms[index], &ui.field, other),
    }
    Ok(None)
}

fn apply_field_input(form: &mut Form, field: &Field, code: KeyCode) {
    let target = match field {
        Field::Title => &mut form.title,
        Field::Message => &mut form.message,
        Field::Intent => return,
    };
    match code {
        KeyCode::Char(c) => target.push(c),
        KeyCode::Backspace => {
            target.pop();
        }
        KeyCode::Enter if matches!(field, Field::Message) => target.push('\n'),
        _ => {}
    }
}

fn finish(
    ui: &mut AdoptUi,
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
) -> Result<Option<Vec<AdoptGroup>>> {
    for (i, form) in ui.forms.iter().enumerate() {
        ui.groups[i].title = form.title.trim().to_string();
        ui.groups[i].intent = form.intent.clone();
        ui.groups[i].message = Some(form.message.clone());
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
                match prepare_from_message(
                    repo,
                    queue,
                    from,
                    head,
                    message,
                    Some(&group.title),
                    &group.intent,
                ) {
                    Ok(report) if group.intent == "upstream" => {
                        if let Err(err) = assert_prepare_ok(&report, &group.title) {
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
    let status = if ui.entering_number {
        format!("group number: {}_", ui.typing)
    } else {
        ui.status.clone()
    };
    frame.render_widget(
        Paragraph::new(status).block(Block::default().borders(Borders::ALL).title("Status")),
        chunks[2],
    );
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
            ListItem::new(format!(
                "{:>3}  {}{marker}  {}",
                ui.numbers[i], commit.short, commit.subject
            ))
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
        Paragraph::new(
            "Each merge is one row. Related rebase commits share a number.\n\
Interleaved numbers (1, 2, 1) are rejected.",
        )
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("Help")),
        help_area,
    );
}

fn draw_details(
    frame: &mut Frame,
    ui: &AdoptUi,
    index: usize,
    form_area: Rect,
    commits_area: Rect,
) {
    let form = &ui.forms[index];
    let group = &ui.groups[index];
    let warn = if index > 0
        && ui.forms[index.saturating_sub(1)].intent == "internal-only"
        && form.intent == "upstream"
    {
        "\nWarning: previous group is internal-only; this upstream patch cannot depend on it."
    } else {
        ""
    };
    let title_mark = if matches!(ui.field, Field::Title) {
        ">"
    } else {
        " "
    };
    let intent_mark = if matches!(ui.field, Field::Intent) {
        ">"
    } else {
        " "
    };
    let message_mark = if matches!(ui.field, Field::Message) {
        ">"
    } else {
        " "
    };
    let body = format!(
        "Group {} of {}\n\
{title_mark} Title: {}\n\
{intent_mark} Intent: {} (i to toggle)\n\
{message_mark} Message:\n{}\n{warn}",
        index + 1,
        ui.forms.len(),
        form.title,
        form.intent,
        form.message
    );
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Group details"),
        ),
        form_area,
    );
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
