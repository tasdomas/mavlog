//! The Filters side block (lists `Session::filters`) plus the add/edit filter
//! popup. The side block is shown whenever the session has any configured
//! filters; the popup opens on demand to add or edit one.

use eframe::egui;

use crate::core::filter::FilterExpr;
use crate::core::session::Session;
use crate::gui::widgets::{searchable_combo, source_choice, source_combo};

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
    /// 0 = any, 1 = primary (tlog), 2 = secondary (bin). Only used when merged.
    source_choice: usize,
    id_query: String,
    type_query: String,
}

/// State for the Filters UI: the optional add/edit popup. The side block's
/// visibility is derived from `Session::filters`, so it needs no open flag.
#[derive(Default)]
pub struct FiltersState {
    editor: Option<FilterEditor>,
    /// Focus the popup's Save button on the next frame, so keyboard
    /// (Tab/Enter) navigation starts inside a freshly opened popup.
    focus_on_open: bool,
}

impl FiltersState {
    /// Whether the add/edit popup is open (used by the layered Esc handler:
    /// cancel the editor before closing anything else).
    pub fn has_editor(&self) -> bool {
        self.editor.is_some()
    }

    pub fn cancel_editor(&mut self) {
        self.editor = None;
    }

    /// Open the popup to add a new filter, and grab keyboard focus for it.
    pub fn open_add(&mut self, session: &Session) {
        self.editor = Some(editor_for(session, None));
        self.focus_on_open = true;
    }

    /// Ask the popup to grab keyboard focus when it is next shown.
    pub fn grab_focus(&mut self) {
        self.focus_on_open = true;
    }
}

/// Body of the Filters side block. Only shown when the session has at least
/// one configured filter (see `GuiApp::ui`), so it never needs to bootstrap
/// from empty — the toolbar/shortcut open the add popup for that.
pub fn panel(ui: &mut egui::Ui, session: &mut Session, state: &mut FiltersState) {
    let ctx = ui.ctx().clone();

    ui.label(format!(
        "{} of {} messages shown",
        session.filtered.len(),
        session.entries.len()
    ));
    ui.separator();

    let mut remove = None;
    let mut edit = None;
    let mut toggled = false;
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, filter) in session.filters.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    // The checkbox conveys the enabled state, so drop the '!'
                    // prefix from the displayed label.
                    let text = filter.to_text();
                    let label = text.strip_prefix('!').unwrap_or(&text);
                    if ui.checkbox(&mut filter.enabled, label).changed() {
                        toggled = true;
                    }
                    if ui.small_button("Edit").clicked() {
                        edit = Some(i);
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
        });

    if toggled {
        session.rebuild_filter_text();
        session.apply_filter();
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
        state.focus_on_open = true;
    }

    ui.separator();
    if ui
        .button("Add filter")
        .on_hover_text(super::hint(&ctx, &super::FILTERS_SHORTCUT, "f"))
        .clicked()
    {
        state.editor = Some(editor_for(session, None));
        state.focus_on_open = true;
    }
}

/// The add/edit filter popup, drawn as a floating window whenever an editor is
/// open. Kept separate from the side block so a new filter can be added even
/// when the block is hidden (no filters yet).
pub fn show_editor(ctx: &egui::Context, session: &mut Session, state: &mut FiltersState) {
    let Some(is_edit) = state.editor.as_ref().map(|e| e.index.is_some()) else {
        return;
    };

    let mut save = false;
    let mut cancel = false;
    egui::Window::new(if is_edit { "Edit filter" } else { "New filter" })
        .collapsible(false)
        .resizable(false)
        .default_width(320.0)
        .show(ctx, |ui| {
            let editor = state.editor.as_mut().unwrap();
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
            source_combo(ui, "filter_source", session, &mut editor.source_choice);
            ui.separator();
            ui.horizontal(|ui| {
                let save_button = ui.button("Save").on_hover_text("Enter");
                if state.focus_on_open {
                    save_button.request_focus();
                    state.focus_on_open = false;
                }
                if save_button.clicked() {
                    save = true;
                }
                if ui.button("Cancel").on_hover_text("Esc").clicked() {
                    cancel = true;
                }
            });
        });

    if !save && ctx.input(|i| i.key_pressed(egui::Key::Enter)) && ctx.memory(|m| m.focused().is_none())
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
            source: crate::gui::widgets::source_from_choice(editor.source_choice),
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
        source_choice: source_choice(expr.and_then(|e| e.source)),
        id_query: String::new(),
        type_query: String::new(),
    }
}
