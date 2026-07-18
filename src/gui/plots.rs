//! The Plots manager window (create/edit/remove `Session::plots`, toggle
//! which ones are open) and the rendering of each open plot as its own
//! `egui_plot::Plot` window — the motivating feature for the whole GUI.

use std::collections::{HashMap, HashSet};

use eframe::egui;
use egui_plot::{Legend, Line, LineStyle, PlotPoints, VLine};

use crate::core::plot::{self, PlotDef, SeriesDef};
use crate::core::session::Session;
use crate::core::time::{format_datetime, format_offset, TimeFormat};
use crate::gui::widgets::searchable_combo;

/// One series row in the plot editor.
struct SeriesEditor {
    /// 0 = any, otherwise 1 + index into `Session::id_options`.
    id_choice: usize,
    /// Index into `Session::type_options` (a type is required, like columns).
    type_choice: usize,
    field: String,
    id_query: String,
    type_query: String,
    field_query: String,
}

struct PlotEditor {
    /// Index into `Session::plots` being edited, or None when adding new.
    index: Option<usize>,
    name: String,
    series: Vec<SeriesEditor>,
}

/// State for the Plots manager and its open plot windows; owned by `GuiApp`.
#[derive(Default)]
pub struct PlotsState {
    manager_open: bool,
    /// Focus the manager's first button on the next frame, so keyboard
    /// (Tab/Enter) navigation starts inside a freshly opened window.
    focus_on_open: bool,
    editor: Option<PlotEditor>,
    error: Option<String>,
    /// Indices into `Session::plots` currently shown as plot windows.
    open: HashSet<usize>,
    /// Extracted (and decimated) points per open plot's series, indexed like
    /// `open`. `Session`'s underlying tlog data never changes once loaded, so
    /// this is computed once per show/edit rather than every frame — without
    /// it, every open plot window would re-decode all of its matching
    /// entries ~60 times a second.
    cache: HashMap<usize, Vec<Vec<[f64; 2]>>>,
}

impl PlotsState {
    pub fn open_manager(&mut self) {
        self.manager_open = true;
        self.focus_on_open = true;
    }

    /// Whether the manager window is open (used by the Esc-to-close audit).
    pub fn is_manager_open(&self) -> bool {
        self.manager_open
    }

    pub fn close_manager(&mut self) {
        self.manager_open = false;
    }

    pub fn toggle_manager(&mut self) {
        self.manager_open = !self.manager_open;
        if self.manager_open {
            self.focus_on_open = true;
        }
    }

    /// Ask the manager window to grab keyboard focus when it is next shown.
    pub fn grab_focus(&mut self) {
        self.focus_on_open = true;
    }

    /// Whether an "add"/"edit" plot editor is currently open (used by the
    /// layered Esc handler: cancel the editor before closing the manager).
    pub fn has_editor(&self) -> bool {
        self.editor.is_some()
    }

    pub fn cancel_editor(&mut self) {
        self.editor = None;
        self.error = None;
    }

    fn extract(session: &Session, plot_def: &PlotDef) -> Vec<Vec<[f64; 2]>> {
        plot_def.series.iter().map(|s| plot::extract(session, s)).collect()
    }
}

fn empty_series() -> SeriesEditor {
    SeriesEditor {
        id_choice: 0,
        type_choice: 0,
        field: String::new(),
        id_query: String::new(),
        type_query: String::new(),
        field_query: String::new(),
    }
}

fn editor_for(session: &Session, index: Option<usize>) -> PlotEditor {
    let plot = index.map(|i| &session.plots[i]);
    let series = plot.map_or_else(
        || vec![empty_series()],
        |p| {
            p.series
                .iter()
                .map(|s| SeriesEditor {
                    id_choice: s
                        .sysid
                        .zip(s.compid)
                        .and_then(|pair| session.id_options.iter().position(|&p| p == pair))
                        .map_or(0, |i| i + 1),
                    type_choice: session
                        .type_options
                        .iter()
                        .position(|t| t.eq_ignore_ascii_case(&s.msg_type))
                        .unwrap_or(0),
                    field: s.field.clone(),
                    id_query: String::new(),
                    type_query: String::new(),
                    field_query: String::new(),
                })
                .collect()
        },
    );
    PlotEditor {
        index,
        name: plot.map(|p| p.name.clone()).unwrap_or_default(),
        series,
    }
}

/// Show the Plots manager window. Does nothing if it isn't open.
pub fn show_manager(ctx: &egui::Context, session: &mut Session, state: &mut PlotsState) {
    if !state.manager_open {
        return;
    }
    if state.editor.is_none() && super::add_requested(ctx) {
        state.editor = Some(editor_for(session, None));
        state.error = None;
    }
    egui::Window::new("Plots")
        .collapsible(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            let mut remove = None;
            let mut edit = None;
            for (i, plot_def) in session.plots.iter().enumerate() {
                ui.horizontal(|ui| {
                    let series_summary = plot_def.series.len();
                    ui.label(format!("{} ({series_summary} series)", plot_def.name));
                    let mut shown = state.open.contains(&i);
                    if ui.checkbox(&mut shown, "Show").clicked() {
                        if shown {
                            state.open.insert(i);
                            state
                                .cache
                                .entry(i)
                                .or_insert_with(|| PlotsState::extract(session, plot_def));
                        } else {
                            state.open.remove(&i);
                        }
                    }
                    if ui.small_button("Edit").clicked() {
                        edit = Some(i);
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }

            if let Some(i) = remove {
                session.plots.remove(i);
                let reindex = |o: usize| if o > i { o - 1 } else { o };
                state.open = state.open.iter().filter(|&&o| o != i).map(|&o| reindex(o)).collect();
                state.cache = state
                    .cache
                    .drain()
                    .filter(|&(o, _)| o != i)
                    .map(|(o, v)| (reindex(o), v))
                    .collect();
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
                        .button("Add plot")
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
                    state.manager_open = false;
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

                let mut remove_series = None;
                for (row, series) in editor.series.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let id_label = session.id_option_text(series.id_choice);
                        if let Some(choice) = searchable_combo(
                            ui,
                            &format!("plot_series_id_{row}"),
                            &id_label,
                            &session.filter_dropdown_labels(0),
                            &mut series.id_query,
                        ) {
                            series.id_choice = choice;
                        }
                        let type_label = session
                            .type_options
                            .get(series.type_choice)
                            .cloned()
                            .unwrap_or_default();
                        if let Some(choice) = searchable_combo(
                            ui,
                            &format!("plot_series_type_{row}"),
                            &type_label,
                            &session.type_options,
                            &mut series.type_query,
                        ) && choice != series.type_choice
                        {
                            series.type_choice = choice;
                            series.field.clear();
                        }
                        let field_options = session.column_dropdown_labels(3, series.type_choice);
                        let field_label =
                            if series.field.is_empty() { "(pick a field)" } else { &series.field };
                        if let Some(choice) = searchable_combo(
                            ui,
                            &format!("plot_series_field_{row}"),
                            field_label,
                            &field_options,
                            &mut series.field_query,
                        ) {
                            series.field = field_options[choice].clone();
                        }
                        if ui.small_button("Remove series").clicked() {
                            remove_series = Some(row);
                        }
                    });
                }
                if let Some(row) = remove_series {
                    editor.series.remove(row);
                }
                if ui.button("Add series").clicked() {
                    editor.series.push(empty_series());
                }

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
                if editor.series.iter().any(|s| s.field.is_empty()) {
                    state.error = Some("Every series needs a field before saving".to_string());
                } else {
                    let editor = state.editor.take().unwrap();
                    let name = if editor.name.trim().is_empty() {
                        "Plot".to_string()
                    } else {
                        editor.name.trim().to_string()
                    };
                    let series = editor
                        .series
                        .into_iter()
                        .map(|s| {
                            let (sysid, compid) = match s.id_choice.checked_sub(1) {
                                Some(i) => {
                                    let (sys, comp) = session.id_options[i];
                                    (Some(sys), Some(comp))
                                }
                                None => (None, None),
                            };
                            SeriesDef {
                                sysid,
                                compid,
                                msg_type: session.type_options[s.type_choice].clone(),
                                field: s.field,
                            }
                        })
                        .collect();
                    let plot_def = PlotDef { name, series };
                    let index = match editor.index {
                        Some(i) => {
                            session.plots[i] = plot_def;
                            i
                        }
                        None => {
                            session.plots.push(plot_def);
                            let i = session.plots.len() - 1;
                            state.open.insert(i);
                            i
                        }
                    };
                    // Edited series definitions invalidate any cached points.
                    if state.open.contains(&index) {
                        let points = PlotsState::extract(session, &session.plots[index]);
                        state.cache.insert(index, points);
                    } else {
                        state.cache.remove(&index);
                    }
                    state.error = None;
                }
            } else if cancel {
                state.editor = None;
                state.error = None;
            }
        });
}

/// Show every currently-open plot as its own `egui_plot::Plot` window.
pub fn show_open_plots(ctx: &egui::Context, session: &Session, state: &mut PlotsState) {
    let mut to_close = Vec::new();
    for &i in &state.open {
        let Some(plot_def) = session.plots.get(i) else {
            continue;
        };
        // Cache should already hold this plot's points (populated on show/
        // save), but recompute defensively if it doesn't rather than panic.
        let points = state
            .cache
            .entry(i)
            .or_insert_with(|| PlotsState::extract(session, plot_def));
        let mut still_open = true;
        egui::Window::new(&plot_def.name)
            .id(egui::Id::new(("plot_window", i)))
            .default_size([500.0, 320.0])
            .open(&mut still_open)
            .show(ctx, |ui| {
                render_plot(ui, session, plot_def, points, i);
            });
        if !still_open {
            to_close.push(i);
        }
    }
    for i in to_close {
        state.open.remove(&i);
        state.cache.remove(&i);
    }
}

/// Vertical mark lines use the same red as the list's marked-row background
/// (`super::MARK_BG`), brightened so a 1px dashed line stays visible.
const MARK_LINE: egui::Color32 = egui::Color32::from_rgb(220, 70, 70);

fn render_plot(
    ui: &mut egui::Ui,
    session: &Session,
    plot_def: &PlotDef,
    points: &[Vec<[f64; 2]>],
    plot_index: usize,
) {
    let time_format = session.time_format;
    let start_us = session.start_us;
    let marks: Vec<(usize, f64, &str)> = session
        .marks
        .iter()
        .map(|(&entry_index, label)| {
            let ts = session.entries[entry_index].timestamp_us;
            (entry_index, ts as f64, label.as_str())
        })
        .collect();

    egui_plot::Plot::new(("plot", plot_index))
        .legend(Legend::default())
        .x_axis_formatter(move |mark, _range| match time_format {
            TimeFormat::DateTime => format_datetime(mark.value.max(0.0) as u64),
            TimeFormat::OffsetSecs => format_offset(mark.value.max(0.0) as u64, start_us),
        })
        .show(ui, |plot_ui| {
            for (series, series_points) in plot_def.series.iter().zip(points) {
                let label = series_label(series);
                plot_ui.line(Line::new(label, PlotPoints::from(series_points.clone())));
            }
            for &(entry_index, x, label) in &marks {
                // A fixed color: the default (transparent) would consume the
                // next color from the same auto-color cycle the data series
                // draw from, making marks look like just another series.
                // An explicit id too: egui_plot derives item ids from names,
                // and marks sharing a name (all unlabeled ones) would
                // otherwise share one id and corrupt per-item hover state.
                let name = if label.is_empty() { "mark" } else { label };
                plot_ui.vline(
                    VLine::new(name, x)
                        .id(egui::Id::new(("mark", entry_index)))
                        .color(MARK_LINE)
                        .width(1.5)
                        .style(LineStyle::dashed_loose()),
                );
            }
        });
}

fn series_label(series: &SeriesDef) -> String {
    match series.sysid.zip(series.compid) {
        Some((s, c)) => format!("{s}:{c} {}.{}", series.msg_type, series.field),
        None => format!("{}.{}", series.msg_type, series.field),
    }
}
