//! Small reusable GUI widgets shared by the filter and column editors.

use eframe::egui;

use crate::core::entry::LogSourceId;
use crate::core::filter::match_labels;
use crate::core::session::Session;

/// A "Source:" dropdown for merged sessions (any / the two logs), shown only
/// when `session.is_merged()`. `choice` is 0 = any, 1 = primary, 2 = secondary.
/// Returns the picked `Option<LogSourceId>` (mapped from `choice`).
pub fn source_combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    session: &Session,
    choice: &mut usize,
) -> Option<LogSourceId> {
    if session.is_merged() {
        let labels = [
            "any".to_string(),
            session.source_name(LogSourceId::Primary).unwrap_or("tlog").to_string(),
            session.source_name(LogSourceId::Secondary).unwrap_or("bin").to_string(),
        ];
        ui.horizontal(|ui| {
            ui.label("Source:");
            egui::ComboBox::from_id_salt(id_salt)
                .selected_text(&labels[(*choice).min(2)])
                .show_ui(ui, |ui| {
                    for (i, label) in labels.iter().enumerate() {
                        ui.selectable_value(choice, i, label);
                    }
                });
        });
    }
    source_from_choice(*choice)
}

/// Map a 0=any / 1=primary / 2=secondary dropdown choice to a source filter.
pub fn source_from_choice(choice: usize) -> Option<LogSourceId> {
    match choice {
        1 => Some(LogSourceId::Primary),
        2 => Some(LogSourceId::Secondary),
        _ => None,
    }
}

/// The dropdown choice (0/1/2) representing an optional source.
pub fn source_choice(source: Option<LogSourceId>) -> usize {
    match source {
        None => 0,
        Some(LogSourceId::Primary) => 1,
        Some(LogSourceId::Secondary) => 2,
    }
}

/// A combo box with a type-to-filter search box in its popup, mirroring the
/// TUI's dropdown-with-autocomplete. `query` is the editor's own persistent
/// search buffer for this field, cleared whenever an option is picked.
/// Returns `Some(index into options)` on selection.
pub fn searchable_combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    selected_label: &str,
    options: &[String],
    query: &mut String,
) -> Option<usize> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected_label)
        // The default CloseOnClick closes the popup on *any* click inside
        // it — including on the search box. Keep it open on inner clicks
        // and close explicitly (`ui.close()`) when an option is picked.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(query).hint_text("Search…"));
            ui.separator();
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for i in match_labels(options, query) {
                    if ui.selectable_label(false, &options[i]).clicked() {
                        picked = Some(i);
                        ui.close();
                    }
                }
            });
        });
    if picked.is_some() {
        query.clear();
    }
    picked
}
