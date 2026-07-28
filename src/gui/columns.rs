//! The Columns window: view, add, edit and remove `Session::columns`.

use eframe::egui;

use crate::core::column::CustomColumn;
use crate::core::session::Session;
use crate::gui::widgets::{searchable_combo, source_choice, source_combo, source_from_choice};

/// Open editor for a single custom column (name/id/type/field), like the
/// TUI's ColumnEditor.
pub struct ColumnEditor {
    /// Index of the column being edited, or None when adding a new one.
    index: Option<usize>,
    name: String,
    /// 0 = any, otherwise 1 + index into `Session::id_options`.
    id_choice: usize,
    /// Index into `Session::type_options` (a type is required).
    type_choice: usize,
    field: String,
    /// 0 = any, 1 = primary (tlog), 2 = secondary (bin). Only used when merged.
    source_choice: usize,
    id_query: String,
    type_query: String,
    field_query: String,
}

/// State for the Columns window; `None` on `GuiApp` means it's closed.
#[derive(Default)]
pub struct ColumnsState {
    editor: Option<ColumnEditor>,
    /// Set when Save is pressed without a field chosen.
    error: Option<String>,
    /// Focus the window's first button on the next frame, so keyboard
    /// (Tab/Enter) navigation starts inside a freshly opened window.
    focus_on_open: bool,
}

impl ColumnsState {
    /// Whether an "add"/"edit" editor is currently open (used by the
    /// layered Esc handler: cancel the editor before closing the window).
    pub fn has_editor(&self) -> bool {
        self.editor.is_some()
    }

    pub fn cancel_editor(&mut self) {
        self.editor = None;
        self.error = None;
    }

    /// Ask the window to grab keyboard focus when it is next shown.
    pub fn grab_focus(&mut self) {
        self.focus_on_open = true;
    }
}

/// Show the Columns window. `open` is cleared when the user clicks Close.
pub fn show(ctx: &egui::Context, open: &mut bool, session: &mut Session, state: &mut ColumnsState) {
    if state.editor.is_none() && super::add_requested(ctx) {
        state.editor = Some(editor_for(session, None));
        state.error = None;
    }

    egui::Window::new("Columns")
        .collapsible(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            let mut remove = None;
            let mut edit = None;
            for (i, col) in session.columns.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(col.to_text());
                    if ui.small_button("Edit").clicked() {
                        edit = Some(i);
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }

            if let Some(i) = remove {
                let mut columns = std::mem::take(&mut session.columns);
                columns.remove(i);
                session.set_columns(columns);
                if state.editor.as_ref().is_some_and(|e| e.index == Some(i)) {
                    state.editor = None;
                }
            }
            if let Some(i) = edit {
                state.editor = Some(editor_for(session, Some(i)));
                state.error = None;
            }

            ui.separator();
            ui.horizontal(|ui| {
                if state.editor.is_none() {
                    let add = ui
                        .button("Add column")
                        .on_hover_text(super::hint(ctx, &super::ADD_SHORTCUT, "a"));
                    if state.focus_on_open {
                        add.request_focus();
                        state.focus_on_open = false;
                    }
                    if add.clicked() {
                        state.editor = Some(editor_for(session, None));
                        state.error = None;
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
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut editor.name);
                });
                ui.horizontal(|ui| {
                    ui.label("Id:");
                    let label = session.id_option_text(editor.id_choice);
                    if let Some(choice) = searchable_combo(
                        ui,
                        "column_id",
                        &label,
                        &session.column_dropdown_labels(1, editor.type_choice),
                        &mut editor.id_query,
                    ) {
                        editor.id_choice = choice;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Type:");
                    let label = session
                        .type_options
                        .get(editor.type_choice)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(choice) = searchable_combo(
                        ui,
                        "column_type",
                        &label,
                        &session.type_options,
                        &mut editor.type_query,
                    ) && choice != editor.type_choice
                    {
                        editor.type_choice = choice;
                        editor.field.clear(); // fields belong to a type; reset on change
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Field:");
                    let field_options = session.column_dropdown_labels(3, editor.type_choice);
                    let label = if editor.field.is_empty() { "(pick a field)" } else { &editor.field };
                    if let Some(choice) = searchable_combo(
                        ui,
                        "column_field",
                        label,
                        &field_options,
                        &mut editor.field_query,
                    ) {
                        editor.field = field_options[choice].clone();
                    }
                });
                source_combo(ui, "column_source", session, &mut editor.source_choice);
                if let Some(err) = &state.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
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
                let editor = state.editor.as_ref().unwrap();
                if editor.field.is_empty() {
                    state.error = Some("Pick a message field before saving".to_string());
                } else {
                    let editor = state.editor.take().unwrap();
                    let name = if editor.name.trim().is_empty() {
                        editor.field.clone()
                    } else {
                        editor.name.trim().to_string()
                    };
                    let (sysid, compid) = match editor.id_choice.checked_sub(1) {
                        Some(i) => {
                            let (s, c) = session.id_options[i];
                            (Some(s), Some(c))
                        }
                        None => (None, None),
                    };
                    let column = CustomColumn {
                        name,
                        sysid,
                        compid,
                        source: source_from_choice(editor.source_choice),
                        msg_type: session.type_options[editor.type_choice].clone(),
                        field: editor.field,
                        matches: Vec::new(),
                    };
                    let mut columns = std::mem::take(&mut session.columns);
                    match editor.index {
                        Some(i) => columns[i] = column,
                        None => columns.push(column),
                    }
                    session.set_columns(columns);
                    state.error = None;
                }
            } else if cancel {
                state.editor = None;
                state.error = None;
            }
        });
}

fn editor_for(session: &Session, index: Option<usize>) -> ColumnEditor {
    let col = index.map(|i| &session.columns[i]);
    ColumnEditor {
        index,
        name: col.map(|c| c.name.clone()).unwrap_or_default(),
        id_choice: col
            .and_then(|c| c.sysid.zip(c.compid))
            .and_then(|pair| session.id_options.iter().position(|&p| p == pair))
            .map_or(0, |i| i + 1),
        type_choice: col
            .and_then(|c| session.type_options.iter().position(|t| t.eq_ignore_ascii_case(&c.msg_type)))
            .unwrap_or(0),
        field: col.map(|c| c.field.clone()).unwrap_or_default(),
        source_choice: source_choice(col.and_then(|c| c.source)),
        id_query: String::new(),
        type_query: String::new(),
        field_query: String::new(),
    }
}
