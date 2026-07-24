//! The graphical (egui/eframe) frontend. Owns an optional `Session` (none
//! until a file is opened) plus GUI-only view state.

mod columns;
mod filters;
#[cfg(target_os = "macos")]
mod macos;
mod plots;
mod widgets;

use anyhow::{anyhow, Result};
use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Modifiers};
use egui_extras::{Column, TableBuilder};

use crate::core::session::Session;
use crate::core::time::{format_datetime, format_offset, parse_jump, TimeFormat};
use crate::tlog;

const OPEN_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
/// Ctrl+Q quit shortcut for non-macOS platforms. On macOS, Cmd+Q is owned by
/// the application menu; `gui::macos` repoints that menu item at the window
/// close request so it hits the same unsaved-changes guard, and egui never
/// sees the key. Elsewhere there is no such menu, so we handle the key here.
const QUIT_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const SAVE_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const JUMP_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::J);
const FILTERS_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
const COLUMNS_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::C);
const PLOTS_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::P);
const SETTINGS_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma);
const HELP_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F1);
/// The modifier half of `add_requested` (the Columns window also accepts a
/// plain `a`), consumed while that window is open. Filters open their add
/// popup straight from the `f` shortcut, and plots use their own `p`.
const ADD_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const MARK_BG: Color32 = Color32::from_rgb(140, 20, 20);

/// A context-menu action on a message row, applied after the table body
/// closure (which only borrows `Session`) finishes.
enum MarkAction {
    Add,
    Remove,
    EditLabel,
}

/// A destructive action deferred while the unsaved-changes prompt is up:
/// replacing the current session with another file, or quitting.
enum PendingAction {
    Open(std::path::PathBuf),
    Quit,
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
    filters_state: filters::FiltersState,
    /// Whether the Columns window is open.
    columns_open: bool,
    columns_state: columns::ColumnsState,
    plots_state: plots::PlotsState,
    /// Whether the Help window is open.
    help_open: bool,
    /// Which right-column blocks are collapsed to just their header. Each is
    /// toggled by clicking that block's header, which stays visible while
    /// collapsed so the block can be reopened. The message-contents detail
    /// pane above them is never collapsible.
    filters_collapsed: bool,
    plots_collapsed: bool,
    marks_collapsed: bool,
    /// Set by the jump-focus shortcut; the toolbar requests focus on the
    /// jump box next time it's drawn, then clears this.
    focus_jump: bool,
    /// Whether a *text box* had keyboard focus at the end of the previous
    /// frame. `Esc` uses this instead of a live `ctx.memory` query: egui's
    /// own input processing already blurs a focused widget on `Esc` before
    /// our code runs each frame, so a live query can never tell "something
    /// was just blurred by this same keypress" apart from "nothing was ever
    /// focused" — both read as unfocused by the time we look. Only text
    /// boxes count: for them Esc-to-unfocus is a meaningful step the user
    /// took on purpose, while a focused *button* (e.g. the focus a popup
    /// grabs when it opens) shouldn't cost an extra Esc press to get past.
    text_focused_last_frame: bool,
    /// Focus the Settings window's first control when it next opens.
    settings_focus: bool,
    /// Focus the label window's text box when it next opens.
    label_focus: bool,
    /// Whether Tab was pressed last frame; `keep_focus_in_popups` only
    /// reacts to focus that *Tab* moved, never focus the user clicked
    /// somewhere on purpose.
    tab_pressed_last_frame: bool,
    /// A file-open or quit awaiting the user's answer to the unsaved-changes
    /// prompt. While set, the prompt is shown and normal key shortcuts pause.
    pending: Option<PendingAction>,
    /// Set once the user has approved quitting with unsaved changes, so the
    /// re-issued close request is allowed through instead of re-prompting.
    allow_close: bool,
    /// macOS: whether the menu's Quit item has been repointed at the window
    /// close request yet (done once, on the first frame the menu exists).
    #[cfg(target_os = "macos")]
    menu_quit_rerouted: bool,
}

/// Whether the currently focused widget (if any) is a text box. Used to gate
/// the plain-letter shortcuts: they must not fire while the user is typing,
/// but focus on a button (e.g. after a popup grabs focus on open, precisely
/// so Tab works inside it) should not disable them.
fn text_input_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

/// Did the user ask to add an item — Ctrl/Cmd+N, or a plain `a` when not
/// typing? Both consume the keypress, so the first open popup that checks
/// each frame wins (the fixed call order in `GuiApp::ui` is the tie-break).
fn add_requested(ctx: &egui::Context) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&ADD_SHORTCUT))
        || (!text_input_focused(ctx)
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::A)))
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
            filters_state: filters::FiltersState::default(),
            columns_open: false,
            columns_state: columns::ColumnsState::default(),
            plots_state: plots::PlotsState::default(),
            help_open: false,
            filters_collapsed: false,
            plots_collapsed: false,
            marks_collapsed: false,
            focus_jump: false,
            text_focused_last_frame: false,
            settings_focus: false,
            label_focus: false,
            tab_pressed_last_frame: false,
            pending: None,
            allow_close: false,
            #[cfg(target_os = "macos")]
            menu_quit_rerouted: false,
        }
    }

    /// Open the add-filter popup (grabbing keyboard focus so Tab/Enter start
    /// inside it). The side block listing filters shows itself based on
    /// whether the session has any, so there is no panel to toggle.
    fn open_new_filter(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        self.filters_state.open_add(session);
    }

    /// Open (or toggle) a popup, letting it grab keyboard focus when it
    /// opens so Tab navigation starts inside it.
    fn open_columns(&mut self) {
        self.columns_open = true;
        self.columns_state.grab_focus();
    }

    fn toggle_columns(&mut self) {
        self.columns_open = !self.columns_open;
        if self.columns_open {
            self.columns_state.grab_focus();
        }
    }

    fn open_settings(&mut self) {
        self.settings_open = true;
        self.settings_focus = true;
    }

    fn toggle_settings(&mut self) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.settings_focus = true;
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
            self.request_open(path);
        }
    }

    /// Open `path`, but first guard the current session's unsaved changes: if
    /// there are any, stash the request and let the prompt decide.
    fn request_open(&mut self, path: std::path::PathBuf) {
        if self.session.as_ref().is_some_and(Session::is_dirty) {
            self.pending = Some(PendingAction::Open(path));
        } else {
            self.open_path(&path);
        }
    }

    /// Quit, guarding the current session's unsaved changes: if there are any,
    /// raise the prompt; otherwise close the window straight away.
    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.session.as_ref().is_some_and(Session::is_dirty) {
            self.pending = Some(PendingAction::Quit);
        } else {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Carry out the deferred action once the user has chosen to proceed
    /// (either after saving or after discarding). Clears `pending`.
    fn resolve_pending(&mut self, ctx: &egui::Context) {
        match self.pending.take() {
            Some(PendingAction::Open(path)) => self.open_path(&path),
            Some(PendingAction::Quit) => {
                // Approve the close and re-issue it; next frame's
                // close_requested is let through by `allow_close`.
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
    }

    /// The unsaved-changes prompt shown before opening another file or quitting
    /// while the setup has unsaved edits. Offers Save / Discard / Cancel.
    fn unsaved_changes_dialog(&mut self, ctx: &egui::Context) {
        let verb = match self.pending {
            Some(PendingAction::Open(_)) => "opening another file",
            Some(PendingAction::Quit) => "quitting",
            None => return,
        };

        enum Choice {
            Save,
            Discard,
            Cancel,
        }
        let mut choice = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "You have unsaved changes to the setup.\nSave before {verb}?"
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(Choice::Save);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(Choice::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(Choice::Cancel);
                    }
                });
            });
        // Esc is the safe (non-destructive) way out of the prompt.
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            choice = Some(Choice::Cancel);
        }

        match choice {
            None => {}
            Some(Choice::Cancel) => self.pending = None,
            Some(Choice::Discard) => self.resolve_pending(ctx),
            Some(Choice::Save) => {
                self.save_setup();
                // Proceed only if the save actually landed (session now clean);
                // otherwise abort the action and leave the error in the status
                // bar rather than silently losing the changes.
                if self.session.as_ref().map_or(true, |s| !s.is_dirty()) {
                    self.resolve_pending(ctx);
                } else {
                    self.pending = None;
                }
            }
        }
    }

    /// Read, parse and load a tlog file, replacing the current session on
    /// success or setting `self.error` on failure.
    fn open_path(&mut self, path: &std::path::Path) {
        match crate::load_session(&path.to_string_lossy()) {
            Ok(mut session) => {
                self.status = session.load_setup();
                self.filters_state.cancel_editor();
                self.session = Some(session);
                self.error = None;
                self.scroll_to_selected = true;
                // The plots cache is keyed by index into the old file's plots.
                self.plots_state.reset();
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
        self.label_focus = true;
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
                if self.label_focus {
                    response.request_focus();
                    self.label_focus = false;
                }
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
        let Some(session) = &mut self.session else {
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
        if ctx.input_mut(|i| i.consume_shortcut(&QUIT_SHORTCUT)) {
            self.request_quit(ctx);
        }
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
            self.open_new_filter();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&COLUMNS_SHORTCUT)) {
            self.toggle_columns();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&PLOTS_SHORTCUT))
            && let Some(session) = &self.session
        {
            self.plots_state.add_plot(session);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SETTINGS_SHORTCUT)) {
            self.toggle_settings();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&HELP_SHORTCUT)) {
            self.help_open = !self.help_open;
        }

        // Plain letters are suppressed only while typing in a text box —
        // focus on a button (a just-opened popup grabs one) keeps them live.
        if text_input_focused(ctx) {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Questionmark)) {
            self.help_open = !self.help_open;
        }
        if ctx.input(|i| i.key_pressed(Key::O)) {
            self.pick_and_open();
        }
        if self.session.is_none() {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::W)) {
            self.save_setup();
        }
        if ctx.input(|i| i.key_pressed(Key::T)) {
            self.focus_jump = true;
        }
        if ctx.input(|i| i.key_pressed(Key::F)) {
            self.open_new_filter();
        }
        if ctx.input(|i| i.key_pressed(Key::C)) {
            self.toggle_columns();
        }
        if ctx.input(|i| i.key_pressed(Key::P)) && let Some(session) = &self.session {
            self.plots_state.add_plot(session);
        }
        if ctx.input(|i| i.key_pressed(Key::S)) {
            self.toggle_settings();
        }
    }

    /// Handle list-navigation keys, unless *any* widget has focus — unlike
    /// the plain-letter shortcuts, these stay gated on button focus too,
    /// because egui itself uses arrows/Space to navigate and activate
    /// focused widgets and both firing at once would be chaos.
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
    }

    /// Esc closes whichever window is open, in a fixed precedence (most
    /// transient first) since egui doesn't track window stacking order for
    /// us. One press closes at most one window: it cancels an open popup
    /// editor before closing that popup's window.
    fn handle_escape(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.key_pressed(Key::Escape)) {
            return;
        }
        if self.text_focused_last_frame {
            // egui already blurred the focused text box for us this frame;
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
        } else if self.filters_state.has_editor() {
            self.filters_state.cancel_editor();
        } else if self.columns_open && self.columns_state.has_editor() {
            self.columns_state.cancel_editor();
        } else if self.columns_open {
            self.columns_open = false;
        } else if self.plots_state.has_editor() {
            self.plots_state.cancel_editor();
        }
    }

    /// Keep Tab traversal inside an open popup. egui walks focus through
    /// every focusable widget in creation order across the whole UI, so
    /// tabbing past a popup's last button lands on the main window's
    /// toolbar (and then hundreds of table cells). If last frame's Tab
    /// moved focus onto a background-layer widget while a popup is open,
    /// pull it back to the top-most popup's entry button. Only Tab-moved
    /// focus is corrected — a deliberate click on a background widget
    /// (e.g. into the jump box) must keep the focus it asked for. Popups
    /// live in non-background layers, so tabbing between two open popups
    /// stays allowed.
    fn keep_focus_in_popups(&mut self, ctx: &egui::Context) {
        if !self.tab_pressed_last_frame {
            return;
        }
        let escaped = ctx
            .memory(|m| m.focused())
            .and_then(|id| ctx.read_response(id))
            .is_some_and(|r| r.layer_id == egui::LayerId::background());
        if !escaped {
            return;
        }
        // The plots sidebar and the filters side block are intentionally
        // absent here: both are docked panels, themselves in the background
        // layer, so Tab moving between their own controls would read as
        // "escaped" and bounce focus back every press. The plots and filters
        // *editor dialogs* below are floating windows, so they are contained
        // like the other popups.
        if self.label_prompt.is_some() {
            self.label_focus = true;
        } else if self.filters_state.has_editor() {
            self.filters_state.grab_focus();
        } else if self.plots_state.has_editor() {
            self.plots_state.grab_editor_focus();
        } else if self.columns_open {
            self.columns_state.grab_focus();
        } else if self.settings_open {
            self.settings_focus = true;
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
                    .button("Add filter")
                    .on_hover_text(hint(&ctx, &FILTERS_SHORTCUT, "f"))
                    .clicked()
                {
                    self.open_new_filter();
                }
                if ui
                    .button("Columns")
                    .on_hover_text(hint(&ctx, &COLUMNS_SHORTCUT, "c"))
                    .clicked()
                {
                    self.open_columns();
                }
                if ui
                    .button("Add plot")
                    .on_hover_text(hint(&ctx, &PLOTS_SHORTCUT, "p"))
                    .clicked()
                    && let Some(session) = &self.session
                {
                    self.plots_state.add_plot(session);
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
                    self.open_settings();
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
                    ("Ctrl/Cmd+F or f", "Add a new filter"),
                    ("Ctrl/Cmd+Shift+C or c", "Toggle the Columns window"),
                    ("Ctrl/Cmd+P or p", "Add a new plot"),
                    ("Ctrl/Cmd+, or s", "Toggle Settings"),
                    ("Ctrl/Cmd+Q", "Quit (warns on unsaved changes)"),
                    ("F1 or ?", "Toggle this Help window"),
                    ("Ctrl/Cmd+N or a", "Add a column (in the Columns window)"),
                    ("Tab / Shift+Tab", "Move focus between a window's controls"),
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
                ui.label("Plain letters only fire while no text box has focus.");
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Mouse").strong());
                ui.add_space(4.0);
                for (action, desc) in [
                    ("Click a row", "Select that message"),
                    ("Right-click a row", "Add/edit/remove its mark"),
                    ("Click a side block's header", "Collapse or expand that block"),
                    ("Double-click a mark (side pane)", "Jump the list to that message"),
                    ("Right-click a mark (side pane)", "Edit label / remove mark"),
                    ("Drag a file onto the window", "Open it"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(action).strong());
                        ui.label(desc);
                    });
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Plot windows").strong());
                ui.add_space(4.0);
                for (action, desc) in [
                    ("Left-drag", "Pan the view"),
                    ("Right-drag", "Box-zoom into a rectangle"),
                    ("Scroll / pinch", "Zoom (drag on an axis to zoom just that axis)"),
                    ("Double-click / Reset view", "Restore auto bounds"),
                    ("Export PNG…", "Save the plot area to an image file"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(action).strong());
                        ui.label(desc);
                    });
                }
                ui.add_space(4.0);
                ui.label("Series sharing a measurement unit share one Y axis; other units get their own.");
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
                let first =
                    ui.radio_value(&mut session.time_format, TimeFormat::DateTime, "Date-time");
                if self.settings_focus {
                    first.request_focus();
                    self.settings_focus = false;
                }
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

        // Rows are click-to-select; selectable label text would fight that
        // with an I-beam cursor and drag-selection. Copyable text lives in
        // the detail pane.
        ui.style_mut().interaction.selectable_labels = false;

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

    /// The pane listing every mark in file order, shown in the bottom of the
    /// right column beneath the detail view. Single-click selects
    /// (without scrolling the list); double-click also scrolls the list to
    /// the row; right-click offers edit/remove, mirroring the table's menu.
    /// Only shown when at least one mark exists (see `GuiApp::ui`).
    fn marks_panel(&mut self, ui: &mut egui::Ui) {
        let Some(session) = &self.session else {
            return;
        };
        ui.separator();

        let mut marks: Vec<(usize, String)> = session
            .marks
            .iter()
            .map(|(&i, l)| (i, l.clone()))
            .collect();
        marks.sort_by_key(|(i, _)| *i);
        let current = session.selected_entry_index();

        let mut select_for = None;
        let mut scroll = false;
        let mut mark_action: Option<(usize, MarkAction)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (entry_index, label) in &marks {
                    let entry = &session.entries[*entry_index];
                    let time = session.format_list_time(entry.timestamp_us);
                    let text = if label.is_empty() {
                        format!("● {time}  {}", entry.name)
                    } else {
                        format!("● {time}  {}\n   {label}", entry.name)
                    };
                    let is_current = current == Some(*entry_index);
                    let response = ui.selectable_label(is_current, text);
                    if response.clicked() {
                        select_for = Some(*entry_index);
                    }
                    if response.double_clicked() {
                        select_for = Some(*entry_index);
                        scroll = true;
                    }
                    response.context_menu(|ui| {
                        if ui.button("Edit label").clicked() {
                            mark_action = Some((*entry_index, MarkAction::EditLabel));
                            ui.close();
                        }
                        if ui.button("Remove mark").clicked() {
                            mark_action = Some((*entry_index, MarkAction::Remove));
                            ui.close();
                        }
                    });
                }
            });

        if let Some(entry_index) = select_for {
            if let Some(session) = &mut self.session {
                session.select_entry(entry_index);
            }
        }
        if scroll {
            self.scroll_to_selected = true;
        }
        if let Some((entry_index, action)) = mark_action {
            match action {
                MarkAction::Remove => {
                    if let Some(session) = &mut self.session {
                        session.marks.remove(&entry_index);
                    }
                }
                MarkAction::EditLabel => self.open_label_editor(entry_index),
                MarkAction::Add => {}
            }
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

/// Draw a docked right-column block's clickable header — a disclosure triangle
/// plus its heading-sized title — and return whether it was clicked this frame
/// (a request to toggle collapse). The header stays visible while the block is
/// collapsed, so clicking it again reopens the block; `collapsed` only chooses
/// which triangle to show.
fn panel_header(ui: &mut egui::Ui, title: &str, collapsed: bool) -> bool {
    let triangle = if collapsed { "▶" } else { "▼" };
    ui.selectable_label(
        false,
        egui::RichText::new(format!("{triangle} {title}")).heading(),
    )
    .on_hover_text(if collapsed { "Expand" } else { "Collapse" })
    .clicked()
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

        // Route the macOS menu's Cmd+Q through the window close request so the
        // unsaved-changes guard applies. The menu only exists once the app has
        // finished launching, so keep trying until it takes.
        #[cfg(target_os = "macos")]
        if !self.menu_quit_rerouted {
            self.menu_quit_rerouted = macos::reroute_menu_quit_to_close();
        }

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.iter().find_map(|f| f.path.clone()) {
            self.request_open(path);
        }

        // Guard the window close against unsaved changes: cancel the close and
        // raise the prompt. Once the user approves (or the setup is clean),
        // `allow_close` lets the re-issued close through.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.session.as_ref().is_some_and(Session::is_dirty)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending.is_none() {
                self.pending = Some(PendingAction::Quit);
            }
        }

        // With no file open the filters UI can't be shown; drop any stale
        // editor so a later Esc isn't silently swallowed by it.
        if self.session.is_none() {
            self.filters_state.cancel_editor();
        }

        // While the unsaved-changes prompt is up it owns the keyboard (Esc/
        // buttons); pausing the normal shortcuts keeps them from acting behind
        // it. Modifier shortcuts must be consumed before the plain-letter
        // checks in handle_nav_keys so a Cmd-modified press can't also satisfy
        // the unmodified check for the same key (see handle_toolbar_shortcuts).
        if self.pending.is_none() {
            self.handle_toolbar_shortcuts(&ctx);
            self.handle_nav_keys(&ctx);
            self.handle_escape(&ctx);
        }
        self.keep_focus_in_popups(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(self.status.as_deref().unwrap_or(""));
        });

        if self.session.is_some() {
            egui::Panel::right("detail").default_size(420.0).show(ui, |ui| {
                // The marks list, plots sidebar and filters block all sit under
                // the message contents view, in the same right column. Bottom
                // panels are added outermost-first, so marks takes the very
                // bottom, then plots, then filters directly beneath the detail
                // pane, which fills the space above them. Each is shown only
                // when it has content; the add/edit filter popup is a floating
                // window (drawn below), so a filter can be added from empty.
                //
                // Each block can be collapsed to just its header by clicking it.
                // A collapsed block drops its resizable body and hugs the header
                // height (bottom panels with no default size size to content),
                // so only the expanded blocks share the leftover space.
                if self.session.as_ref().is_some_and(|s| !s.marks.is_empty()) {
                    let collapsed = self.marks_collapsed;
                    let mut panel = egui::Panel::bottom("marks");
                    if !collapsed {
                        panel = panel.resizable(true).default_size(220.0);
                    }
                    let mut toggled = false;
                    panel.show(ui, |ui| {
                        toggled = panel_header(ui, "Marks", collapsed);
                        if !collapsed {
                            self.marks_panel(ui);
                        }
                    });
                    self.marks_collapsed ^= toggled;
                }
                if self.session.as_ref().is_some_and(|s| !s.plots.is_empty()) {
                    let collapsed = self.plots_collapsed;
                    let mut panel = egui::Panel::bottom("plots");
                    if !collapsed {
                        panel = panel.resizable(true).default_size(240.0);
                    }
                    let mut toggled = false;
                    panel.show(ui, |ui| {
                        toggled = panel_header(ui, "Plots", collapsed);
                        if !collapsed && let Some(session) = &mut self.session {
                            plots::show_sidebar(ui, session, &mut self.plots_state);
                        }
                    });
                    self.plots_collapsed ^= toggled;
                }
                if self.session.as_ref().is_some_and(|s| !s.filters.is_empty()) {
                    let collapsed = self.filters_collapsed;
                    let mut panel = egui::Panel::bottom("filters");
                    if !collapsed {
                        panel = panel.resizable(true).default_size(200.0);
                    }
                    let mut toggled = false;
                    panel.show(ui, |ui| {
                        toggled = panel_header(ui, "Filters", collapsed);
                        if !collapsed && let Some(session) = &mut self.session {
                            filters::panel(ui, session, &mut self.filters_state);
                        }
                    });
                    self.filters_collapsed ^= toggled;
                }
                self.detail_panel(ui);
            });
        }

        egui::CentralPanel::default().show(ui, |ui| self.list_panel(ui));

        self.settings_window(&ctx);
        self.label_window(&ctx);
        self.help_window(&ctx);
        self.unsaved_changes_dialog(&ctx);
        if self.columns_open {
            if let Some(session) = &mut self.session {
                columns::show(&ctx, &mut self.columns_open, session, &mut self.columns_state);
            } else {
                self.columns_open = false;
            }
        }
        if let Some(session) = &mut self.session {
            filters::show_editor(&ctx, session, &mut self.filters_state);
            plots::show_editor(&ctx, session, &mut self.plots_state);
            plots::show_open_plots(&ctx, session, &mut self.plots_state);
        }
        // A pending plot export completes when its screenshot arrives (a frame
        // or two after the request), independent of whether a session is open.
        if let Some(msg) = plots::handle_export(&ctx, &mut self.plots_state) {
            self.status = Some(msg);
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

        // Captured for next frame's handle_escape / keep_focus_in_popups —
        // see text_focused_last_frame's doc comment for why a live query
        // there can't do this job.
        self.text_focused_last_frame = text_input_focused(&ctx);
        self.tab_pressed_last_frame = ctx.input(|i| i.key_pressed(Key::Tab));
    }
}
