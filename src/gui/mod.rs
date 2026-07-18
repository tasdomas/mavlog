//! The graphical (egui/eframe) frontend. Owns an optional `Session` (none
//! until a file is opened) plus GUI-only view state.

mod columns;
mod filters;
mod plots;
mod widgets;

use anyhow::{anyhow, Result};
use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Modifiers};
use egui_extras::{Column, TableBuilder};

use crate::core::session::Session;
use crate::core::time::{format_datetime, format_offset, parse_jump, TimeFormat};
use crate::tlog;

const OPEN_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const JUMP_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::J);
const FILTERS_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
const COLUMNS_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::C);
const PLOTS_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::P);
const SETTINGS_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma);
const HELP_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F1);
/// Consumed by whichever of the Filters/Columns/Plots windows is open and
/// calls `ctx.input_mut` first each frame — see `GuiApp::ui`'s fixed call
/// order (filters, then columns, then plots) for the tie-break.
pub(crate) const ADD_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const MARK_BG: Color32 = Color32::from_rgb(140, 20, 20);

/// A context-menu action on a message row, applied after the table body
/// closure (which only borrows `Session`) finishes.
enum MarkAction {
    Add,
    Remove,
    EditLabel,
}

/// Launch the native GUI window. `session` may be None, in which case the
/// window opens in its empty state awaiting a file.
pub fn run(session: Option<Session>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "mavlog",
        options,
        Box::new(|_cc| Ok(Box::new(GuiApp::new(session)))),
    )
    .map_err(|e| anyhow!("gui error: {e}"))
}

struct GuiApp {
    session: Option<Session>,
    /// Transient status line (e.g. setup load result).
    status: Option<String>,
    /// Set when a file fails to open; shown in a modal until dismissed.
    error: Option<String>,
    /// Text in the jump-to-time toolbar box.
    jump_input: String,
    /// Set when the jump input fails to parse.
    jump_error: Option<String>,
    /// When true, the list should scroll the selection into view next frame,
    /// then clear itself. Set by keyboard nav and jump-to-time, not by
    /// mouse clicks (so a click never fights the user for scroll position).
    scroll_to_selected: bool,
    /// Rows visible in the list on the last frame, for Page Up/Down.
    visible_rows: usize,
    /// Whether the Settings window is open.
    settings_open: bool,
    /// Entry index currently being labeled, if the label window is open.
    label_prompt: Option<usize>,
    /// Text in the label window.
    label_input: String,
    /// Whether the Filters window is open.
    filters_open: bool,
    filters_state: filters::FiltersState,
    /// Whether the Columns window is open.
    columns_open: bool,
    columns_state: columns::ColumnsState,
    plots_state: plots::PlotsState,
    /// Whether the Help window is open.
    help_open: bool,
    /// Set by the jump-focus shortcut; the toolbar requests focus on the
    /// jump box next time it's drawn, then clears this.
    focus_jump: bool,
    /// Whether any widget had keyboard focus at the end of the previous
    /// frame. `Esc` uses this instead of a live `ctx.memory` query: egui's
    /// own input processing already blurs a focused widget on `Esc` before
    /// our code runs each frame, so a live query can never tell "something
    /// was just blurred by this same keypress" apart from "nothing was ever
    /// focused" — both read as unfocused by the time we look.
    focused_last_frame: bool,
}

impl GuiApp {
    fn new(mut session: Option<Session>) -> Self {
        let status = session.as_mut().and_then(Session::load_setup);
        Self {
            session,
            status,
            error: None,
            jump_input: String::new(),
            jump_error: None,
            scroll_to_selected: false,
            visible_rows: 20,
            settings_open: false,
            label_prompt: None,
            label_input: String::new(),
            filters_open: false,
            filters_state: filters::FiltersState::default(),
            columns_open: false,
            columns_state: columns::ColumnsState::default(),
            plots_state: plots::PlotsState::default(),
            help_open: false,
            focus_jump: false,
            focused_last_frame: false,
        }
    }

    fn title(&self) -> String {
        match &self.session {
            Some(s) => format!("{}  —  {} messages", s.path, s.entries.len()),
            None => "No file open".to_string(),
        }
    }

    /// Show the native "open file" dialog and load the chosen file, if any.
    fn pick_and_open(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("MAVLink tlog", &["tlog"])
            .pick_file()
        {
            self.open_path(&path);
        }
    }

    /// Read, parse and load a tlog file, replacing the current session on
    /// success or setting `self.error` on failure.
    fn open_path(&mut self, path: &std::path::Path) {
        match crate::load_session(&path.to_string_lossy()) {
            Ok(mut session) => {
                self.status = session.load_setup();
                self.session = Some(session);
                self.error = None;
                self.scroll_to_selected = true;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Move the selection by `delta` rows, clamped to the visible range.
    fn move_selection(&mut self, delta: isize) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.filtered.is_empty() {
            return;
        }
        let max = session.filtered.len() - 1;
        session.selected = session.selected.saturating_add_signed(delta).min(max);
        self.scroll_to_selected = true;
    }

    fn select(&mut self, index: usize) {
        let Some(session) = &mut self.session else {
            return;
        };
        session.selected = index.min(session.filtered.len().saturating_sub(1));
    }

    /// Parse and act on the jump-to-time box.
    fn jump(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        match parse_jump(&self.jump_input, session.time_format, session.start_us) {
            Ok(target_us) => {
                session.jump_to_time(target_us);
                self.scroll_to_selected = true;
                self.jump_error = None;
            }
            Err(err) => self.jump_error = Some(err),
        }
    }

    /// Toggle the mark on the selected message, opening the label window
    /// when a new mark is added. Unmarking discards the label.
    fn toggle_mark(&mut self) {
        let Some(entry_index) = self
            .session
            .as_ref()
            .and_then(Session::selected_entry_index)
        else {
            return;
        };
        self.toggle_mark_for(entry_index);
    }

    fn toggle_mark_for(&mut self, entry_index: usize) {
        let Some(session) = &mut self.session else {
            return;
        };
        let newly_marked = session.marks.remove(&entry_index).is_none();
        if newly_marked {
            session.marks.insert(entry_index, String::new());
        }
        if newly_marked {
            self.open_label_editor(entry_index);
        }
    }

    /// Open the label window for a message, prefilled with its current label.
    fn open_label_editor(&mut self, entry_index: usize) {
        self.label_input = self
            .session
            .as_ref()
            .and_then(|s| s.marks.get(&entry_index).cloned())
            .unwrap_or_default();
        self.label_prompt = Some(entry_index);
    }

    fn label_window(&mut self, ctx: &egui::Context) {
        let Some(entry_index) = self.label_prompt else {
            return;
        };
        if self.session.is_none() {
            self.label_prompt = None;
            return;
        }
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("Mark label")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let response = ui.text_edit_singleline(&mut self.label_input);
                let submitted =
                    response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || submitted {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if save {
            if let Some(session) = &mut self.session {
                session
                    .marks
                    .insert(entry_index, self.label_input.trim().to_string());
            }
            self.label_prompt = None;
        } else if cancel {
            self.label_prompt = None;
        }
    }

    /// Write the current setup to the sidecar, showing the outcome in the
    /// status bar.
    fn save_setup(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        self.status = Some(match session.save_setup() {
            Ok(path) => format!("Setup saved to {path}"),
            Err(err) => err,
        });
    }

    /// Consume the toolbar's modifier shortcuts (Cmd/Ctrl-based; these work
    /// regardless of focus, like any app-level shortcut), then — only where
    /// nothing has keyboard focus — their plain-letter TUI-parity
    /// equivalents. Consuming a modifier shortcut first matters: egui's
    /// `key_pressed` ignores modifiers entirely, so an un-consumed Cmd+F
    /// press would also satisfy a later plain `f` check and double-toggle
    /// the same window shut immediately after opening it.
    fn handle_toolbar_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_shortcut(&OPEN_SHORTCUT)) {
            self.pick_and_open();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SAVE_SHORTCUT)) {
            self.save_setup();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&JUMP_SHORTCUT)) {
            self.focus_jump = true;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&FILTERS_SHORTCUT)) {
            self.filters_open = !self.filters_open;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&COLUMNS_SHORTCUT)) {
            self.columns_open = !self.columns_open;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&PLOTS_SHORTCUT)) {
            self.plots_state.toggle_manager();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SETTINGS_SHORTCUT)) {
            self.settings_open = !self.settings_open;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&HELP_SHORTCUT)) {
            self.help_open = !self.help_open;
        }

        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Questionmark)) {
            self.help_open = !self.help_open;
        }
        if ctx.input(|i| i.key_pressed(Key::O)) {
            self.pick_and_open();
        }
    }

    /// Handle list-navigation keys and the session-only plain-letter
    /// shortcuts, unless a text field currently has focus.
    fn handle_nav_keys(&mut self, ctx: &egui::Context) {
        if self.session.is_none() || ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let page = self.visible_rows.max(1) as isize;
        if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
            self.move_selection(1);
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
            self.move_selection(-1);
        }
        if ctx.input(|i| i.key_pressed(Key::PageDown)) {
            self.move_selection(page);
        }
        if ctx.input(|i| i.key_pressed(Key::PageUp)) {
            self.move_selection(-page);
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) {
            self.select(0);
            self.scroll_to_selected = true;
        }
        if ctx.input(|i| i.key_pressed(Key::End)) {
            if let Some(session) = &self.session {
                let last = session.filtered.len().saturating_sub(1);
                self.select(last);
                self.scroll_to_selected = true;
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.toggle_mark();
        }
        // Plain-letter, TUI-parity equivalents of the toolbar shortcuts.
        // Modifier forms are handled (and consumed first) in
        // handle_toolbar_shortcuts.
        if ctx.input(|i| i.key_pressed(Key::W)) {
            self.save_setup();
        }
        if ctx.input(|i| i.key_pressed(Key::T)) {
            self.focus_jump = true;
        }
        if ctx.input(|i| i.key_pressed(Key::F)) {
            self.filters_open = !self.filters_open;
        }
        if ctx.input(|i| i.key_pressed(Key::C)) {
            self.columns_open = !self.columns_open;
        }
        if ctx.input(|i| i.key_pressed(Key::P)) {
            self.plots_state.toggle_manager();
        }
        if ctx.input(|i| i.key_pressed(Key::S)) {
            self.settings_open = !self.settings_open;
        }
    }

    /// Esc closes whichever window is open, in a fixed precedence (most
    /// transient first) since egui doesn't track window stacking order for
    /// us. One press closes at most one window: it cancels an open popup
    /// editor before closing that popup's window.
    fn handle_escape(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.key_pressed(Key::Escape)) {
            return;
        }
        if self.focused_last_frame {
            // egui already blurred the focused widget for us this frame;
            // don't also close a window on the same keypress.
            return;
        }
        if self.error.is_some() {
            self.error = None;
        } else if self.label_prompt.is_some() {
            self.label_prompt = None;
        } else if self.help_open {
            self.help_open = false;
        } else if self.settings_open {
            self.settings_open = false;
        } else if self.filters_open && self.filters_state.has_editor() {
            self.filters_state.cancel_editor();
        } else if self.filters_open {
            self.filters_open = false;
        } else if self.columns_open && self.columns_state.has_editor() {
            self.columns_state.cancel_editor();
        } else if self.columns_open {
            self.columns_open = false;
        } else if self.plots_state.is_manager_open() && self.plots_state.has_editor() {
            self.plots_state.cancel_editor();
        } else if self.plots_state.is_manager_open() {
            self.plots_state.close_manager();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if ui
                .button("Open…")
                .on_hover_text(hint(&ctx, &OPEN_SHORTCUT, "o"))
                .clicked()
            {
                self.pick_and_open();
            }
            ui.separator();
            ui.label(self.title());
            if self.session.is_some() {
                ui.separator();
                ui.label("Jump to:");
                let response = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.jump_input)
                            .hint_text(match self.session.as_ref().unwrap().time_format {
                                TimeFormat::DateTime => "2024-07-17 07:06:40",
                                TimeFormat::OffsetSecs => "T+4.5s",
                            })
                            .desired_width(160.0),
                    )
                    .on_hover_text(format!("Focus: {}", hint(&ctx, &JUMP_SHORTCUT, "t")));
                if self.focus_jump {
                    response.request_focus();
                    self.focus_jump = false;
                }
                let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Jump").on_hover_text("Enter").clicked() || submitted {
                    self.jump();
                }
                if let Some(err) = &self.jump_error {
                    ui.colored_label(Color32::LIGHT_RED, err);
                }
                ui.separator();
                if ui
                    .button("Filters")
                    .on_hover_text(hint(&ctx, &FILTERS_SHORTCUT, "f"))
                    .clicked()
                {
                    self.filters_open = true;
                }
                if ui
                    .button("Columns")
                    .on_hover_text(hint(&ctx, &COLUMNS_SHORTCUT, "c"))
                    .clicked()
                {
                    self.columns_open = true;
                }
                if ui
                    .button("Plots")
                    .on_hover_text(hint(&ctx, &PLOTS_SHORTCUT, "p"))
                    .clicked()
                {
                    self.plots_state.open_manager();
                }
                if ui
                    .button("Save setup")
                    .on_hover_text(hint(&ctx, &SAVE_SHORTCUT, "w"))
                    .clicked()
                {
                    self.save_setup();
                }
                if ui
                    .button("Settings")
                    .on_hover_text(hint(&ctx, &SETTINGS_SHORTCUT, "s"))
                    .clicked()
                {
                    self.settings_open = true;
                }
            }
            ui.separator();
            if ui
                .button("Help")
                .on_hover_text(hint(&ctx, &HELP_SHORTCUT, "?"))
                .clicked()
            {
                self.help_open = true;
            }
        });
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.help_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Help")
            .open(&mut open)
            .collapsible(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Keyboard shortcuts").strong());
                ui.add_space(4.0);
                for (keys, action) in [
                    ("Ctrl/Cmd+O or o", "Open a tlog file"),
                    ("Ctrl/Cmd+S or w", "Save the setup sidecar"),
                    ("Ctrl/Cmd+J or t", "Focus the jump-to-time box"),
                    ("Ctrl/Cmd+F or f", "Toggle the Filters window"),
                    ("Ctrl/Cmd+Shift+C or c", "Toggle the Columns window"),
                    ("Ctrl/Cmd+P or p", "Toggle the Plots manager"),
                    ("Ctrl/Cmd+, or s", "Toggle Settings"),
                    ("F1 or ?", "Toggle this Help window"),
                    ("Ctrl/Cmd+N", "Add a filter/column/plot (in that window)"),
                    ("Up / Down", "Move the selection"),
                    ("Page Up / Page Down", "Move the selection by a page"),
                    ("Home / End", "Jump to the first / last message"),
                    ("Space", "Toggle a mark on the selected message"),
                    ("Enter", "Submit a box, save an open editor, or dismiss an error"),
                    ("Esc", "Release focus, then cancel an editor, then close a window"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(keys).monospace().strong());
                        ui.label(action);
                    });
                }
                ui.add_space(4.0);
                ui.label("Plain letters only fire when no text field has focus.");
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Mouse").strong());
                ui.add_space(4.0);
                for (action, desc) in [
                    ("Click a row", "Select that message"),
                    ("Right-click a row", "Add/edit/remove its mark"),
                    ("Drag a file onto the window", "Open it"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(action).strong());
                        ui.label(desc);
                    });
                }
            });
        self.help_open = open;
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let Some(session) = &mut self.session else {
            self.settings_open = false;
            return;
        };
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Time column:");
                ui.radio_value(&mut session.time_format, TimeFormat::DateTime, "Date-time");
                ui.radio_value(&mut session.time_format, TimeFormat::OffsetSecs, "Offset (s)");
            });
        self.settings_open = open;
    }

    fn list_panel(&mut self, ui: &mut egui::Ui) {
        let Some(session) = &mut self.session else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label("Open a .tlog file to begin, or drag one onto this window.");
                });
            });
            return;
        };

        let time_width = match session.time_format {
            TimeFormat::DateTime => 170.0,
            TimeFormat::OffsetSecs => 100.0,
        };
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        self.visible_rows = (ui.available_height() / row_height).floor().max(1.0) as usize;

        let mut builder = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .column(Column::exact(60.0))
            .column(Column::exact(time_width))
            .column(Column::exact(70.0))
            .column(Column::initial(160.0).at_least(80.0).resizable(true));
        for col in &session.columns {
            builder = builder.column(
                Column::initial(col.name.len().max(8) as f32 * 7.0)
                    .at_least(60.0)
                    .resizable(true),
            );
        }
        builder = builder.column(Column::remainder().at_least(80.0));

        if self.scroll_to_selected {
            builder = builder.scroll_to_row(session.selected, Some(Align::Center));
        }

        let mut clicked_row = None;
        let mut mark_action: Option<(usize, MarkAction)> = None;
        builder
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.strong("#");
                });
                header.col(|ui| {
                    ui.strong("TIME");
                });
                header.col(|ui| {
                    ui.strong("SYS:CMP");
                });
                header.col(|ui| {
                    ui.strong("MESSAGE");
                });
                for col in &session.columns {
                    header.col(|ui| {
                        ui.strong(&col.name);
                    });
                }
                header.col(|ui| {
                    ui.strong("LABEL");
                });
            })
            .body(|body| {
                body.rows(row_height, session.filtered.len(), |mut row| {
                    let row_index = row.index();
                    let entry_index = session.filtered[row_index];
                    let entry = &session.entries[entry_index];
                    let mark = session.marks.get(&entry_index);
                    let is_selected = row_index == session.selected;
                    row.set_selected(is_selected);
                    let mark_bg = (!is_selected && mark.is_some()).then_some(MARK_BG);

                    cell(&mut row, mark_bg, |ui| {
                        ui.label(entry_index.to_string());
                    });
                    cell(&mut row, mark_bg, |ui| {
                        ui.label(session.format_list_time(entry.timestamp_us));
                    });
                    cell(&mut row, mark_bg, |ui| {
                        ui.label(format!("{}:{}", entry.sysid, entry.compid));
                    });
                    cell(&mut row, mark_bg, |ui| {
                        ui.label(&entry.name);
                    });
                    for col in &session.columns {
                        let value = session.column_value(col, entry_index);
                        cell(&mut row, mark_bg, |ui| {
                            ui.label(value);
                        });
                    }
                    let label = match mark {
                        Some(label) if label.is_empty() => "●".to_string(),
                        Some(label) => format!("● {label}"),
                        None => String::new(),
                    };
                    cell(&mut row, mark_bg, |ui| {
                        ui.label(label);
                    });

                    let response = row.response();
                    if response.clicked() {
                        clicked_row = Some(row_index);
                    }
                    let is_marked = mark.is_some();
                    response.context_menu(|ui| {
                        if is_marked {
                            if ui.button("Edit label").clicked() {
                                mark_action = Some((entry_index, MarkAction::EditLabel));
                                ui.close();
                            }
                            if ui.button("Remove mark").clicked() {
                                mark_action = Some((entry_index, MarkAction::Remove));
                                ui.close();
                            }
                        } else if ui.button("Add mark").clicked() {
                            mark_action = Some((entry_index, MarkAction::Add));
                            ui.close();
                        }
                    });
                });
            });

        if let Some(row_index) = clicked_row {
            session.selected = row_index;
        }
        let mut open_label_for = None;
        if let Some((entry_index, action)) = mark_action {
            match action {
                MarkAction::Add => {
                    session.marks.insert(entry_index, String::new());
                    open_label_for = Some(entry_index);
                }
                MarkAction::Remove => {
                    session.marks.remove(&entry_index);
                }
                MarkAction::EditLabel => open_label_for = Some(entry_index),
            }
        }
        self.scroll_to_selected = false;
        if let Some(entry_index) = open_label_for {
            self.open_label_editor(entry_index);
        }
    }

    fn detail_panel(&self, ui: &mut egui::Ui) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(&entry_index) = session.filtered.get(session.selected) else {
            ui.label("No messages match the filter.");
            return;
        };
        let entry = &session.entries[entry_index];
        ui.label(
            egui::RichText::new(format!("{} (id {})", entry.name, entry.msg_id)).strong(),
        );
        let mut body = format!(
            "Time: {}  ({})\n",
            format_datetime(entry.timestamp_us),
            format_offset(entry.timestamp_us, session.start_us),
        );
        if let Some(label) = session.marks.get(&entry_index) {
            body.push_str("Mark: ●");
            if !label.is_empty() {
                body.push_str(&format!(" {label}"));
            }
            body.push('\n');
        }
        body.push('\n');
        body.push_str(&match tlog::decode(&session.data, entry) {
            Ok(msg) => format!("{msg:#?}"),
            Err(_) => tlog::hex_dump(&session.data[entry.payload.clone()]),
        });

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut body)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
    }
}

/// Tooltip text for a toolbar button: the platform-formatted modifier
/// shortcut plus its plain-letter equivalent, e.g. "⌘F or f" on macOS.
fn hint(ctx: &egui::Context, shortcut: &KeyboardShortcut, plain: &str) -> String {
    format!("{} or {plain}", ctx.format_shortcut(shortcut))
}

/// Paint an optional flat background behind a cell, then run its contents.
fn cell(
    row: &mut egui_extras::TableRow<'_, '_>,
    bg: Option<Color32>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    row.col(|ui| {
        if let Some(color) = bg {
            ui.painter().rect_filled(ui.max_rect(), 0.0, color);
        }
        add_contents(ui);
    });
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.iter().find_map(|f| f.path.clone()) {
            self.open_path(&path);
        }

        // Modifier shortcuts must be consumed before the plain-letter checks
        // in handle_nav_keys so a Cmd-modified press can't also satisfy the
        // unmodified check for the same key (see handle_toolbar_shortcuts).
        self.handle_toolbar_shortcuts(&ctx);
        self.handle_nav_keys(&ctx);
        self.handle_escape(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(self.status.as_deref().unwrap_or(""));
        });

        if self.session.is_some() {
            egui::Panel::right("detail").default_size(420.0).show(ui, |ui| {
                self.detail_panel(ui);
            });
        }

        egui::CentralPanel::default().show(ui, |ui| self.list_panel(ui));

        self.settings_window(&ctx);
        self.label_window(&ctx);
        self.help_window(&ctx);
        if self.filters_open {
            if let Some(session) = &mut self.session {
                filters::show(&ctx, &mut self.filters_open, session, &mut self.filters_state);
            } else {
                self.filters_open = false;
            }
        }
        if self.columns_open {
            if let Some(session) = &mut self.session {
                columns::show(&ctx, &mut self.columns_open, session, &mut self.columns_state);
            } else {
                self.columns_open = false;
            }
        }
        if let Some(session) = &mut self.session {
            plots::show_manager(&ctx, session, &mut self.plots_state);
            plots::show_open_plots(&ctx, session, &mut self.plots_state);
        }

        if let Some(message) = self.error.clone() {
            let mut dismiss = false;
            egui::Window::new("Failed to open file")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label(message);
                    if ui.button("OK").clicked() {
                        dismiss = true;
                    }
                });
            if dismiss || ctx.input(|i| i.key_pressed(Key::Enter)) {
                self.error = None;
            }
        }

        // Captured for next frame's handle_escape — see focused_last_frame's
        // doc comment for why a live query there can't do this job.
        self.focused_last_frame = ctx.memory(|m| m.focused().is_some());
    }
}
