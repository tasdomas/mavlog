//! The Filters window: view, add, edit and remove `Session::filters`.

use eframe::egui;

use crate::core::filter::FilterExpr;
use crate::core::session::Session;
use crate::gui::widgets::searchable_combo;

/// Open editor for a single filter expression (dropdown-based, like the TUI's
/// FilterEditor): pick an id (any, or a sysid:compid pair) and/or a type (any,
/// or an exact type name).
pub struct FilterEditor {
    /// Index of the filter being edited, or None when adding a new one.
    index: Option<usize>,
    /// 0 = any, otherwise 1 + index into `Session::id_options`.
    id_choice: usize,
    /// 0 = any, otherwise 1 + index into `Session::type_options`.
    type_choice: usize,
    id_query: String,
    type_query: String,
}

/// State for the Filters window; `None` on `GuiApp` means it's closed.
#[derive(Default)]
pub struct FiltersState {
    editor: Option<FilterEditor>,
}

/// Show the Filters window. `open` is cleared when the user clicks Close.
pub fn show(ctx: &egui::Context, open: &mut bool, session: &mut Session, state: &mut FiltersState) {
    egui::Window::new("Filters")
        .collapsible(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.label(format!(
                "{} of {} messages shown",
                session.filtered.len(),
                session.entries.len()
            ));
            ui.separator();

            let mut remove = None;
            let mut edit = None;
            for (i, filter) in session.filters.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(filter.to_text());
                    if ui.small_button("Edit").clicked() {
                        edit = Some(i);
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }

            if let Some(i) = remove {
                session.filters.remove(i);
                session.rebuild_filter_text();
                session.apply_filter();
                if state.editor.as_ref().is_some_and(|e| e.index == Some(i)) {
                    state.editor = None;
                }
            }
            if let Some(i) = edit {
                state.editor = Some(editor_for(session, Some(i)));
            }

            ui.separator();
            ui.horizontal(|ui| {
                if state.editor.is_none() && ui.button("Add filter").clicked() {
                    state.editor = Some(editor_for(session, None));
                }
                if ui.button("Close").clicked() {
                    *open = false;
                }
            });

            let mut save = false;
            let mut cancel = false;
            if let Some(editor) = &mut state.editor {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Id:");
                    let label = session.id_option_text(editor.id_choice);
                    if let Some(choice) = searchable_combo(
                        ui,
                        "filter_id",
                        &label,
                        &id_labels(session),
                        &mut editor.id_query,
                    ) {
                        editor.id_choice = choice;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Type:");
                    let label = session.type_option_text(editor.type_choice);
                    if let Some(choice) = searchable_combo(
                        ui,
                        "filter_type",
                        &label,
                        &type_labels(session),
                        &mut editor.type_query,
                    ) {
                        editor.type_choice = choice;
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            }
            if save {
                let editor = state.editor.take().unwrap();
                let (sysid, compid) = match editor.id_choice.checked_sub(1) {
                    Some(i) => {
                        let (s, c) = session.id_options[i];
                        (Some(s), Some(c))
                    }
                    None => (None, None),
                };
                let name = editor
                    .type_choice
                    .checked_sub(1)
                    .map(|i| session.type_options[i].to_ascii_lowercase());
                let expr = FilterExpr {
                    sysid,
                    compid,
                    name,
                    exact: true,
                };
                match editor.index {
                    Some(i) => session.filters[i] = expr,
                    None => session.filters.push(expr),
                }
                session.rebuild_filter_text();
                session.apply_filter();
            } else if cancel {
                state.editor = None;
            }
        });
}

fn editor_for(session: &Session, index: Option<usize>) -> FilterEditor {
    let expr = index.map(|i| &session.filters[i]);
    let id_choice = expr
        .and_then(|e| e.sysid.zip(e.compid))
        .and_then(|pair| session.id_options.iter().position(|&p| p == pair))
        .map_or(0, |i| i + 1);
    let type_choice = expr
        .and_then(|e| e.name.as_deref())
        .and_then(|n| session.type_options.iter().position(|t| t.eq_ignore_ascii_case(n)))
        .map_or(0, |i| i + 1);
    FilterEditor {
        index,
        id_choice,
        type_choice,
        id_query: String::new(),
        type_query: String::new(),
    }
}

fn id_labels(session: &Session) -> Vec<String> {
    (0..=session.id_options.len()).map(|i| session.id_option_text(i)).collect()
}

fn type_labels(session: &Session) -> Vec<String> {
    (0..=session.type_options.len()).map(|i| session.type_option_text(i)).collect()
}
