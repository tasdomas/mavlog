mod tlog;

use std::{collections::HashMap, env, fs};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDateTime, NaiveTime};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEventKind};
use ratatui::{
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Row, Table, Wrap},
    DefaultTerminal, Frame,
};

fn main() -> Result<()> {
    let path = env::args()
        .nth(1)
        .context("usage: mavlog <file.tlog>")?;
    let data = fs::read(&path).with_context(|| format!("failed to read {path}"))?;
    let entries = tlog::parse(&data);
    if entries.is_empty() {
        bail!("no MAVLink messages found in {path}");
    }

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), event::EnableMouseCapture);
    let result = App::new(path, data, entries).run(&mut terminal);
    let _ = crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    ratatui::restore();
    result
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    List,
    Detail,
}

#[derive(PartialEq, Clone, Copy)]
enum TimeFormat {
    DateTime,
    OffsetSecs,
}

struct Settings {
    time_format: TimeFormat,
}

/// One row in the settings menu: a label plus accessors for cycling and
/// showing the current value.
struct SettingItem {
    label: &'static str,
    value: fn(&Settings) -> &'static str,
    toggle: fn(&mut Settings),
}

const SETTING_ITEMS: &[SettingItem] = &[SettingItem {
    label: "Time column",
    value: |s| match s.time_format {
        TimeFormat::DateTime => "date-time",
        TimeFormat::OffsetSecs => "offset (s)",
    },
    toggle: |s| {
        s.time_format = match s.time_format {
            TimeFormat::DateTime => TimeFormat::OffsetSecs,
            TimeFormat::OffsetSecs => TimeFormat::DateTime,
        }
    },
}];

struct App {
    path: String,
    data: Vec<u8>,
    entries: Vec<tlog::LogEntry>,
    /// Timestamp of the first message; offsets are relative to it.
    start_us: u64,
    selected: usize,
    offset: usize,
    view_height: usize,
    focus: Focus,
    detail_scroll: usize,
    settings: Settings,
    settings_open: bool,
    settings_selected: usize,
    prompt: Option<Prompt>,
    filters: Vec<FilterExpr>,
    /// Raw filter text, kept so reopening the prompt allows editing it.
    filter_text: String,
    /// Indices into `entries` that pass the current filter.
    filtered: Vec<usize>,
    filter_popup: Option<FilterPopup>,
    /// Distinct sys:comp pairs present in the file, sorted.
    id_options: Vec<(u8, u8)>,
    /// Distinct message-type names present in the file, sorted.
    type_options: Vec<String>,
    /// Marked messages by entry index; the value is an optional label.
    marks: HashMap<usize, String>,
}

#[derive(PartialEq, Clone, Copy)]
enum PromptKind {
    Jump,
    Filter,
    /// Label the marked message at this entry index.
    Label(usize),
}

/// Footer input line (jump-to-time or filter).
struct Prompt {
    kind: PromptKind,
    input: String,
    error: Option<String>,
}

/// One filter expression; every part that is present must match. A message
/// is shown if any expression matches it.
struct FilterExpr {
    sysid: Option<u8>,
    compid: Option<u8>,
    /// Lowercase message-type pattern, may contain '*' wildcards.
    name: Option<String>,
    /// Match the type name exactly instead of substring/glob.
    exact: bool,
}

impl FilterExpr {
    fn matches(&self, entry: &tlog::LogEntry) -> bool {
        self.sysid.is_none_or(|s| s == entry.sysid)
            && self.compid.is_none_or(|c| c == entry.compid)
            && self.name.as_deref().is_none_or(|p| {
                if self.exact {
                    entry.name.eq_ignore_ascii_case(p)
                } else {
                    name_matches(p, &entry.name)
                }
            })
    }

    /// Text form accepted by `parse_filters`.
    fn to_text(&self) -> String {
        let mut parts = Vec::new();
        if self.sysid.is_some() || self.compid.is_some() {
            let part = |v: Option<u8>| v.map_or("*".to_string(), |v| v.to_string());
            parts.push(format!("{}:{}", part(self.sysid), part(self.compid)));
        }
        if let Some(name) = &self.name {
            let name = name.to_ascii_uppercase();
            parts.push(if self.exact { format!("={name}") } else { name });
        }
        if parts.is_empty() {
            "*".to_string()
        } else {
            parts.join(" ")
        }
    }
}

/// Popup for viewing, creating, editing and deleting filters.
struct FilterPopup {
    /// Selected row: an index into the filter list, or one past the end for
    /// the "add filter" row.
    row: usize,
    editor: Option<FilterEditor>,
}

/// Dropdown-based editor for a single filter expression.
struct FilterEditor {
    /// Index of the filter being edited, or None when creating a new one.
    index: Option<usize>,
    /// 0 = any, otherwise 1 + index into App::id_options.
    id_choice: usize,
    /// 0 = any, otherwise 1 + index into App::type_options.
    type_choice: usize,
    /// 0 = id field, 1 = type field, 2 = save, 3 = cancel.
    row: usize,
    dropdown: Option<Dropdown>,
}

/// An open dropdown with type-to-filter autocomplete.
struct Dropdown {
    /// Typed query narrowing the options.
    query: String,
    /// Highlighted position within the filtered option list.
    highlight: usize,
}

impl App {
    fn new(path: String, data: Vec<u8>, entries: Vec<tlog::LogEntry>) -> Self {
        let mut id_options: Vec<(u8, u8)> =
            entries.iter().map(|e| (e.sysid, e.compid)).collect();
        id_options.sort_unstable();
        id_options.dedup();
        let mut type_options: Vec<String> =
            entries.iter().map(|e| e.name.clone()).collect();
        type_options.sort_unstable();
        type_options.dedup();

        Self {
            path,
            data,
            start_us: entries[0].timestamp_us,
            filtered: (0..entries.len()).collect(),
            entries,
            filter_popup: None,
            id_options,
            type_options,
            marks: HashMap::new(),
            selected: 0,
            offset: 0,
            view_height: 1,
            focus: Focus::List,
            detail_scroll: 0,
            settings: Settings {
                time_format: TimeFormat::DateTime,
            },
            settings_open: false,
            settings_selected: 0,
            prompt: None,
            filters: Vec::new(),
            filter_text: String::new(),
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if !self.handle_key(key.code) {
                        return Ok(());
                    }
                }
                Event::Mouse(mouse) => {
                    if self.settings_open
                        || self.prompt.is_some()
                        || self.filter_popup.is_some()
                    {
                        continue;
                    }
                    let delta = match mouse.kind {
                        MouseEventKind::ScrollUp => -3,
                        MouseEventKind::ScrollDown => 3,
                        _ => continue,
                    };
                    self.scroll_by(delta);
                }
                _ => {}
            }
        }
    }

    /// Returns false when the app should quit.
    fn handle_key(&mut self, code: KeyCode) -> bool {
        if self.settings_open {
            self.handle_settings_key(code);
            return true;
        }
        if self.filter_popup.is_some() {
            self.handle_filter_popup_key(code);
            return true;
        }
        if self.prompt.is_some() {
            self.handle_prompt_key(code);
            return true;
        }
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Char('s') => self.settings_open = true,
            KeyCode::Char('t') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Jump,
                    input: String::new(),
                    error: None,
                })
            }
            KeyCode::Char('f') => {
                self.filter_popup = Some(FilterPopup {
                    row: 0,
                    editor: None,
                })
            }
            KeyCode::Char('F') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Filter,
                    input: self.filter_text.clone(),
                    error: None,
                })
            }
            KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Esc => {
                if self.focus == Focus::Detail {
                    self.focus = Focus::List;
                } else {
                    return false;
                }
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter => {
                self.focus = match self.focus {
                    Focus::List => Focus::Detail,
                    Focus::Detail => Focus::List,
                };
            }
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::List,
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Detail,
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-(self.view_height as isize)),
            KeyCode::PageDown => self.scroll_by(self.view_height as isize),
            KeyCode::Home | KeyCode::Char('g') => match self.focus {
                Focus::List => self.select(0),
                Focus::Detail => self.detail_scroll = 0,
            },
            KeyCode::End | KeyCode::Char('G') => match self.focus {
                Focus::List => self.select(self.filtered.len().saturating_sub(1)),
                Focus::Detail => self.detail_scroll = usize::MAX, // clamped in draw
            },
            _ => {}
        }
        true
    }

    fn handle_prompt_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => {
                let prompt = self.prompt.as_ref().unwrap();
                let (kind, input) = (prompt.kind, prompt.input.clone());
                let result = match kind {
                    PromptKind::Jump => {
                        parse_jump(&input, self.settings.time_format, self.start_us)
                            .map(|target_us| self.jump_to_time(target_us))
                    }
                    PromptKind::Filter => parse_filters(&input).map(|filters| {
                        self.filters = filters;
                        self.filter_text = input.trim().to_string();
                        self.apply_filter();
                    }),
                    PromptKind::Label(entry_index) => {
                        self.marks.insert(entry_index, input.trim().to_string());
                        Ok(())
                    }
                };
                match result {
                    Ok(()) => self.prompt = None,
                    Err(err) => self.prompt.as_mut().unwrap().error = Some(err),
                }
            }
            KeyCode::Backspace => {
                if let Some(prompt) = &mut self.prompt {
                    prompt.input.pop();
                    prompt.error = None;
                }
            }
            KeyCode::Char(c) => {
                if let Some(prompt) = &mut self.prompt {
                    prompt.input.push(c);
                    prompt.error = None;
                }
            }
            _ => {}
        }
    }

    /// Toggle the mark on the selected message. Marking also opens the
    /// optional label prompt; unmarking discards the label.
    fn toggle_mark(&mut self) {
        let Some(&entry_index) = self.filtered.get(self.selected) else {
            return;
        };
        if self.marks.remove(&entry_index).is_none() {
            self.marks.insert(entry_index, String::new());
            self.prompt = Some(Prompt {
                kind: PromptKind::Label(entry_index),
                input: String::new(),
                error: None,
            });
        }
    }

    /// Select the first visible message at or after the target time.
    fn jump_to_time(&mut self, target_us: u64) {
        if self.filtered.is_empty() {
            return;
        }
        let index = self
            .filtered
            .partition_point(|&i| self.entries[i].timestamp_us < target_us)
            .min(self.filtered.len() - 1);
        self.select(index);
    }

    /// Rebuild the visible index list, keeping the selection as close as
    /// possible to the previously selected message.
    fn apply_filter(&mut self) {
        let current = self.filtered.get(self.selected).copied().unwrap_or(0);
        self.filtered = (0..self.entries.len())
            .filter(|&i| {
                self.filters.is_empty()
                    || self.filters.iter().any(|f| f.matches(&self.entries[i]))
            })
            .collect();
        self.selected = self
            .filtered
            .partition_point(|&i| i < current)
            .min(self.filtered.len().saturating_sub(1));
        self.offset = self.offset.min(self.selected);
        self.detail_scroll = 0;
    }

    fn handle_filter_popup_key(&mut self, code: KeyCode) {
        let popup = self.filter_popup.as_mut().unwrap();
        if popup.editor.is_some() {
            self.handle_filter_editor_key(code);
            return;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('f') => {
                self.filter_popup = None
            }
            KeyCode::Up | KeyCode::Char('k') => popup.row = popup.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                popup.row = (popup.row + 1).min(self.filters.len())
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                if popup.row < self.filters.len() {
                    self.filters.remove(popup.row);
                    let popup = self.filter_popup.as_mut().unwrap();
                    popup.row = popup.row.min(self.filters.len());
                    self.sync_filter_text();
                }
            }
            KeyCode::Enter | KeyCode::Char('n') | KeyCode::Char('a') => {
                let editing = popup.row < self.filters.len() && code == KeyCode::Enter;
                let (index, expr) = if editing {
                    (Some(popup.row), Some(&self.filters[popup.row]))
                } else {
                    (None, None)
                };
                // Prefill the dropdowns from the expression where possible.
                let id_choice = expr
                    .and_then(|e| e.sysid.zip(e.compid))
                    .and_then(|pair| self.id_options.iter().position(|&p| p == pair))
                    .map_or(0, |i| i + 1);
                let type_choice = expr
                    .and_then(|e| e.name.as_deref())
                    .and_then(|n| {
                        self.type_options
                            .iter()
                            .position(|t| t.eq_ignore_ascii_case(n))
                    })
                    .map_or(0, |i| i + 1);
                self.filter_popup.as_mut().unwrap().editor = Some(FilterEditor {
                    index,
                    id_choice,
                    type_choice,
                    row: 0,
                    dropdown: None,
                });
            }
            _ => {}
        }
    }

    fn handle_filter_editor_key(&mut self, code: KeyCode) {
        // A dropdown is open: characters filter the options, arrows navigate.
        let editor_ref = self.filter_popup.as_ref().unwrap().editor.as_ref().unwrap();
        let field_row = editor_ref.row;
        if let Some(dropdown) = &editor_ref.dropdown {
            let options = self.dropdown_options(field_row, &dropdown.query);
            let editor = self.filter_popup.as_mut().unwrap().editor.as_mut().unwrap();
            let dropdown = editor.dropdown.as_mut().unwrap();
            match code {
                KeyCode::Esc => editor.dropdown = None,
                KeyCode::Char(c) => {
                    dropdown.query.push(c);
                    dropdown.highlight = 0;
                }
                KeyCode::Backspace => {
                    dropdown.query.pop();
                    dropdown.highlight = 0;
                }
                KeyCode::Up => dropdown.highlight = dropdown.highlight.saturating_sub(1),
                KeyCode::Down => {
                    dropdown.highlight =
                        (dropdown.highlight + 1).min(options.len().saturating_sub(1))
                }
                KeyCode::Home => dropdown.highlight = 0,
                KeyCode::End => dropdown.highlight = options.len().saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(&choice) = options.get(dropdown.highlight) {
                        match field_row {
                            0 => editor.id_choice = choice,
                            _ => editor.type_choice = choice,
                        }
                        editor.dropdown = None;
                    }
                }
                _ => {}
            }
            return;
        }

        let editor = self.filter_popup.as_mut().unwrap().editor.as_mut().unwrap();
        match code {
            KeyCode::Esc => self.filter_popup.as_mut().unwrap().editor = None,
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                editor.row = editor.row.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                editor.row = (editor.row + 1).min(3)
            }
            KeyCode::Left | KeyCode::Char('h') if editor.row == 3 => editor.row = 2,
            KeyCode::Right | KeyCode::Char('l') if editor.row == 2 => editor.row = 3,
            KeyCode::Enter => match editor.row {
                // Open the dropdown highlighting the current value.
                0 | 1 => {
                    editor.dropdown = Some(Dropdown {
                        query: String::new(),
                        highlight: if editor.row == 0 {
                            editor.id_choice
                        } else {
                            editor.type_choice
                        },
                    })
                }
                2 => self.save_filter_editor(),
                _ => self.filter_popup.as_mut().unwrap().editor = None,
            },
            _ => {}
        }
    }

    fn save_filter_editor(&mut self) {
        let popup = self.filter_popup.as_mut().unwrap();
        let editor = popup.editor.take().unwrap();
        let (sysid, compid) = match editor.id_choice.checked_sub(1) {
            Some(i) => {
                let (s, c) = self.id_options[i];
                (Some(s), Some(c))
            }
            None => (None, None),
        };
        let name = editor
            .type_choice
            .checked_sub(1)
            .map(|i| self.type_options[i].to_ascii_lowercase());
        let expr = FilterExpr {
            sysid,
            compid,
            name,
            exact: true,
        };
        match editor.index {
            Some(i) => self.filters[i] = expr,
            None => {
                self.filters.push(expr);
                popup.row = self.filters.len() - 1;
            }
        }
        self.sync_filter_text();
    }

    /// Regenerate the editable filter text and reapply the filter.
    fn sync_filter_text(&mut self) {
        self.filter_text = self
            .filters
            .iter()
            .map(FilterExpr::to_text)
            .collect::<Vec<_>>()
            .join(", ");
        self.apply_filter();
    }

    fn handle_settings_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Char('s') | KeyCode::Esc => {
                self.settings_open = false
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_selected = self.settings_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings_selected =
                    (self.settings_selected + 1).min(SETTING_ITEMS.len() - 1)
            }
            KeyCode::Enter
            | KeyCode::Char(' ')
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                (SETTING_ITEMS[self.settings_selected].toggle)(&mut self.settings)
            }
            _ => {}
        }
    }

    /// Scroll whichever pane has focus.
    fn scroll_by(&mut self, delta: isize) {
        match self.focus {
            Focus::List => {
                if self.filtered.is_empty() {
                    return;
                }
                self.select(
                    self.selected
                        .saturating_add_signed(delta)
                        .min(self.filtered.len() - 1),
                );
            }
            Focus::Detail => {
                self.detail_scroll = self.detail_scroll.saturating_add_signed(delta);
            }
        }
    }

    fn select(&mut self, index: usize) {
        if index != self.selected {
            self.selected = index;
            self.detail_scroll = 0;
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        // The filter pane only exists while filters are active.
        let filter_height = if self.filters.is_empty() {
            0
        } else {
            self.filters.len() as u16 + 2
        };
        let [header_area, filter_area, main_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(filter_height),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [list_area, detail_area] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(main_area);

        let bar_style = Style::new().fg(Color::Black).bg(Color::Cyan);
        let count = if self.filters.is_empty() {
            format!("{} messages", self.entries.len())
        } else {
            format!("{} of {} messages", self.filtered.len(), self.entries.len())
        };
        frame.render_widget(
            Line::styled(
                format!(" {}  —  {count}", self.path),
                bar_style.add_modifier(Modifier::BOLD),
            ),
            header_area,
        );

        if !self.filters.is_empty() {
            self.draw_filter_pane(frame, filter_area);
        }
        self.draw_list(frame, list_area);
        self.draw_detail(frame, detail_area);

        if let Some(prompt) = &self.prompt {
            let label = match prompt.kind {
                PromptKind::Jump => match self.settings.time_format {
                    TimeFormat::DateTime => " Jump to time (HH:MM:SS[.mmm] or YYYY-MM-DD HH:MM:SS): ",
                    TimeFormat::OffsetSecs => " Jump to offset (seconds): ",
                },
                PromptKind::Filter => " Filter (e.g. 1:1 HEARTBEAT, GPS*, 255 — empty clears): ",
                PromptKind::Label(_) => " Label for marked message (Enter to save, Esc to skip): ",
            };
            let mut spans = vec![
                Span::styled(label, bar_style.add_modifier(Modifier::BOLD)),
                Span::styled(prompt.input.clone(), bar_style),
                Span::styled("█", bar_style),
            ];
            if let Some(err) = &prompt.error {
                spans.push(Span::styled(
                    format!("  ✗ {err}"),
                    Style::new().fg(Color::Red).bg(Color::Cyan).add_modifier(Modifier::BOLD),
                ));
            }
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            spans.push(Span::styled(
                " ".repeat((footer_area.width as usize).saturating_sub(used)),
                bar_style,
            ));
            frame.render_widget(Line::from(spans), footer_area);
        } else {
            let position = if self.filtered.is_empty() {
                "0/0 ".to_string()
            } else {
                format!("{}/{} ", self.selected + 1, self.filtered.len())
            };
            let hints = if self.settings_open {
                " ↑/↓ select  Enter/Space toggle  Esc close"
            } else if let Some(popup) = &self.filter_popup {
                match &popup.editor {
                    Some(editor) if editor.dropdown.is_some() => {
                        " type to filter  ↑/↓ choose  Enter select  Esc cancel"
                    }
                    Some(_) => " ↑/↓ fields  Enter open/confirm  Esc back",
                    None => " ↑/↓ select  Enter edit  n new  d delete  Esc close",
                }
            } else {
                match self.focus {
                    Focus::List => " ↑/↓ select  →/Tab details  Space mark  t jump  f filter  s settings  q quit",
                    Focus::Detail => " ↑/↓ scroll  ←/Esc list  Space mark  t jump  f filter  s settings  q quit",
                }
            };
            frame.render_widget(
                Line::styled(
                    format!(
                        "{hints}{position:>width$}",
                        width = (footer_area.width as usize)
                            .saturating_sub(hints.chars().count()),
                    ),
                    bar_style,
                ),
                footer_area,
            );
        }

        if self.settings_open {
            self.draw_settings(frame);
        }
        self.draw_filter_popup(frame);
    }

    fn draw_filter_pane(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let lines: Vec<Line> = self
            .filters
            .iter()
            .map(|f| Line::raw(format!(" {}", f.to_text())))
            .collect();
        let block = Block::bordered()
            .title(Line::styled(
                " Filters ",
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::raw(" f edit ").right_aligned())
            .border_style(Style::new().fg(Color::DarkGray));
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn draw_filter_popup(&self, frame: &mut Frame) {
        let Some(popup) = &self.filter_popup else {
            return;
        };
        if let Some(editor) = &popup.editor {
            self.draw_filter_editor(frame, editor);
            return;
        }

        let height = self.filters.len() as u16 + 3;
        let area = centered_fixed(frame.area(), 48, height);
        let lines: Vec<Line> = (0..=self.filters.len())
            .map(|i| {
                let text = if i < self.filters.len() {
                    format!(" {} ", self.filters[i].to_text())
                } else {
                    " + Add filter ".to_string()
                };
                if i == popup.row {
                    Line::styled(text, Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    Line::raw(text)
                }
            })
            .collect();

        let block = Block::bordered()
            .title(Line::styled(
                " Filters ",
                Style::new().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::raw(" Enter edit  n new  d delete  Esc ").right_aligned())
            .border_style(Style::new().fg(Color::Cyan));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn draw_filter_editor(&self, frame: &mut Frame, editor: &FilterEditor) {
        let area = centered_fixed(frame.area(), 48, 6);
        let field_style = |row: usize| {
            if editor.row == row && editor.dropdown.is_none() {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new()
            }
        };
        let field = |label: &str, value: String, row: usize| {
            Line::from(vec![
                Span::raw(format!(" {label:<18}")),
                Span::styled(format!("[ {value:<20} ▾ ]"), field_style(row)),
            ])
        };
        let lines = vec![
            field("System:Component", self.id_option_text(editor.id_choice), 0),
            field("Message type", self.type_option_text(editor.type_choice), 1),
            Line::raw(""),
            Line::from(vec![
                Span::raw(" ".repeat(12)),
                Span::styled("[ Save ]", field_style(2)),
                Span::raw("    "),
                Span::styled("[ Cancel ]", field_style(3)),
            ]),
        ];

        let title = if editor.index.is_some() {
            " Edit filter "
        } else {
            " New filter "
        };
        let block = Block::bordered()
            .title(Line::styled(title, Style::new().add_modifier(Modifier::BOLD)))
            .border_style(Style::new().fg(Color::Cyan));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);

        if let Some(dropdown) = &editor.dropdown {
            self.draw_dropdown(frame, editor, dropdown);
        }
    }

    /// Option indices (0 = "any") whose label contains the query,
    /// case-insensitive.
    fn dropdown_options(&self, field_row: usize, query: &str) -> Vec<usize> {
        let query = query.to_ascii_lowercase();
        let count = 1 + match field_row {
            0 => self.id_options.len(),
            _ => self.type_options.len(),
        };
        (0..count)
            .filter(|&i| {
                let label = match field_row {
                    0 => self.id_option_text(i),
                    _ => self.type_option_text(i),
                };
                label.to_ascii_lowercase().contains(&query)
            })
            .collect()
    }

    fn draw_dropdown(&self, frame: &mut Frame, editor: &FilterEditor, dropdown: &Dropdown) {
        let title = match editor.row {
            0 => " System:Component ",
            _ => " Message type ",
        };
        let options = self.dropdown_options(editor.row, &dropdown.query);
        let list_len = options.len().max(1); // keep room for "(no matches)"
        let view_height = list_len.min(12);
        let area = centered_fixed(frame.area(), 36, view_height as u16 + 3);

        let mut lines = vec![Line::from(vec![
            Span::styled(" > ", Style::new().fg(Color::Yellow)),
            Span::raw(dropdown.query.clone()),
            Span::styled("█", Style::new().fg(Color::Yellow)),
        ])];
        if options.is_empty() {
            lines.push(Line::styled(
                " (no matches) ",
                Style::new().fg(Color::DarkGray),
            ));
        } else {
            // Window the options around the highlighted one.
            let offset = dropdown
                .highlight
                .saturating_sub(view_height / 2)
                .min(options.len().saturating_sub(view_height));
            lines.extend((offset..options.len().min(offset + view_height)).map(|i| {
                let value = match editor.row {
                    0 => self.id_option_text(options[i]),
                    _ => self.type_option_text(options[i]),
                };
                let text = format!(" {value} ");
                if i == dropdown.highlight {
                    Line::styled(text, Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    Line::raw(text)
                }
            }));
        }

        let mut block = Block::bordered()
            .title(Line::styled(title, Style::new().add_modifier(Modifier::BOLD)))
            .border_style(Style::new().fg(Color::Yellow));
        if options.len() > view_height {
            block = block.title_bottom(
                Line::raw(format!(" {}/{} ", dropdown.highlight + 1, options.len()))
                    .right_aligned(),
            );
        }
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    /// Dropdown option label: 0 is "any", the rest map into id_options.
    fn id_option_text(&self, choice: usize) -> String {
        match choice.checked_sub(1) {
            None => "any".to_string(),
            Some(i) => format!("{}:{}", self.id_options[i].0, self.id_options[i].1),
        }
    }

    /// Dropdown option label: 0 is "any", the rest map into type_options.
    fn type_option_text(&self, choice: usize) -> String {
        match choice.checked_sub(1) {
            None => "any".to_string(),
            Some(i) => self.type_options[i].clone(),
        }
    }

    fn draw_settings(&self, frame: &mut Frame) {
        let width = 40;
        let height = SETTING_ITEMS.len() as u16 + 2;
        let [area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(frame.area());
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);

        let inner_width = (width as usize).saturating_sub(2);
        let lines: Vec<Line> = SETTING_ITEMS
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let value = format!("◂ {} ▸", (item.value)(&self.settings));
                let text = format!(
                    " {}{value:>vw$} ",
                    item.label,
                    vw = inner_width.saturating_sub(item.label.len() + 2),
                );
                if i == self.settings_selected {
                    Line::styled(text, Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    Line::raw(text)
                }
            })
            .collect();

        let block = Block::bordered()
            .title(Line::styled(
                " Settings ",
                Style::new().add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::new().fg(Color::Cyan));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn draw_list(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        // Two border rows plus the column header row.
        self.view_height = (area.height as usize).saturating_sub(3).max(1);

        // Keep the selection inside the visible window.
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + self.view_height {
            self.offset = self.selected - self.view_height + 1;
        }

        let end = (self.offset + self.view_height).min(self.filtered.len());
        let selection_style = if self.focus == Focus::List {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new().fg(Color::Black).bg(Color::DarkGray)
        };
        let time_width = match self.settings.time_format {
            TimeFormat::DateTime => 23,
            TimeFormat::OffsetSecs => 12,
        };
        let rows = self.filtered[self.offset.min(end)..end]
            .iter()
            .enumerate()
            .map(|(i, &entry_index)| {
                let position = self.offset + i;
                let entry = &self.entries[entry_index];
                let mark = self.marks.get(&entry_index);
                let label = match mark {
                    Some(label) if label.is_empty() => "●".to_string(),
                    Some(label) => format!("● {label}"),
                    None => String::new(),
                };
                let row = Row::new(vec![
                    format!("{entry_index:>7}"),
                    self.format_list_time(entry.timestamp_us),
                    format!("{:>3}:{:<3}", entry.sysid, entry.compid),
                    entry.name.clone(),
                    label,
                ]);
                if position == self.selected {
                    row.style(selection_style)
                } else if mark.is_some() {
                    row.style(Style::new().fg(Color::White).bg(Color::Red))
                } else {
                    row
                }
            });

        let table = Table::new(
            rows,
            [
                Constraint::Length(7),
                Constraint::Length(time_width),
                Constraint::Length(7),
                Constraint::Fill(1),
                Constraint::Length(14),
            ],
        )
        .header(
            Row::new(vec!["#", "TIME", "SYS:CMP", "MESSAGE", "LABEL"])
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        )
        .block(self.pane_block(Focus::List).title(" Messages "));
        frame.render_widget(table, area);
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let Some(&entry_index) = self.filtered.get(self.selected) else {
            frame.render_widget(
                Paragraph::new("\n No messages match the filter.")
                    .block(self.pane_block(Focus::Detail)),
                area,
            );
            return;
        };
        let entry = &self.entries[entry_index];
        let mut body = format!(
            "Time: {}  ({})\n",
            format_datetime(entry.timestamp_us),
            format_offset(entry.timestamp_us, self.start_us),
        );
        if let Some(label) = self.marks.get(&entry_index) {
            body.push_str("Mark: ●");
            if !label.is_empty() {
                body.push_str(&format!(" {label}"));
            }
            body.push('\n');
        }
        body.push('\n');
        body.push_str(&match tlog::decode(&self.data, entry) {
            Ok(msg) => format!("{msg:#?}"),
            Err(_) => hex_dump(&self.data[entry.payload.clone()]),
        });
        let lines: Vec<&str> = body.lines().collect();

        let inner_height = (area.height as usize).saturating_sub(2);
        self.detail_scroll = self
            .detail_scroll
            .min(lines.len().saturating_sub(inner_height));

        let title = format!(" {} (id {}) ", entry.name, entry.msg_id);
        let mut block = self
            .pane_block(Focus::Detail)
            .title(Line::styled(title, Style::new().add_modifier(Modifier::BOLD)));
        if lines.len() > inner_height {
            block = block.title_bottom(
                Line::raw(format!(
                    " {}-{}/{} ",
                    self.detail_scroll + 1,
                    (self.detail_scroll + inner_height).min(lines.len()),
                    lines.len(),
                ))
                .right_aligned(),
            );
        }

        let text: Vec<Line> = lines
            .iter()
            .skip(self.detail_scroll)
            .take(inner_height)
            .map(|line| Line::raw(*line))
            .collect();
        frame.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
            area,
        );
    }

    fn pane_block(&self, pane: Focus) -> Block<'static> {
        let color = if self.focus == pane && !self.settings_open {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        Block::bordered().border_style(Style::new().fg(color))
    }

    fn format_list_time(&self, timestamp_us: u64) -> String {
        match self.settings.time_format {
            TimeFormat::DateTime => format_datetime(timestamp_us),
            TimeFormat::OffsetSecs => format_offset(timestamp_us, self.start_us),
        }
    }
}

/// Seconds relative to the start of the log, e.g. "T+4.600s".
fn format_offset(timestamp_us: u64, start_us: u64) -> String {
    let secs = (timestamp_us as i64 - start_us as i64) as f64 / 1e6;
    format!("T{secs:+.3}s")
}

/// Parse jump input into an absolute timestamp in microseconds. The accepted
/// syntax follows the current time column format: seconds relative to the log
/// start, or a wall-clock time (date defaults to the log start's date, UTC).
fn parse_jump(input: &str, format: TimeFormat, start_us: u64) -> Result<u64, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty input".to_string());
    }
    match format {
        TimeFormat::OffsetSecs => {
            let s = s.strip_prefix('T').unwrap_or(s);
            let s = s.strip_suffix('s').unwrap_or(s);
            let secs: f64 = s
                .parse()
                .map_err(|_| "expected seconds, e.g. 42.5".to_string())?;
            let target = start_us as i64 + (secs * 1e6).round() as i64;
            Ok(target.max(0) as u64)
        }
        TimeFormat::DateTime => {
            if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
                return Ok(dt.and_utc().timestamp_micros().max(0) as u64);
            }
            let start = DateTime::from_timestamp_micros(start_us as i64)
                .ok_or_else(|| "log start time invalid".to_string())?;
            for fmt in ["%H:%M:%S%.f", "%H:%M"] {
                if let Ok(t) = NaiveTime::parse_from_str(s, fmt) {
                    let dt = start.date_naive().and_time(t);
                    return Ok(dt.and_utc().timestamp_micros().max(0) as u64);
                }
            }
            Err("expected HH:MM:SS[.mmm] or YYYY-MM-DD HH:MM:SS".to_string())
        }
    }
}

/// Parse a comma-separated list of filter expressions. Each expression is an
/// optional `sys[:comp]` id spec and/or a message-type pattern, e.g.
/// "1:1 HEARTBEAT, GPS*, 255". An empty input clears the filter.
fn parse_filters(input: &str) -> Result<Vec<FilterExpr>, String> {
    let mut exprs = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut expr = FilterExpr {
            sysid: None,
            compid: None,
            name: None,
            exact: false,
        };
        for token in part.split_whitespace() {
            // An id spec is digits/':'/'*' only; a bare "*" is a type pattern.
            let is_id_spec = token != "*"
                && token
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ':' || c == '*');
            if is_id_spec {
                if expr.sysid.is_some() || expr.compid.is_some() {
                    return Err(format!("more than one id spec in '{part}'"));
                }
                let (sys, comp) = match token.split_once(':') {
                    Some((sys, comp)) => (sys, comp),
                    None => (token, ""),
                };
                let parse_id = |s: &str| -> Result<Option<u8>, String> {
                    if s.is_empty() || s == "*" {
                        return Ok(None);
                    }
                    s.parse()
                        .map(Some)
                        .map_err(|_| format!("bad id '{s}' in '{part}'"))
                };
                expr.sysid = parse_id(sys)?;
                expr.compid = parse_id(comp)?;
            } else {
                if expr.name.is_some() {
                    return Err(format!("more than one message type in '{part}'"));
                }
                // '=' prefix requires an exact type match.
                match token.strip_prefix('=') {
                    Some(rest) => {
                        expr.name = Some(rest.to_ascii_lowercase());
                        expr.exact = true;
                    }
                    None => expr.name = Some(token.to_ascii_lowercase()),
                }
            }
        }
        exprs.push(expr);
    }
    Ok(exprs)
}

/// Case-insensitive message-type match: substring by default, glob when the
/// pattern contains '*'.
fn name_matches(pattern: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if !pattern.contains('*') {
        return name.contains(pattern);
    }
    let segments: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (segments[0], *segments.last().unwrap());
    if name.len() < first.len() + last.len()
        || !name.starts_with(first)
        || !name.ends_with(last)
    {
        return false;
    }
    // Middle segments must appear in order between the anchored ends.
    let mut rest = &name[first.len()..name.len() - last.len()];
    for seg in &segments[1..segments.len() - 1] {
        match rest.find(seg) {
            Some(i) => rest = &rest[i + seg.len()..],
            None => return false,
        }
    }
    true
}

/// A rect of fixed size centered in `area` (clamped to fit).
fn centered_fixed(area: ratatui::layout::Rect, width: u16, height: u16) -> ratatui::layout::Rect {
    let [rect] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(rect);
    rect
}

fn format_datetime(timestamp_us: u64) -> String {
    match DateTime::from_timestamp_micros(timestamp_us as i64) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("({timestamp_us} us)"),
    }
}

/// Fallback body for messages the dialect can't decode.
fn hex_dump(payload: &[u8]) -> String {
    let mut out = String::from("undecodable payload:\n");
    for (i, chunk) in payload.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        out.push_str(&format!("{:04x}  {:<47}  {ascii}\n", i * 16, hex.join(" ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2024-07-17 07:06:40 UTC in microseconds.
    const START_US: u64 = 1_721_200_000_000_000;

    #[test]
    fn parses_offset_input() {
        assert_eq!(
            parse_jump("42.5", TimeFormat::OffsetSecs, START_US).unwrap(),
            START_US + 42_500_000
        );
        assert_eq!(
            parse_jump("T+1s", TimeFormat::OffsetSecs, START_US).unwrap(),
            START_US + 1_000_000
        );
        assert_eq!(
            parse_jump("-5", TimeFormat::OffsetSecs, START_US).unwrap(),
            START_US - 5_000_000
        );
        assert!(parse_jump("abc", TimeFormat::OffsetSecs, START_US).is_err());
    }

    #[test]
    fn parses_datetime_input() {
        // Time-only inherits the log start's date.
        assert_eq!(
            parse_jump("07:06:41", TimeFormat::DateTime, START_US).unwrap(),
            START_US + 1_000_000
        );
        assert_eq!(
            parse_jump("07:06:41.250", TimeFormat::DateTime, START_US).unwrap(),
            START_US + 1_250_000
        );
        assert_eq!(
            parse_jump("2024-07-17 07:06:41", TimeFormat::DateTime, START_US).unwrap(),
            START_US + 1_000_000
        );
        assert!(parse_jump("07:06:41", TimeFormat::OffsetSecs, START_US).is_err());
        assert!(parse_jump("nonsense", TimeFormat::DateTime, START_US).is_err());
    }

    fn entry(sysid: u8, compid: u8, name: &str) -> tlog::LogEntry {
        tlog::LogEntry {
            timestamp_us: 0,
            sysid,
            compid,
            msg_id: 0,
            version: mavlink::MavlinkVersion::V2,
            payload: 0..0,
            name: name.to_string(),
        }
    }

    #[test]
    fn parses_filter_expressions() {
        let exprs = parse_filters("1:1 HEARTBEAT, GPS*, 255,  :50").unwrap();
        assert_eq!(exprs.len(), 4);
        assert_eq!((exprs[0].sysid, exprs[0].compid), (Some(1), Some(1)));
        assert_eq!(exprs[0].name.as_deref(), Some("heartbeat"));
        assert_eq!((exprs[1].sysid, exprs[1].compid), (None, None));
        assert_eq!((exprs[2].sysid, exprs[2].compid), (Some(255), None));
        assert!(exprs[2].name.is_none());
        assert_eq!((exprs[3].sysid, exprs[3].compid), (None, Some(50)));

        assert!(parse_filters("").unwrap().is_empty());
        assert!(parse_filters("999 FOO").is_err());
        assert!(parse_filters("1:1 2:2").is_err());
        assert!(parse_filters("FOO BAR").is_err());
    }

    #[test]
    fn exact_type_match() {
        let exprs = parse_filters("=ATTITUDE").unwrap();
        assert!(exprs[0].exact);
        assert!(exprs[0].matches(&entry(1, 1, "ATTITUDE")));
        assert!(!exprs[0].matches(&entry(1, 1, "ATTITUDE_TARGET")));

        // Substring filters (no '=') do match extensions of the name.
        let loose = parse_filters("ATTITUDE").unwrap();
        assert!(loose[0].matches(&entry(1, 1, "ATTITUDE_TARGET")));
    }

    #[test]
    fn filter_text_roundtrip() {
        for text in ["1:1 =HEARTBEAT", "GPS*", "255:*", "*:50 =VFR_HUD", "*"] {
            let exprs = parse_filters(text).unwrap();
            assert_eq!(exprs[0].to_text(), text, "roundtrip of '{text}'");
        }
    }

    #[test]
    fn filter_expressions_match() {
        let exprs = parse_filters("1:1 HEART, GPS*STATUS, 255").unwrap();
        let matches = |e: &tlog::LogEntry| exprs.iter().any(|f| f.matches(e));

        assert!(matches(&entry(1, 1, "HEARTBEAT")));
        assert!(!matches(&entry(2, 1, "HEARTBEAT")));
        assert!(matches(&entry(2, 1, "GPS_RAW_STATUS")));
        assert!(!matches(&entry(2, 1, "GPS_RAW_INT")));
        assert!(matches(&entry(255, 190, "PARAM_REQUEST_LIST")));
        assert!(!matches(&entry(1, 1, "ATTITUDE")));
    }
}
