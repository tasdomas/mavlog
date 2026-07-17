//! Small reusable GUI widgets shared by the filter and column editors.

use eframe::egui;

use crate::core::filter::match_labels;

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
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(query).hint_text("Search…"));
            ui.separator();
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for i in match_labels(options, query) {
                    if ui.selectable_label(false, &options[i]).clicked() {
                        picked = Some(i);
                    }
                }
            });
        });
    if picked.is_some() {
        query.clear();
    }
    picked
}
