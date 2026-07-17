//! The graphical (egui/eframe) frontend. Owns an optional `Session` (none
//! until a file is opened) plus GUI-only view state.

use anyhow::{anyhow, Result};
use eframe::egui;

use crate::core::session::Session;

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
}

impl GuiApp {
    fn new(mut session: Option<Session>) -> Self {
        let status = session.as_mut().and_then(Session::load_setup);
        Self { session, status }
    }

    fn title(&self) -> String {
        match &self.session {
            Some(s) => format!("{}  —  {} messages", s.path, s.entries.len()),
            None => "No file open".to_string(),
        }
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let _ = ui.button("Open…"); // wired to a file dialog next
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
                    ui.label("Open a .tlog file to begin.");
                });
            }
        });
    }
}
