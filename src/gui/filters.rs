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
    /// Focus the window's first button on the next frame, so keyboard
    /// (Tab/Enter) navigation starts inside a freshly opened window.
    focus_on_open: bool,
}

impl FiltersState {
    /// Whether an "add"/"edit" editor is currently open (used by the
    /// layered Esc handler: cancel the editor before closing the window).
    pub fn has_editor(&self) -> bool {
        self.editor.is_some()
    }

    pub fn cancel_editor(&mut self) {
        self.editor = None;
    }

    /// Ask the window to grab keyboard focus when it is next shown.
    pub fn grab_focus(&mut self) {
        self.focus_on_open = true;
    }
}

/// Show the Filters window. `open` is cleared when the user clicks Close.
pub fn show(ctx: &egui::Context, open: &mut bool, session: &mut Session, state: &mut FiltersState) {
    if state.editor.is_none() && super::add_requested(ctx) {
        state.editor = Some(editor_for(session, None));
    }

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
                if state.editor.is_none() {
                    let add = ui
                        .button("Add filter")
                        .on_hover_text(super::hint(ctx, &super::ADD_SHORTCUT, "a"));
                    if state.focus_on_open {
                        add.request_focus();
                        state.focus_on_open = false;
                    }
                    if add.clicked() {
                        state.editor = Some(editor_for(session, None));
                    }
                }
                let close = ui.button("Close").on_hover_text("Esc");
                if state.focus_on_open {
                    close.request_focus();
                    state.focus_on_open = false;
                }
                if close.clicked() {
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
                        &session.filter_dropdown_labels(0),
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
                        &session.filter_dropdown_labels(1),
                        &mut editor.type_query,
                    ) {
                        editor.type_choice = choice;
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Save").on_hover_text("Enter").clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").on_hover_text("Esc").clicked() {
                        cancel = true;
                    }
                });
            }
            if state.editor.is_some()
                && ctx.input(|i| i.key_pressed(egui::Key::Enter))
                && ctx.memory(|m| m.focused().is_none())
            {
                save = true;
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
                // Editing a disabled filter must not silently re-enable it;
                // new filters start enabled.
                let enabled = editor
                    .index
                    .and_then(|i| session.filters.get(i))
                    .is_none_or(|f| f.enabled);
                let expr = FilterExpr {
                    sysid,
                    compid,
                    name,
                    exact: true,
                    enabled,
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
