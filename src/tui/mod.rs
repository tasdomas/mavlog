//! The terminal (ratatui) frontend. Owns a `Session` plus terminal-only
//! view state and all drawing/input handling.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEventKind};
use ratatui::{
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Row, Table, Wrap},
    DefaultTerminal, Frame,
};

use crate::core::column::{parse_columns, CustomColumn};
use crate::core::filter::{match_labels, parse_filters, FilterExpr};
use crate::core::session::Session;
use crate::core::time::{format_datetime, format_offset, parse_jump, TimeFormat};
use crate::tlog;

/// Run the terminal UI over a loaded session until the user quits.
pub fn run(session: Session) -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), event::EnableMouseCapture);
    let mut app = App::new(session);
    app.load_setup();
    let result = app.run(&mut terminal);
    let _ = crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    ratatui::restore();
    result
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    List,
    Detail,
}

/// One row in the settings menu: a label plus accessors for cycling and
/// showing the current value.
struct SettingItem {
    label: &'static str,
    value: fn(&Session) -> &'static str,
    toggle: fn(&mut Session),
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

/// The TUI frontend: a `Session` plus terminal-specific view state.
struct App {
    session: Session,
    offset: usize,
    view_height: usize,
    focus: Focus,
    detail_scroll: usize,
    settings_open: bool,
    settings_selected: usize,
    prompt: Option<Prompt>,
    filter_popup: Option<FilterPopup>,
    columns_popup: Option<ColumnsPopup>,
    /// Transient footer notice (e.g. save confirmation), cleared on keypress.
    status: Option<String>,
}

#[derive(PartialEq, Clone, Copy)]
enum PromptKind {
    Jump,
    Filter,
    /// Label the marked message at this entry index.
    Label(usize),
    Columns,
}

/// Footer input line (jump-to-time or filter).
struct Prompt {
    kind: PromptKind,
    input: String,
    error: Option<String>,
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

/// Popup for viewing, creating, editing and deleting custom columns.
struct ColumnsPopup {
    /// Selected row: an index into the column list, or one past the end for
    /// the "add column" row.
    row: usize,
    editor: Option<ColumnEditor>,
}

/// Editor for a single custom column.
struct ColumnEditor {
    /// Index of the column being edited, or None when creating a new one.
    index: Option<usize>,
    name: String,
    /// 0 = any, otherwise 1 + index into App::id_options.
    id_choice: usize,
    /// Index into App::type_options (a type is required).
    type_choice: usize,
    field: String,
    /// 0 = name, 1 = id, 2 = type, 3 = field, 4 = save, 5 = cancel.
    row: usize,
    dropdown: Option<Dropdown>,
}

impl App {
    fn new(session: Session) -> Self {
        Self {
            session,
            offset: 0,
            view_height: 1,
            focus: Focus::List,
            detail_scroll: 0,
            settings_open: false,
            settings_selected: 0,
            prompt: None,
            filter_popup: None,
            columns_popup: None,
            status: None,
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
                        || self.columns_popup.is_some()
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
        self.status = None;
        if self.settings_open {
            self.handle_settings_key(code);
            return true;
        }
        if self.filter_popup.is_some() {
            self.handle_filter_popup_key(code);
            return true;
        }
        if self.columns_popup.is_some() {
            self.handle_columns_popup_key(code);
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
                    input: self.session.filter_text.clone(),
                    error: None,
                })
            }
            KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Char('w') => self.save_setup(),
            KeyCode::Char('c') => {
                self.columns_popup = Some(ColumnsPopup {
                    row: 0,
                    editor: None,
                })
            }
            KeyCode::Char('C') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Columns,
                    input: self.session.columns_text.clone(),
                    error: None,
                })
            }
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
                Focus::List => self.select(self.session.filtered.len().saturating_sub(1)),
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
                        parse_jump(&input, self.session.time_format, self.session.start_us)
                            .map(|target_us| self.jump_to_time(target_us))
                    }
                    PromptKind::Filter => parse_filters(&input).map(|filters| {
                        self.session.filters = filters;
                        self.session.filter_text = input.trim().to_string();
                        self.apply_filter();
                    }),
                    PromptKind::Label(entry_index) => {
                        self.session.marks.insert(entry_index, input.trim().to_string());
                        Ok(())
                    }
                    PromptKind::Columns => {
                        parse_columns(&input).map(|columns| self.session.set_columns(columns))
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

    /// Restore a previously saved setup, showing the outcome in the status bar.
    fn load_setup(&mut self) {
        self.status = self.session.load_setup();
    }

    fn save_setup(&mut self) {
        self.status = Some(match self.session.save_setup() {
            Ok(path) => format!("Setup saved to {path}"),
            Err(err) => err,
        });
    }

    /// Toggle the mark on the selected message. Marking also opens the
    /// optional label prompt; unmarking discards the label.
    fn toggle_mark(&mut self) {
        let Some(entry_index) = self.session.selected_entry_index() else {
            return;
        };
        if self.session.marks.remove(&entry_index).is_none() {
            self.session.marks.insert(entry_index, String::new());
            self.prompt = Some(Prompt {
                kind: PromptKind::Label(entry_index),
                input: String::new(),
                error: None,
            });
        }
    }

    /// Jump to the target time and reset the detail scroll.
    fn jump_to_time(&mut self, target_us: u64) {
        self.session.jump_to_time(target_us);
        self.detail_scroll = 0;
    }

    /// Reapply the filter and fix up the viewport around the new selection.
    fn apply_filter(&mut self) {
        self.session.apply_filter();
        self.offset = self.offset.min(self.session.selected);
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
                popup.row = (popup.row + 1).min(self.session.filters.len())
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                if popup.row < self.session.filters.len() {
                    self.session.filters.remove(popup.row);
                    let popup = self.filter_popup.as_mut().unwrap();
                    popup.row = popup.row.min(self.session.filters.len());
                    self.sync_filter_text();
                }
            }
            KeyCode::Enter | KeyCode::Char('n') | KeyCode::Char('a') => {
                let editing = popup.row < self.session.filters.len() && code == KeyCode::Enter;
                let (index, expr) = if editing {
                    (Some(popup.row), Some(&self.session.filters[popup.row]))
                } else {
                    (None, None)
                };
                // Prefill the dropdowns from the expression where possible.
                let id_choice = expr
                    .and_then(|e| e.sysid.zip(e.compid))
                    .and_then(|pair| self.session.id_options.iter().position(|&p| p == pair))
                    .map_or(0, |i| i + 1);
                let type_choice = expr
                    .and_then(|e| e.name.as_deref())
                    .and_then(|n| {
                        self.session.type_options
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
            let labels = self.session.filter_dropdown_labels(field_row);
            let options = match_labels(&labels, &dropdown.query);
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
                let (s, c) = self.session.id_options[i];
                (Some(s), Some(c))
            }
            None => (None, None),
        };
        let name = editor
            .type_choice
            .checked_sub(1)
            .map(|i| self.session.type_options[i].to_ascii_lowercase());
        // Editing a disabled filter preserves its state; new ones start enabled.
        let enabled = editor
            .index
            .is_none_or(|i| self.session.filters[i].enabled);
        let expr = FilterExpr {
            sysid,
            compid,
            name,
            exact: true,
            enabled,
        };
        match editor.index {
            Some(i) => self.session.filters[i] = expr,
            None => {
                self.session.filters.push(expr);
                popup.row = self.session.filters.len() - 1;
            }
        }
        self.sync_filter_text();
    }

    /// Regenerate the editable filter text and reapply the filter.
    /// Regenerate the filter text from the expressions and reapply.
    fn sync_filter_text(&mut self) {
        self.session.rebuild_filter_text();
        self.apply_filter();
    }

    fn handle_columns_popup_key(&mut self, code: KeyCode) {
        let popup = self.columns_popup.as_mut().unwrap();
        if popup.editor.is_some() {
            self.handle_column_editor_key(code);
            return;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('c') => {
                self.columns_popup = None
            }
            KeyCode::Up | KeyCode::Char('k') => popup.row = popup.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                popup.row = (popup.row + 1).min(self.session.columns.len())
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                if popup.row < self.session.columns.len() {
                    let row = popup.row;
                    let mut columns = std::mem::take(&mut self.session.columns);
                    columns.remove(row);
                    self.session.set_columns(columns);
                    let popup = self.columns_popup.as_mut().unwrap();
                    popup.row = popup.row.min(self.session.columns.len());
                }
            }
            KeyCode::Enter | KeyCode::Char('n') | KeyCode::Char('a') => {
                let editing = popup.row < self.session.columns.len() && code == KeyCode::Enter;
                let editor = if editing {
                    let col = &self.session.columns[popup.row];
                    ColumnEditor {
                        index: Some(popup.row),
                        name: col.name.clone(),
                        id_choice: col
                            .sysid
                            .zip(col.compid)
                            .and_then(|pair| {
                                self.session.id_options.iter().position(|&p| p == pair)
                            })
                            .map_or(0, |i| i + 1),
                        type_choice: self
                            .session
                            .type_options
                            .iter()
                            .position(|t| t.eq_ignore_ascii_case(&col.msg_type))
                            .unwrap_or(0),
                        field: col.field.clone(),
                        row: 0,
                        dropdown: None,
                    }
                } else {
                    ColumnEditor {
                        index: None,
                        name: String::new(),
                        id_choice: 0,
                        type_choice: 0,
                        field: String::new(),
                        row: 0,
                        dropdown: None,
                    }
                };
                self.columns_popup.as_mut().unwrap().editor = Some(editor);
            }
            _ => {}
        }
    }

    fn handle_column_editor_key(&mut self, code: KeyCode) {
        let editor_ref = self.columns_popup.as_ref().unwrap().editor.as_ref().unwrap();
        let (field_row, type_choice) = (editor_ref.row, editor_ref.type_choice);

        // A dropdown is open: characters filter the options, arrows navigate.
        if let Some(dropdown) = &editor_ref.dropdown {
            let labels = self.session.column_dropdown_labels(field_row, type_choice);
            let options = match_labels(&labels, &dropdown.query);
            let editor = self.columns_popup.as_mut().unwrap().editor.as_mut().unwrap();
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
                            1 => editor.id_choice = choice,
                            2 => {
                                if editor.type_choice != choice {
                                    editor.type_choice = choice;
                                    // Fields belong to a type; reset on change.
                                    editor.field.clear();
                                }
                            }
                            _ => editor.field = labels[choice].clone(),
                        }
                        editor.dropdown = None;
                    }
                }
                _ => {}
            }
            return;
        }

        // Position of the current field within the field dropdown, computed
        // up front to avoid borrowing tangles when opening that dropdown.
        let field_highlight = {
            let labels = self.session.column_dropdown_labels(3, type_choice);
            labels
                .iter()
                .position(|l| l == &editor_ref.field)
                .unwrap_or(0)
        };

        let editor = self.columns_popup.as_mut().unwrap().editor.as_mut().unwrap();
        match code {
            KeyCode::Esc => self.columns_popup.as_mut().unwrap().editor = None,
            KeyCode::Up | KeyCode::BackTab => editor.row = editor.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => editor.row = (editor.row + 1).min(5),
            KeyCode::Left if editor.row == 5 => editor.row = 4,
            KeyCode::Right if editor.row == 4 => editor.row = 5,
            // The name row is a text input.
            KeyCode::Char(c) if editor.row == 0 => editor.name.push(c),
            KeyCode::Backspace if editor.row == 0 => {
                editor.name.pop();
            }
            KeyCode::Char('k') if editor.row != 0 => {
                editor.row = editor.row.saturating_sub(1)
            }
            KeyCode::Char('j') if editor.row != 0 => editor.row = (editor.row + 1).min(5),
            KeyCode::Enter => match editor.row {
                0 => editor.row = 1,
                1 | 2 | 3 => {
                    let highlight = match editor.row {
                        1 => editor.id_choice,
                        2 => editor.type_choice,
                        _ => field_highlight,
                    };
                    editor.dropdown = Some(Dropdown {
                        query: String::new(),
                        highlight,
                    });
                }
                4 => self.save_column_editor(),
                _ => self.columns_popup.as_mut().unwrap().editor = None,
            },
            _ => {}
        }
    }

    fn save_column_editor(&mut self) {
        let editor = self.columns_popup.as_ref().unwrap().editor.as_ref().unwrap();
        if editor.field.is_empty() {
            // A field is required; a column can't be saved without one.
            self.status = Some("Pick a message field before saving".to_string());
            return;
        }
        let name = if editor.name.trim().is_empty() {
            editor.field.clone()
        } else {
            editor.name.trim().to_string()
        };
        let (sysid, compid) = match editor.id_choice.checked_sub(1) {
            Some(i) => {
                let (s, c) = self.session.id_options[i];
                (Some(s), Some(c))
            }
            None => (None, None),
        };
        let column = CustomColumn {
            name,
            sysid,
            compid,
            msg_type: self.session.type_options[editor.type_choice].clone(),
            field: editor.field.clone(),
            matches: Vec::new(),
        };
        let index = editor.index;
        let mut columns = std::mem::take(&mut self.session.columns);
        match index {
            Some(i) => columns[i] = column,
            None => columns.push(column),
        }
        self.session.set_columns(columns);
        let popup = self.columns_popup.as_mut().unwrap();
        popup.editor = None;
        popup.row = index.unwrap_or(self.session.columns.len() - 1);
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
                (SETTING_ITEMS[self.settings_selected].toggle)(&mut self.session)
            }
            _ => {}
        }
    }

    /// Scroll whichever pane has focus.
    fn scroll_by(&mut self, delta: isize) {
        match self.focus {
            Focus::List => {
                if self.session.filtered.is_empty() {
                    return;
                }
                self.select(
                    self.session.selected
                        .saturating_add_signed(delta)
                        .min(self.session.filtered.len() - 1),
                );
            }
            Focus::Detail => {
                self.detail_scroll = self.detail_scroll.saturating_add_signed(delta);
            }
        }
    }

    fn select(&mut self, index: usize) {
        if index != self.session.selected {
            self.session.selected = index;
            self.detail_scroll = 0;
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        // The filter pane only exists while filters are active.
        let filter_height = if self.session.filters.is_empty() {
            0
        } else {
            self.session.filters.len() as u16 + 2
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
        let count = if self.session.filters.is_empty() {
            format!("{} messages", self.session.entries.len())
        } else {
            format!("{} of {} messages", self.session.filtered.len(), self.session.entries.len())
        };
        frame.render_widget(
            Line::styled(
                format!(" {}  —  {count}", self.session.path),
                bar_style.add_modifier(Modifier::BOLD),
            ),
            header_area,
        );

        if !self.session.filters.is_empty() {
            self.draw_filter_pane(frame, filter_area);
        }
        self.draw_list(frame, list_area);
        self.draw_detail(frame, detail_area);

        if let Some(prompt) = &self.prompt {
            let label = match prompt.kind {
                PromptKind::Jump => match self.session.time_format {
                    TimeFormat::DateTime => " Jump to time (HH:MM:SS[.mmm] or YYYY-MM-DD HH:MM:SS): ",
                    TimeFormat::OffsetSecs => " Jump to offset (seconds): ",
                },
                PromptKind::Filter => " Filter (e.g. 1:1 HEARTBEAT, GPS*, 255 — empty clears): ",
                PromptKind::Label(_) => " Label for marked message (Enter to save, Esc to skip): ",
                PromptKind::Columns => {
                    " Columns (NAME = [sys[:comp]] TYPE.FIELD, comma-separated — empty clears): "
                }
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
            let position = if self.session.filtered.is_empty() {
                "0/0 ".to_string()
            } else {
                format!("{}/{} ", self.session.selected + 1, self.session.filtered.len())
            };
            let status_text;
            let hints = if let Some(status) = &self.status {
                status_text = format!(" {status}");
                status_text.as_str()
            } else if self.settings_open {
                " ↑/↓ select  Enter/Space toggle  Esc close"
            } else if let Some(popup) = &self.filter_popup {
                match &popup.editor {
                    Some(editor) if editor.dropdown.is_some() => {
                        " type to filter  ↑/↓ choose  Enter select  Esc cancel"
                    }
                    Some(_) => " ↑/↓ fields  Enter open/confirm  Esc back",
                    None => " ↑/↓ select  Enter edit  n new  d delete  Esc close",
                }
            } else if let Some(popup) = &self.columns_popup {
                match &popup.editor {
                    Some(editor) if editor.dropdown.is_some() => {
                        " type to filter  ↑/↓ choose  Enter select  Esc cancel"
                    }
                    Some(_) => " ↑/↓ fields  type in Name  Enter open/confirm  Esc back",
                    None => " ↑/↓ select  Enter edit  n new  d delete  Esc close",
                }
            } else {
                match self.focus {
                    Focus::List => " ↑/↓ select  →/Tab details  Space mark  t jump  f filter  c columns  s settings  w save  q quit",
                    Focus::Detail => " ↑/↓ scroll  ←/Esc list  Space mark  t jump  f filter  c columns  s settings  w save  q quit",
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
        self.draw_columns_popup(frame);
    }

    fn draw_columns_popup(&self, frame: &mut Frame) {
        let Some(popup) = &self.columns_popup else {
            return;
        };
        if let Some(editor) = &popup.editor {
            self.draw_column_editor(frame, editor);
            return;
        }

        let height = self.session.columns.len() as u16 + 3;
        let area = centered_fixed(frame.area(), 56, height);
        let lines: Vec<Line> = (0..=self.session.columns.len())
            .map(|i| {
                let text = if i < self.session.columns.len() {
                    format!(" {} ", self.session.columns[i].to_text())
                } else {
                    " + Add column ".to_string()
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
                " Columns ",
                Style::new().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::raw(" Enter edit  n new  d delete  Esc ").right_aligned())
            .border_style(Style::new().fg(Color::Cyan));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn draw_column_editor(&self, frame: &mut Frame, editor: &ColumnEditor) {
        let area = centered_fixed(frame.area(), 52, 8);
        let focused = |row: usize| editor.row == row && editor.dropdown.is_none();
        let field_style = |row: usize| {
            if focused(row) {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new()
            }
        };
        let dropdown_field = |label: &str, value: String, row: usize| {
            Line::from(vec![
                Span::raw(format!(" {label:<18}")),
                Span::styled(format!("[ {value:<22} ▾ ]"), field_style(row)),
            ])
        };
        let name_value = if focused(0) {
            format!("{}█", editor.name)
        } else {
            editor.name.clone()
        };
        let field_value = if editor.field.is_empty() {
            "(pick a field)".to_string()
        } else {
            editor.field.clone()
        };
        let lines = vec![
            Line::from(vec![
                Span::raw(format!(" {:<18}", "Name")),
                Span::styled(format!("[ {name_value:<24} ]"), field_style(0)),
            ]),
            dropdown_field("System:Component", self.session.id_option_text(editor.id_choice), 1),
            dropdown_field(
                "Message type",
                self.session.type_options
                    .get(editor.type_choice)
                    .cloned()
                    .unwrap_or_default(),
                2,
            ),
            dropdown_field("Field", field_value, 3),
            Line::raw(""),
            Line::from(vec![
                Span::raw(" ".repeat(14)),
                Span::styled("[ Save ]", field_style(4)),
                Span::raw("    "),
                Span::styled("[ Cancel ]", field_style(5)),
            ]),
        ];

        let title = if editor.index.is_some() {
            " Edit column "
        } else {
            " New column "
        };
        let block = Block::bordered()
            .title(Line::styled(title, Style::new().add_modifier(Modifier::BOLD)))
            .border_style(Style::new().fg(Color::Cyan));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);

        if let Some(dropdown) = &editor.dropdown {
            let title = match editor.row {
                1 => " System:Component ",
                2 => " Message type ",
                _ => " Field ",
            };
            let labels = self.session.column_dropdown_labels(editor.row, editor.type_choice);
            self.draw_dropdown(frame, title, &labels, dropdown);
        }
    }

    fn draw_filter_pane(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let lines: Vec<Line> = self
            .session
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

        let height = self.session.filters.len() as u16 + 3;
        let area = centered_fixed(frame.area(), 48, height);
        let lines: Vec<Line> = (0..=self.session.filters.len())
            .map(|i| {
                let text = if i < self.session.filters.len() {
                    format!(" {} ", self.session.filters[i].to_text())
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
            field("System:Component", self.session.id_option_text(editor.id_choice), 0),
            field("Message type", self.session.type_option_text(editor.type_choice), 1),
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
            let title = match editor.row {
                0 => " System:Component ",
                _ => " Message type ",
            };
            let labels = self.session.filter_dropdown_labels(editor.row);
            self.draw_dropdown(frame, title, &labels, dropdown);
        }
    }

    fn draw_dropdown(
        &self,
        frame: &mut Frame,
        title: &str,
        labels: &[String],
        dropdown: &Dropdown,
    ) {
        let options = match_labels(labels, &dropdown.query);
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
                let text = format!(" {} ", labels[options[i]]);
                if i == dropdown.highlight {
                    Line::styled(text, Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    Line::raw(text)
                }
            }));
        }

        let mut block = Block::bordered()
            .title(Line::styled(
                title.to_string(),
                Style::new().add_modifier(Modifier::BOLD),
            ))
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
                let value = format!("◂ {} ▸", (item.value)(&self.session));
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
        if self.session.selected < self.offset {
            self.offset = self.session.selected;
        } else if self.session.selected >= self.offset + self.view_height {
            self.offset = self.session.selected - self.view_height + 1;
        }

        let end = (self.offset + self.view_height).min(self.session.filtered.len());
        let selection_style = if self.focus == Focus::List {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new().fg(Color::Black).bg(Color::DarkGray)
        };
        let time_width = match self.session.time_format {
            TimeFormat::DateTime => 23,
            TimeFormat::OffsetSecs => 12,
        };
        let rows = self.session.filtered[self.offset.min(end)..end]
            .iter()
            .enumerate()
            .map(|(i, &entry_index)| {
                let position = self.offset + i;
                let entry = &self.session.entries[entry_index];
                let mark = self.session.marks.get(&entry_index);
                let label = match mark {
                    Some(label) if label.is_empty() => "●".to_string(),
                    Some(label) => format!("● {label}"),
                    None => String::new(),
                };
                let mut cells = vec![
                    format!("{entry_index:>7}"),
                    self.session.format_list_time(entry.timestamp_us),
                    format!("{:>7}", entry.id_label()),
                    entry.name.clone(),
                ];
                cells.extend(
                    self.session.columns
                        .iter()
                        .map(|col| self.session.column_value(col, entry_index)),
                );
                cells.push(label);
                let row = Row::new(cells);
                if position == self.session.selected {
                    row.style(selection_style)
                } else if mark.is_some() {
                    row.style(Style::new().fg(Color::White).bg(Color::Red))
                } else {
                    row
                }
            });

        let mut constraints = vec![
            Constraint::Length(7),
            Constraint::Length(time_width),
            Constraint::Length(7),
            Constraint::Min(12),
        ];
        let mut header = vec![
            "#".to_string(),
            "TIME".to_string(),
            "SYS:CMP".to_string(),
            "MESSAGE".to_string(),
        ];
        for col in &self.session.columns {
            constraints.push(Constraint::Length(col.name.len().max(8) as u16));
            header.push(col.name.clone());
        }
        constraints.push(Constraint::Length(if self.session.columns.is_empty() { 14 } else { 10 }));
        header.push("LABEL".to_string());

        let table = Table::new(rows, constraints)
        .header(
            Row::new(header)
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        )
        .block(self.pane_block(Focus::List).title(" Messages "));
        frame.render_widget(table, area);
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let Some(&entry_index) = self.session.filtered.get(self.session.selected) else {
            frame.render_widget(
                Paragraph::new("\n No messages match the filter.")
                    .block(self.pane_block(Focus::Detail)),
                area,
            );
            return;
        };
        let entry = &self.session.entries[entry_index];
        let mut body = format!(
            "Time: {}  ({})\n",
            format_datetime(entry.timestamp_us),
            format_offset(entry.timestamp_us, self.session.start_us),
        );
        if let Some(label) = self.session.marks.get(&entry_index) {
            body.push_str("Mark: ●");
            if !label.is_empty() {
                body.push_str(&format!(" {label}"));
            }
            body.push('\n');
        }
        body.push('\n');
        body.push_str(&match tlog::decode(&self.session.data, entry) {
            Some(msg) => format!("{msg:#?}"),
            None => tlog::hex_dump(&self.session.data[entry.payload.clone()]),
        });
        let lines: Vec<&str> = body.lines().collect();

        let inner_height = (area.height as usize).saturating_sub(2);
        self.detail_scroll = self
            .detail_scroll
            .min(lines.len().saturating_sub(inner_height));

        let title = format!(" {} (id {}) ", entry.name, entry.msg_id());
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

