//! The graphical (egui/eframe) frontend. Owns an optional `Session` (none
//! until a file is opened) plus GUI-only view state.

use anyhow::{anyhow, Result};
use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Modifiers};
use egui_extras::{Column, TableBuilder};

use crate::core::session::Session;
use crate::core::time::{format_datetime, format_offset, parse_jump, TimeFormat};
use crate::tlog;

const OPEN_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const MARK_BG: Color32 = Color32::from_rgb(140, 20, 20);

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

    /// Handle list-navigation keys, unless a text field currently has focus.
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
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open…").clicked() {
                self.pick_and_open();
            }
            ui.separator();
            ui.label(self.title());
            if self.session.is_some() {
                ui.separator();
                ui.label("Jump to:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.jump_input)
                        .hint_text(match self.session.as_ref().unwrap().time_format {
                            TimeFormat::DateTime => "2024-07-17 07:06:40",
                            TimeFormat::OffsetSecs => "T+4.5s",
                        })
                        .desired_width(160.0),
                );
                let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Jump").clicked() || submitted {
                    self.jump();
                }
                if let Some(err) = &self.jump_error {
                    ui.colored_label(Color32::LIGHT_RED, err);
                }
            }
        });
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

                    if row.response().clicked() {
                        clicked_row = Some(row_index);
                    }
                });
            });

        if let Some(row_index) = clicked_row {
            session.selected = row_index;
        }
        self.scroll_to_selected = false;
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

        if ctx.input_mut(|i| i.consume_shortcut(&OPEN_SHORTCUT)) {
            self.pick_and_open();
        }

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.iter().find_map(|f| f.path.clone()) {
            self.open_path(&path);
        }

        self.handle_nav_keys(&ctx);

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
            if dismiss {
                self.error = None;
            }
        }
    }
}
