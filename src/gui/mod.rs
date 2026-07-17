//! The graphical (egui/eframe) frontend. Owns an optional `Session` (none
//! until a file is opened) plus GUI-only view state.

use anyhow::{anyhow, Result};
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};

use crate::core::session::Session;

const OPEN_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);

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
}

impl GuiApp {
    fn new(mut session: Option<Session>) -> Self {
        let status = session.as_mut().and_then(Session::load_setup);
        Self {
            session,
            status,
            error: None,
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
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
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

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open…").clicked() {
                    self.pick_and_open();
                }
                ui.separator();
                ui.label(self.title());
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(self.status.as_deref().unwrap_or(""));
        });

        egui::CentralPanel::default().show(ui, |ui| match &self.session {
            Some(s) => {
                ui.label(format!(
                    "Loaded {} messages from {}.",
                    s.entries.len(),
                    s.path
                ));
                ui.label("Message list and plots coming soon.");
            }
            None => {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("Open a .tlog file to begin, or drag one onto this window.");
                        if ui.button("Open…").clicked() {
                            self.pick_and_open();
                        }
                    });
                });
            }
        });

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
