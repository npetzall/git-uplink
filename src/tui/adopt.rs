//! The adopt window: the commits as a tree of groups on the left, the
//! selected commit's change or group's form on the right. Enter (or a
//! double-click) on a commit offers what to do with it; Ctrl+s adopts and
//! Ctrl+w writes the groups file, each after a confirmation.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::{
    DefaultTerminal,
    crossterm::event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::tree::{Action, AdoptTree, GroupId, Row};
use super::{TuiOutcome, button_at, buttons, centered, clamp_top, contains};
use crate::adopt::{AdoptGroup, AheadAnalysis, resolve_groups};
use crate::assess::{assert_assess_ok, assess_from_message, cutoff_marker};
use crate::edit::{clip, width};
use crate::error::{Error, Result};
use crate::git::git_ok;
use crate::prompt::{Choice, Selection, next_selection};
use crate::types::QueueState;

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Columns the commit view moves per sideways step.
const SIDEWAYS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Upstream,
    Message,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    /// The change of the selected commit.
    Commit,
    /// A field of the selected group's form.
    Field(Field),
}

enum Popup {
    Actions {
        commit: usize,
        items: Vec<(Action, bool)>,
        selected: usize,
    },
    Confirm {
        kind: Confirm,
        text: String,
        groups: Vec<AdoptGroup>,
        selected: usize,
    },
}

/// What a confirmation is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirm {
    Adopt,
    Write,
    /// Leave without adopting or writing anything.
    Quit,
}

/// Where the last draw put what can be clicked or scrolled.
#[derive(Default)]
struct Spots {
    tree: Rect,
    detail: Rect,
    title: Rect,
    upstream: Rect,
    message: Rect,
    popup: Rect,
    items: Vec<Rect>,
    buttons_at: (u16, u16),
    buttons: Vec<(u16, u16)>,
}

enum Flow {
    Continue,
    Done(TuiOutcome),
    /// Quit was confirmed.
    Quit,
}

struct Ui<'a> {
    repo: &'a Path,
    queue: &'a QueueState,
    analysis: &'a AheadAnalysis,
    groups_file: Option<&'a Path>,
    tree: AdoptTree,
    rows: Vec<Row>,
    selected: usize,
    list: ListState,
    focus: Focus,
    popup: Option<Popup>,
    status: String,
    shows: HashMap<String, Vec<String>>,
    commit_top: usize,
    /// First column of the commit view, for lines wider than the pane.
    commit_left: usize,
    spots: Spots,
    last_click: Option<(Instant, usize)>,
}

pub(super) fn run(
    terminal: &mut DefaultTerminal,
    repo: &Path,
    queue: &QueueState,
    analysis: &AheadAnalysis,
    groups_file: Option<&Path>,
) -> Result<TuiOutcome> {
    let tree = AdoptTree::new(
        analysis.commits.clone(),
        analysis.allow_skip,
        &cutoff_marker(queue),
    );
    let mut ui = Ui {
        repo,
        queue,
        analysis,
        groups_file,
        rows: tree.rows(),
        tree,
        selected: 0,
        list: ListState::default().with_selected(Some(0)),
        focus: Focus::Tree,
        popup: None,
        status: String::new(),
        shows: HashMap::new(),
        commit_top: 0,
        commit_left: 0,
        spots: Spots::default(),
        last_click: None,
    };
    loop {
        terminal.draw(|frame| ui.draw(frame))?;
        let flow = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => ui.on_key(key)?,
            Event::Mouse(mouse) => ui.on_mouse(mouse)?,
            _ => Flow::Continue,
        };
        match flow {
            Flow::Done(outcome) => return Ok(outcome),
            Flow::Quit => return Err(ui.aborted()),
            Flow::Continue => {}
        }
    }
}

impl Ui<'_> {
    fn row(&self) -> Row {
        self.rows[self.selected]
    }

    fn aborted(&self) -> Error {
        Error::msg(if self.tree.allow_skip() {
            "adopt aborted; nothing was changed"
        } else {
            "adopt grouping aborted; re-run git uplink init to continue"
        })
    }

    fn select(&mut self, index: usize) {
        let index = index.min(self.rows.len().saturating_sub(1));
        if index != self.selected {
            self.commit_top = 0;
            self.commit_left = 0;
        }
        self.selected = index;
        self.list.select(Some(index));
    }

    fn select_where(&mut self, wanted: impl Fn(Row) -> bool) {
        if let Some(index) = self.rows.iter().position(|row| wanted(*row)) {
            self.select(index);
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows = self.tree.rows();
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<Flow> {
        if self.popup.is_some() {
            return Ok(self.on_popup_key(key));
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let in_text = matches!(self.focus, Focus::Field(Field::Title | Field::Message));
        match key.code {
            KeyCode::Char('c') if ctrl => return Err(self.aborted()),
            KeyCode::Char('s') if ctrl => {
                self.confirm(false);
                return Ok(Flow::Continue);
            }
            KeyCode::Char('w') if ctrl && !in_text && self.tree.allow_skip() => {
                self.confirm(true);
                return Ok(Flow::Continue);
            }
            _ => {}
        }
        match self.focus {
            Focus::Tree => self.on_tree_key(key),
            Focus::Commit => {
                self.on_commit_key(key);
                Ok(Flow::Continue)
            }
            Focus::Field(field) => {
                self.on_field_key(field, key);
                Ok(Flow::Continue)
            }
        }
    }

    fn on_tree_key(&mut self, key: KeyEvent) -> Result<Flow> {
        let page = (self.spots.tree.height as usize).max(1);
        match key.code {
            KeyCode::Up => self.select(self.selected.saturating_sub(1)),
            KeyCode::Down => self.select(self.selected + 1),
            KeyCode::PageUp => self.select(self.selected.saturating_sub(page)),
            KeyCode::PageDown => self.select(self.selected + page),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(usize::MAX),
            KeyCode::Right | KeyCode::Tab => self.focus_right(),
            KeyCode::Enter => match self.row() {
                Row::Group(_) => self.focus = Focus::Field(Field::Title),
                row => {
                    if let Some(commit) = row.commit() {
                        self.open_actions(commit);
                    }
                }
            },
            KeyCode::Char('q') | KeyCode::Esc => self.confirm_quit(),
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn focus_right(&mut self) {
        self.focus = match self.row() {
            Row::Group(_) => Focus::Field(Field::Title),
            _ => Focus::Commit,
        };
    }

    fn on_commit_key(&mut self, key: KeyEvent) {
        let page = (self.spots.detail.height as usize).saturating_sub(1).max(1);
        match key.code {
            KeyCode::Up => self.commit_top = self.commit_top.saturating_sub(1),
            KeyCode::Down => self.commit_top += 1,
            KeyCode::PageUp => self.commit_top = self.commit_top.saturating_sub(page),
            KeyCode::PageDown => self.commit_top += page,
            KeyCode::Home => self.commit_top = 0,
            KeyCode::End => self.commit_top = usize::MAX,
            // Sideways, until the view is back at the first column.
            KeyCode::Right => self.commit_left = self.commit_left.saturating_add(SIDEWAYS),
            KeyCode::Left if self.commit_left > 0 => {
                self.commit_left = self.commit_left.saturating_sub(SIDEWAYS);
            }
            KeyCode::Left | KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => {
                self.focus = Focus::Tree;
            }
            KeyCode::Enter => {
                if let Some(commit) = self.row().commit() {
                    self.open_actions(commit);
                }
            }
            _ => {}
        }
    }

    fn on_field_key(&mut self, field: Field, key: KeyEvent) {
        let Row::Group(id) = self.row() else {
            self.focus = Focus::Tree;
            return;
        };
        let next = |f| match f {
            Field::Title => Field::Upstream,
            Field::Upstream => Field::Message,
            Field::Message => Field::Title,
        };
        let prev = |f| match f {
            Field::Title => Field::Message,
            Field::Upstream => Field::Title,
            Field::Message => Field::Upstream,
        };
        match (field, key.code) {
            (_, KeyCode::Esc) => self.focus = Focus::Tree,
            (_, KeyCode::Tab) => self.focus = Focus::Field(next(field)),
            (_, KeyCode::BackTab) => self.focus = Focus::Field(prev(field)),
            (Field::Upstream, KeyCode::Char(' ') | KeyCode::Enter) => {
                let form = self.tree.form_mut(id);
                form.upstream = !form.upstream;
            }
            (Field::Upstream, KeyCode::Left) => self.focus = Focus::Tree,
            (Field::Upstream, KeyCode::Up) | (Field::Title, KeyCode::Up) => {
                self.focus = Focus::Field(Field::Title);
            }
            (Field::Upstream, KeyCode::Down) | (Field::Title, KeyCode::Down | KeyCode::Enter) => {
                self.focus = Focus::Field(next(field));
            }
            (Field::Title, _) => {
                self.tree.form_mut(id).title.handle_key(key);
            }
            (Field::Message, KeyCode::Up) if self.tree.form(id).message.on_first_line() => {
                self.focus = Focus::Field(Field::Upstream);
            }
            (Field::Message, _) => {
                self.tree.form_mut(id).message.handle_key(key);
            }
            _ => {}
        }
    }

    fn open_actions(&mut self, commit: usize) {
        let items = self.tree.actions(commit);
        let selected = items.iter().position(|(_, ok)| *ok).unwrap_or(0);
        self.popup = Some(Popup::Actions {
            commit,
            items,
            selected,
        });
    }

    fn take_action(&mut self, commit: usize, action: Action) {
        self.popup = None;
        match self.tree.apply(commit, action) {
            Ok(group) => {
                self.rebuild_rows();
                self.select_where(|row| row.commit() == Some(commit));
                let short = &self.tree.commits()[commit].short;
                self.status = match group {
                    Some(id) => format!("{short} is in group {}", self.tree.number(id)),
                    None => format!("{short} is skipped"),
                };
            }
            Err(why) => self.status = why.to_string(),
        }
        self.focus = Focus::Tree;
    }

    fn confirm_quit(&mut self) {
        let text = if self.tree.allow_skip() {
            "Quit without adopting? Nothing is changed."
        } else {
            "Quit grouping? Nothing is adopted; re-run git uplink init to continue."
        };
        self.popup = Some(Popup::Confirm {
            kind: Confirm::Quit,
            text: text.to_string(),
            groups: Vec::new(),
            selected: 1,
        });
    }

    /// Checks the groups as adopting them would and asks to go ahead.
    fn confirm(&mut self, write: bool) {
        let Some(groups) = self.validate() else {
            return;
        };
        let (n, grouped, skipped) = self.tree.counts();
        let text = if write {
            let path = self.groups_file.map_or_else(
                || "the groups file".to_string(),
                |p| p.display().to_string(),
            );
            format!("Write the {n} group(s) to\n{path}\nand exit without adopting?")
        } else {
            format!("Adopt {n} group(s): {grouped} commit(s), {skipped} skipped?")
        };
        self.popup = Some(Popup::Confirm {
            kind: if write {
                Confirm::Write
            } else {
                Confirm::Adopt
            },
            text,
            groups,
            selected: 0,
        });
    }

    /// The groups, resolved and assessed; on a problem the group it is in
    /// is selected and the status says what it is.
    fn validate(&mut self) -> Option<Vec<AdoptGroup>> {
        let groups = self.tree.to_groups();
        let ids = self.tree.group_ids();
        if let Some(i) = groups.iter().position(|g| g.title.is_empty()) {
            self.show_problem(ids[i], format!("group {} has no title", i + 1));
            return None;
        }
        let resolved = match resolve_groups(self.analysis, &groups) {
            Ok(resolved) => resolved,
            Err(err) => {
                self.status = err.to_string();
                return None;
            }
        };
        for (i, group) in resolved.iter().enumerate() {
            let from = self
                .analysis
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
            let checked = assess_from_message(
                self.repo,
                self.queue,
                from,
                head,
                message,
                Some(&group.title),
                group.intent,
            )
            .and_then(|report| {
                if group.intent.is_internal_only() {
                    Ok(())
                } else {
                    assert_assess_ok(&report, &group.title)
                }
            });
            if let Err(err) = checked {
                self.show_problem(ids[i], err.to_string());
                return None;
            }
        }
        Some(resolved)
    }

    fn show_problem(&mut self, id: GroupId, status: String) {
        self.select_where(|row| row == Row::Group(id));
        self.focus = Focus::Field(Field::Title);
        self.status = status;
    }

    fn on_popup_key(&mut self, key: KeyEvent) -> Flow {
        let Some(popup) = self.popup.as_mut() else {
            return Flow::Continue;
        };
        match popup {
            Popup::Actions {
                commit,
                items,
                selected,
            } => {
                let enabled: Vec<usize> = (0..items.len()).filter(|i| items[*i].1).collect();
                match key.code {
                    KeyCode::Up => {
                        if let Some(i) = enabled.iter().rev().find(|i| **i < *selected) {
                            *selected = *i;
                        }
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        if let Some(i) = enabled.iter().find(|i| **i > *selected) {
                            *selected = *i;
                        }
                    }
                    KeyCode::Enter if items[*selected].1 => {
                        let (commit, action) = (*commit, items[*selected].0);
                        self.take_action(commit, action);
                    }
                    KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                    _ => {}
                }
                Flow::Continue
            }
            Popup::Confirm { kind, selected, .. } => {
                let choices = confirm_choices(*kind);
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match next_selection(&choices, *selected, key.code, ctrl) {
                    Selection::Move(i) => {
                        *selected = i;
                        Flow::Continue
                    }
                    Selection::Picked(0) => self.finish(),
                    Selection::Picked(_) | Selection::Cancel => {
                        self.popup = None;
                        Flow::Continue
                    }
                    Selection::Ignore => Flow::Continue,
                }
            }
        }
    }

    fn finish(&mut self) -> Flow {
        match self.popup.take() {
            Some(Popup::Confirm { kind, groups, .. }) => match kind {
                Confirm::Adopt => Flow::Done(TuiOutcome::Adopt(groups)),
                Confirm::Write => Flow::Done(TuiOutcome::WriteAndExit(groups)),
                Confirm::Quit => Flow::Quit,
            },
            other => {
                self.popup = other;
                Flow::Continue
            }
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> Result<Flow> {
        let (x, y) = (mouse.column, mouse.row);
        if self.popup.is_some() {
            return Ok(self.on_popup_mouse(mouse));
        }
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if contains(self.spots.tree, x, y) {
                    let at = if up {
                        self.selected.saturating_sub(1)
                    } else {
                        self.selected + 1
                    };
                    self.select(at);
                } else if contains(self.spots.detail, x, y)
                    && self.row().commit().is_some()
                    && mouse.modifiers.contains(KeyModifiers::SHIFT)
                {
                    // Shift+wheel goes sideways, where the terminal sends it.
                    self.commit_left = if up {
                        self.commit_left.saturating_sub(SIDEWAYS)
                    } else {
                        self.commit_left.saturating_add(SIDEWAYS)
                    };
                } else if contains(self.spots.detail, x, y) && self.row().commit().is_some() {
                    self.commit_top = if up {
                        self.commit_top.saturating_sub(3)
                    } else {
                        self.commit_top.saturating_add(3)
                    };
                }
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                if contains(self.spots.detail, x, y) && self.row().commit().is_some() =>
            {
                self.commit_left = if mouse.kind == MouseEventKind::ScrollLeft {
                    self.commit_left.saturating_sub(SIDEWAYS)
                } else {
                    self.commit_left.saturating_add(SIDEWAYS)
                };
            }
            MouseEventKind::Down(MouseButton::Left) => self.on_click(x, y),
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn on_click(&mut self, x: u16, y: u16) {
        let spots = &self.spots;
        if contains(spots.tree, x, y) {
            let index = self.list.offset() + (y - spots.tree.y) as usize;
            if index >= self.rows.len() {
                return;
            }
            let now = Instant::now();
            let double = self
                .last_click
                .is_some_and(|(at, row)| row == index && now.duration_since(at) < DOUBLE_CLICK);
            self.select(index);
            self.focus = Focus::Tree;
            self.last_click = Some((now, index));
            if double {
                self.last_click = None;
                match self.row() {
                    Row::Group(_) => self.focus = Focus::Field(Field::Title),
                    row => {
                        if let Some(commit) = row.commit() {
                            self.open_actions(commit);
                        }
                    }
                }
            }
            return;
        }
        let Row::Group(id) = self.row() else {
            if contains(spots.detail, x, y) {
                self.focus = Focus::Commit;
            }
            return;
        };
        let (title, upstream, message) = (spots.title, spots.upstream, spots.message);
        if contains(title, x, y) {
            self.focus = Focus::Field(Field::Title);
            self.tree
                .form_mut(id)
                .title
                .place_cursor((x - title.x) as usize, 0);
        } else if contains(upstream, x, y) {
            self.focus = Focus::Field(Field::Upstream);
            let form = self.tree.form_mut(id);
            form.upstream = !form.upstream;
        } else if contains(message, x, y) {
            self.focus = Focus::Field(Field::Message);
            self.tree
                .form_mut(id)
                .message
                .place_cursor((x - message.x) as usize, (y - message.y) as usize);
        }
    }

    fn on_popup_mouse(&mut self, mouse: MouseEvent) -> Flow {
        let MouseEventKind::Down(MouseButton::Left) = mouse.kind else {
            return Flow::Continue;
        };
        let (x, y) = (mouse.column, mouse.row);
        if !contains(self.spots.popup, x, y) {
            self.popup = None;
            return Flow::Continue;
        }
        match &self.popup {
            Some(Popup::Actions { commit, items, .. }) => {
                if let Some(i) = self.spots.items.iter().position(|r| contains(*r, x, y))
                    && items[i].1
                {
                    let (commit, action) = (*commit, items[i].0);
                    self.take_action(commit, action);
                }
                Flow::Continue
            }
            Some(Popup::Confirm { .. }) if y == self.spots.buttons_at.1 => {
                match button_at(&self.spots.buttons, self.spots.buttons_at.0, x) {
                    Some(0) => self.finish(),
                    Some(_) => {
                        self.popup = None;
                        Flow::Continue
                    }
                    None => Flow::Continue,
                }
            }
            _ => Flow::Continue,
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(2)])
            .split(frame.area());
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(outer[0]);
        self.spots = Spots::default();
        self.draw_tree(frame, panes[0]);
        match self.row() {
            Row::Group(id) => self.draw_group(frame, panes[1], id),
            Row::Member(i) | Row::Skipped(i) => self.draw_commit(frame, panes[1], i),
        }
        self.draw_status(frame, outer[1]);
        if self.popup.is_some() {
            self.draw_popup(frame, outer[0]);
        }
    }

    fn pane(&self, title: String, focused: bool) -> Block<'static> {
        let block = Block::default().borders(Borders::ALL).title(title);
        if focused {
            block.border_style(Style::default().fg(Color::Cyan))
        } else {
            block
        }
    }

    fn draw_tree(&mut self, frame: &mut Frame, area: Rect) {
        let commits = self.tree.commits();
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|row| match *row {
                Row::Group(id) => {
                    let form = self.tree.form(id);
                    let tag = if form.upstream { "[up]" } else { "[int]" };
                    ListItem::new(format!(
                        "▾ {} {}  {tag}",
                        self.tree.number(id),
                        form.title.text()
                    ))
                    .style(Style::default().add_modifier(Modifier::BOLD))
                }
                Row::Member(i) => ListItem::new(format!("    {}", commit_line(&commits[i]))),
                Row::Skipped(i) => ListItem::new(format!("  {}  skip", commit_line(&commits[i])))
                    .style(Style::default().add_modifier(Modifier::DIM)),
            })
            .collect();
        let (groups, _, skipped) = self.tree.counts();
        let block = self.pane(
            format!("Commits: {groups} group(s), {skipped} skipped"),
            self.focus == Focus::Tree,
        );
        self.spots.tree = block.inner(area);
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, area, &mut self.list);
    }

    fn draw_commit(&mut self, frame: &mut Frame, area: Rect, commit: usize) {
        let c = &self.tree.commits()[commit];
        let mut title = format!("{} {}", c.short, c.subject);
        let sha = c.sha.clone();
        let repo = self.repo;
        let lines = self.shows.entry(sha.clone()).or_insert_with(|| {
            git_ok(
                repo,
                &[
                    "show",
                    "--first-parent",
                    "--stat",
                    "--patch",
                    "--format=fuller",
                    "--no-color",
                    &sha,
                ],
            )
            .unwrap_or_else(|err| err.to_string())
            .replace('\t', "    ")
            .lines()
            .map(str::to_string)
            .collect()
        });
        let widest = lines.iter().map(|l| width(l)).max().unwrap_or(0);
        let pane_width = area.width.saturating_sub(2) as usize;
        self.commit_left = self.commit_left.min(widest.saturating_sub(pane_width));
        if self.commit_left > 0 {
            title.push_str(&format!("  ← from column {}", self.commit_left + 1));
        }
        let left = self.commit_left;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(if self.focus == Focus::Commit {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            });
        let inner = block.inner(area);
        self.spots.detail = inner;
        let height = inner.height as usize;
        self.commit_top = clamp_top(self.commit_top, lines.len(), height);
        let shown: Vec<Line> = lines
            .iter()
            .skip(self.commit_top)
            .take(height)
            .map(|line| {
                let style = if line.starts_with("+++") || line.starts_with("---") {
                    Style::default().add_modifier(Modifier::BOLD)
                } else if line.starts_with('+') {
                    Style::default().fg(Color::Green)
                } else if line.starts_with('-') {
                    Style::default().fg(Color::Red)
                } else if line.starts_with("@@") {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };
                Line::styled(clip(line, left, pane_width), style)
            })
            .collect();
        frame.render_widget(Paragraph::new(shown).block(block), area);
    }

    fn draw_group(&mut self, frame: &mut Frame, area: Rect, id: GroupId) {
        let focus = self.focus;
        let warn = self.tree.after_internal_only(id);
        let members = self.tree.members(id);
        let shorts: Vec<String> = members
            .iter()
            .map(|i| self.tree.commits()[*i].short.clone())
            .collect();
        let block = self.pane(
            format!("Group {}", self.tree.number(id)),
            matches!(focus, Focus::Field(_)),
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.spots.detail = inner;
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(if warn { 2 } else { 0 }),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);
        let mark = |field| {
            if focus == Focus::Field(field) {
                "> "
            } else {
                "  "
            }
        };
        let form = self.tree.form_mut(id);

        let label = format!("{}Title: ", mark(Field::Title));
        let label_width = width(&label) as u16;
        let title_area = Rect {
            x: rows[0].x + label_width,
            width: rows[0].width.saturating_sub(label_width),
            ..rows[0]
        };
        let view = form.title.view(title_area.width as usize, 1);
        frame.render_widget(
            Paragraph::new(format!(
                "{label}{}",
                view.lines.first().map_or("", String::as_str)
            )),
            rows[0],
        );
        if focus == Focus::Field(Field::Title) {
            frame.set_cursor_position((title_area.x + view.cursor.0, title_area.y));
        }

        let check = if form.upstream { "x" } else { " " };
        let upstream_text = format!(
            "{}[{check}] Upstream (unticked: internal-only)",
            mark(Field::Upstream)
        );
        frame.render_widget(Paragraph::new(upstream_text.clone()), rows[1]);
        let upstream_area = Rect {
            width: (width(&upstream_text) as u16).min(rows[1].width),
            ..rows[1]
        };

        frame.render_widget(
            Paragraph::new(format!("  Commits: {}", shorts.join(", ")))
                .style(Style::default().add_modifier(Modifier::DIM)),
            rows[2],
        );
        if warn {
            frame.render_widget(
                Paragraph::new(
                    "  Warning: the group above is internal-only; this upstream patch cannot depend on it.",
                )
                .style(Style::default().fg(Color::Yellow))
                .wrap(Wrap { trim: true }),
                rows[3],
            );
        }
        frame.render_widget(
            Paragraph::new(format!("{}Message:", mark(Field::Message))),
            rows[4],
        );
        let message_area = Rect {
            x: rows[5].x + 2,
            width: rows[5].width.saturating_sub(2),
            ..rows[5]
        };
        let view = form
            .message
            .view(message_area.width as usize, message_area.height as usize);
        if focus == Focus::Field(Field::Message) && !view.lines.is_empty() {
            frame.set_cursor_position((
                message_area.x + view.cursor.0,
                message_area.y + view.cursor.1,
            ));
        }
        frame.render_widget(Paragraph::new(view.lines.join("\n")), message_area);
        self.spots.title = title_area;
        self.spots.upstream = upstream_area;
        self.spots.message = message_area;
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let write = if self.tree.allow_skip() {
            " · Ctrl+w write & exit"
        } else {
            ""
        };
        let hint = match self.focus {
            Focus::Tree => format!(
                "↑/↓ select · Enter or double-click: actions/edit · → preview · Ctrl+s adopt{write} · q/Esc quit · Shift/Option+drag selects text"
            ),
            Focus::Commit => format!(
                "↑/↓ PgUp/PgDn or wheel scroll · →/← sideways (← at column 1: back) · Enter actions · Ctrl+s adopt{write}"
            ),
            Focus::Field(_) => {
                "Tab next field · Space toggles Upstream · Ctrl+a/Ctrl+e line start/end · Alt+←/→ word · Ctrl+w delete word · Esc back · Ctrl+s adopt"
                    .to_string()
            }
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(hint, Style::default().add_modifier(Modifier::DIM)),
                Line::styled(self.status.clone(), Style::default().fg(Color::Yellow)),
            ]),
            area,
        );
    }

    fn draw_popup(&mut self, frame: &mut Frame, area: Rect) {
        match &self.popup {
            Some(Popup::Actions {
                commit,
                items,
                selected,
            }) => {
                let c = &self.tree.commits()[*commit];
                let grouped = self.tree.owner(*commit).is_some();
                let labels: Vec<&str> = items
                    .iter()
                    .map(|(action, _)| action_label(*action, grouped))
                    .collect();
                let w = labels.iter().map(|l| width(l)).max().unwrap_or(10) as u16 + 6;
                let title = format!("{} {}", c.short, c.subject);
                let w = w.max(width(&title).min(50) as u16 + 4);
                let popup = centered(area, w, items.len() as u16 + 2);
                frame.render_widget(Clear, popup);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(Color::Cyan));
                let inner = block.inner(popup);
                frame.render_widget(block, popup);
                let mut spots = Vec::new();
                for (i, ((_, enabled), label)) in items.iter().zip(&labels).enumerate() {
                    let row = Rect {
                        y: inner.y + i as u16,
                        height: 1,
                        ..inner
                    };
                    let mut style = Style::default();
                    if !enabled {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    if i == *selected {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    frame.render_widget(Paragraph::new(format!(" {label}")).style(style), row);
                    spots.push(row);
                }
                self.spots.popup = popup;
                self.spots.items = spots;
            }
            Some(Popup::Confirm {
                kind,
                text,
                selected,
                ..
            }) => {
                let lines: Vec<&str> = text.lines().collect();
                let w = lines.iter().map(|l| width(l)).max().unwrap_or(20) as u16 + 4;
                let popup = centered(area, w.max(30), lines.len() as u16 + 4);
                frame.render_widget(Clear, popup);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(match kind {
                        Confirm::Adopt => "Adopt",
                        Confirm::Write => "Write groups",
                        Confirm::Quit => "Quit",
                    })
                    .border_style(Style::default().fg(Color::Cyan));
                let inner = block.inner(popup);
                frame.render_widget(block, popup);
                let (line, spots) = buttons(&confirm_choices(*kind), *selected);
                let mut shown: Vec<Line> = lines.iter().map(|l| Line::raw(l.to_string())).collect();
                shown.push(Line::raw(""));
                shown.push(line);
                frame.render_widget(Paragraph::new(shown), inner);
                self.spots.popup = popup;
                self.spots.buttons_at = (inner.x, inner.y + lines.len() as u16 + 1);
                self.spots.buttons = spots;
            }
            None => {}
        }
    }
}

/// The buttons of a confirmation: go ahead, then Cancel. Esc cancels.
fn confirm_choices(kind: Confirm) -> [Choice; 2] {
    [
        match kind {
            Confirm::Adopt => Choice {
                label: "Adopt",
                key: 'a',
            },
            Confirm::Write => Choice {
                label: "Write",
                key: 'w',
            },
            Confirm::Quit => Choice {
                label: "Quit",
                key: 'q',
            },
        },
        Choice {
            label: "Cancel",
            key: 'c',
        },
    ]
}

fn action_label(action: Action, grouped: bool) -> &'static str {
    match (action, grouped) {
        (Action::NewGroup, _) => "New group",
        (Action::ToPrevious, true) => "Move to previous group",
        (Action::ToPrevious, false) => "Add to previous group",
        (Action::ToNext, true) => "Move to next group",
        (Action::ToNext, false) => "Add to next group",
        (Action::Skip, _) => "Skip",
    }
}

fn commit_line(commit: &crate::adopt::AdoptCommit) -> String {
    let merge = if commit.is_merge {
        format!(" M{}", commit.parent_count)
    } else {
        String::new()
    };
    let on_main = if commit.on_main { "  (on main)" } else { "" };
    format!("{}{merge}  {}{on_main}", commit.short, commit.subject)
}
