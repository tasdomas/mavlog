//! The Plots sidebar (a panel under the message list that lists
//! `Session::plots` with per-plot Show toggles plus Edit/Remove/Add), the
//! add/edit dialog, and the rendering of each shown plot as its own floating
//! `egui_plot::Plot` window — the motivating feature for the whole GUI.
//!
//! Each plot window draws on a white canvas (light egui visuals), supports
//! egui_plot's built-in pan/box-zoom/scroll-zoom with a Reset button, and can
//! export its canvas to a PNG via a screenshot request (`handle_export`).
//! Series carrying the same measurement unit share one Y axis; other units are
//! rescaled onto extra axes (matplotlib twinx style) via [`plot::axis_groups`].

use std::collections::{HashMap, HashSet};

use eframe::egui;
use egui_plot::{AxisHints, HPlacement, HoverPosition, Legend, Line, LineStyle, PlotPoints, VLine};

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
    /// Optional user legend name (blank = fall back to the `type.field` label).
    name: String,
    /// Optional measurement unit; series sharing a unit share a Y axis.
    unit: String,
    id_query: String,
    type_query: String,
    field_query: String,
}

struct PlotEditor {
    /// Index into `Session::plots` being edited, or None when adding new.
    index: Option<usize>,
    name: String,
    series: Vec<SeriesEditor>,
    show_marks: bool,
}

/// State for the Plots sidebar, the add/edit dialog and the open plot windows;
/// owned by `GuiApp`.
#[derive(Default)]
pub struct PlotsState {
    editor: Option<PlotEditor>,
    /// Focus the editor dialog's first field on the next frame, so keyboard
    /// (Tab/Enter) navigation starts inside a freshly opened dialog and stays
    /// contained when Tab would otherwise let it escape.
    editor_focus: bool,
    error: Option<String>,
    /// Indices into `Session::plots` currently shown as their own windows.
    open: HashSet<usize>,
    /// Extracted (and decimated) points per plot's series, keyed by index into
    /// `Session::plots`. `Session`'s underlying tlog data never changes once
    /// loaded, so this is computed once per show/edit rather than every frame —
    /// without it, every open plot window would re-decode all of its matching
    /// entries ~60 times a second.
    cache: HashMap<usize, Vec<plot::SeriesData>>,
    /// A pending "Export PNG" request awaiting the screenshot reply. Set when
    /// the user picks a path; consumed by `handle_export` when the rendered
    /// frame arrives as an `Event::Screenshot` (a frame or two later).
    pending_export: Option<PendingExport>,
}

/// A plot export the user has confirmed a path for, waiting on the frame
/// capture. `rect` is the plot's white area in UI points, cropped out of the
/// full-viewport screenshot.
struct PendingExport {
    path: std::path::PathBuf,
    rect: egui::Rect,
}

impl PlotsState {
    /// Open the dialog to add a new plot (the `p` / Plots-button shortcut).
    pub fn add_plot(&mut self, session: &Session) {
        self.editor = Some(editor_for(session, None));
        self.editor_focus = true;
        self.error = None;
    }

    /// Whether the add/edit dialog is open (used by the Esc-to-close audit and
    /// Tab containment).
    pub fn has_editor(&self) -> bool {
        self.editor.is_some()
    }

    pub fn cancel_editor(&mut self) {
        self.editor = None;
        self.error = None;
    }

    /// Ask the open editor dialog to re-grab keyboard focus (keeps Tab inside
    /// it). No-op when the dialog is closed.
    pub fn grab_editor_focus(&mut self) {
        if self.editor.is_some() {
            self.editor_focus = true;
        }
    }

    /// Discard cached points, open windows and any editor. Called when a new
    /// file is loaded, since these are all keyed by index into the old file's
    /// plots.
    pub fn reset(&mut self) {
        self.editor = None;
        self.error = None;
        self.editor_focus = false;
        self.open.clear();
        self.cache.clear();
        self.pending_export = None;
    }

    fn extract(session: &Session, plot_def: &PlotDef) -> Vec<plot::SeriesData> {
        plot_def.series.iter().map(|s| plot::extract(session, s)).collect()
    }
}

fn empty_series() -> SeriesEditor {
    SeriesEditor {
        id_choice: 0,
        type_choice: 0,
        field: String::new(),
        name: String::new(),
        unit: String::new(),
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
                    name: s.name.clone().unwrap_or_default(),
                    unit: s.unit.clone().unwrap_or_default(),
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
        show_marks: plot.is_none_or(|p| p.show_marks),
    }
}

/// Render the Plots sidebar (the caller supplies the docked bottom panel) into
/// `ui`: the plot list, each row with a Show toggle, Edit and Remove, plus an
/// Add button. Shown by `GuiApp` whenever the session has any plots. The
/// add/edit dialog is a separate window (`show_editor`); each shown plot draws
/// in its own window (`show_open_plots`).
pub fn show_sidebar(ui: &mut egui::Ui, session: &mut Session, state: &mut PlotsState) {
    let ctx = ui.ctx().clone();
    if ui
        .button("Add plot")
        .on_hover_text(super::hint(&ctx, &super::PLOTS_SHORTCUT, "p"))
        .clicked()
    {
        state.add_plot(session);
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut remove = None;
            let mut edit = None;
            for (i, plot_def) in session.plots.iter().enumerate() {
                ui.horizontal(|ui| {
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
                    let series_summary = plot_def.series.len();
                    ui.label(format!("{} ({series_summary} series)", plot_def.name));
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
                state.open =
                    state.open.iter().filter(|&&o| o != i).map(|&o| reindex(o)).collect();
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
                state.editor_focus = true;
                state.error = None;
            }
        });
}

/// Show the add/edit plot dialog while one is open (`PlotsState::add_plot` via
/// the `p` shortcut, or the sidebar's Edit button): a floating window with the
/// name, a "Show markers" toggle, the variable-length series list and
/// Save/Cancel. Adding a plot also opens it.
pub fn show_editor(ctx: &egui::Context, session: &mut Session, state: &mut PlotsState) {
    if state.editor.is_none() {
        return;
    }
    let editing = state.editor.as_ref().is_some_and(|e| e.index.is_some());
    let title = if editing { "Edit plot" } else { "Add plot" };
    let mut focus = std::mem::take(&mut state.editor_focus);
    egui::Window::new(title)
        .id(egui::Id::new("plot_editor"))
        .collapsible(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            let mut save = false;
            let mut cancel = false;
            if let Some(editor) = &mut state.editor {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    let resp = ui.text_edit_singleline(&mut editor.name);
                    if focus {
                        resp.request_focus();
                        focus = false;
                    }
                });
                ui.checkbox(&mut editor.show_marks, "Show markers");

                let mut remove_series = None;
                for (row, series) in editor.series.iter_mut().enumerate() {
                    ui.group(|ui| {
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
                            let field_options =
                                session.column_dropdown_labels(3, series.type_choice);
                            let field_label = if series.field.is_empty() {
                                "(pick a field)"
                            } else {
                                &series.field
                            };
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
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.add(
                                egui::TextEdit::singleline(&mut series.name)
                                    .desired_width(140.0)
                                    .hint_text("(legend label)"),
                            );
                            ui.label("Unit:");
                            ui.add(
                                egui::TextEdit::singleline(&mut series.unit)
                                    .desired_width(70.0)
                                    .hint_text("e.g. m/s"),
                            );
                        });
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
                                name: none_if_blank(s.name),
                                unit: none_if_blank(s.unit),
                            }
                        })
                        .collect();
                    let plot_def = PlotDef { name, series, show_marks: editor.show_marks };
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

/// Show every currently-shown plot as its own `egui_plot::Plot` window. A
/// plot is shown when its "Show" toggle in the sidebar is checked; closing a
/// plot's window (its title-bar ✕) unchecks it.
pub fn show_open_plots(ctx: &egui::Context, session: &Session, state: &mut PlotsState) {
    let mut to_close = Vec::new();
    // At most one export per frame; collected here and acted on after the loop
    // so the `&state.open` borrow is released before touching other state.
    let mut export_request: Option<(usize, egui::Rect)> = None;
    for &i in &state.open {
        let Some(plot_def) = session.plots.get(i) else {
            continue;
        };
        // Cache should already hold this plot's points (populated on show/
        // save), but recompute defensively if it doesn't rather than panic.
        let series_data = state
            .cache
            .entry(i)
            .or_insert_with(|| PlotsState::extract(session, plot_def));
        let mut still_open = true;
        let mut result = None;
        egui::Window::new(&plot_def.name)
            .id(egui::Id::new(("plot_window", i)))
            .default_size([560.0, 360.0])
            .open(&mut still_open)
            .show(ctx, |ui| {
                result = Some(render_plot(ui, session, plot_def, series_data, i));
            });
        if !still_open {
            to_close.push(i);
        }
        if let Some(r) = result
            && r.export_clicked
        {
            export_request = Some((i, r.export_rect));
        }
    }
    for i in to_close {
        state.open.remove(&i);
        state.cache.remove(&i);
    }

    // An "Export PNG" click: ask for a path (blocking, like opening a file),
    // then request a screenshot. The capture arrives a frame or two later and
    // is cropped/saved by `handle_export`.
    if let Some((i, rect)) = export_request
        && let Some(name) = session.plots.get(i).map(|p| p.name.clone())
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name(format!("{name}.png"))
            .save_file()
    {
        state.pending_export = Some(PendingExport { path, rect });
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
    }
}

/// If an export is pending and this frame delivered its screenshot, crop the
/// plot area out of the full-viewport capture and write it as a PNG. Returns a
/// status message (success or failure) to surface in the status bar, or `None`
/// when there is nothing to do. Call once per frame from the app's `update`.
pub fn handle_export(ctx: &egui::Context, state: &mut PlotsState) -> Option<String> {
    // Nothing requested → don't consume any screenshot events (other code may
    // rely on them in the future).
    state.pending_export.as_ref()?;
    let screenshot = ctx.input(|i| {
        i.raw.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    })?;
    let PendingExport { path, rect } = state.pending_export.take()?;
    let cropped = screenshot.region(&rect, Some(ctx.pixels_per_point()));
    let [w, h] = cropped.size;
    match image::save_buffer(
        &path,
        cropped.as_raw(),
        w as u32,
        h as u32,
        image::ExtendedColorType::Rgba8,
    ) {
        Ok(()) => Some(format!("Plot exported to {}", path.display())),
        Err(e) => Some(format!("Plot export failed: {e}")),
    }
}

/// Vertical mark lines use the same red as the list's marked-row background
/// (`super::MARK_BG`), brightened so a 1px dashed line stays visible.
const MARK_LINE: egui::Color32 = egui::Color32::from_rgb(220, 70, 70);

/// What a plot window reports back to `show_open_plots`: the on-screen rect of
/// its white canvas (for cropping a screenshot) and whether Export was clicked.
struct RenderResult {
    export_rect: egui::Rect,
    export_clicked: bool,
}

/// Format a plot x coordinate (microsecond timestamp) as time, matching the
/// x-axis ticks. Shared by the axis formatter and the hover label.
fn fmt_time(value: f64, time_format: TimeFormat, start_us: u64) -> String {
    match time_format {
        TimeFormat::DateTime => format_datetime(value.max(0.0) as u64),
        TimeFormat::OffsetSecs => format_offset(value.max(0.0) as u64, start_us),
    }
}

fn render_plot(
    ui: &mut egui::Ui,
    session: &Session,
    plot_def: &PlotDef,
    series_data: &[plot::SeriesData],
    plot_index: usize,
) -> RenderResult {
    let time_format = session.time_format;
    let start_us = session.start_us;

    // Toolbar, kept outside the white frame so it is not part of the export.
    let mut reset = false;
    let mut export_clicked = false;
    ui.horizontal(|ui| {
        reset = ui
            .button("Reset view")
            .on_hover_text("Restore auto bounds (or double-click the plot)")
            .clicked();
        export_clicked = ui
            .button("Export PNG…")
            .on_hover_text("Save the plot area to a PNG image")
            .clicked();
        ui.label(
            egui::RichText::new("right-drag: box zoom · scroll: zoom · double-click: reset")
                .weak()
                .small(),
        );
    });

    // Marks are only collected/drawn when this plot opts into them.
    let marks: Vec<(usize, f64, &str)> = if plot_def.show_marks {
        session
            .marks
            .iter()
            .map(|(&entry_index, label)| {
                let ts = session.entries[entry_index].timestamp_us;
                (entry_index, ts as f64, label.as_str())
            })
            .collect()
    } else {
        Vec::new()
    };

    // Group series into shared Y axes by unit. Non-primary groups are drawn
    // rescaled into the primary group's coordinate space so one plot transform
    // can host several units at once (egui_plot's custom axes are display-only).
    let groups = plot::axis_groups(&plot_def.series, series_data);
    let multi_axis = groups.len() > 1;
    // Per-series affine map `(scale, offset)` into the shared space.
    let mut xform = vec![(1.0_f64, 0.0_f64); plot_def.series.len()];
    for g in &groups {
        for &row in &g.series {
            xform[row] = (g.scale, g.offset);
        }
    }
    // Hover lookup keyed by legend label: real values are recovered by
    // inverting each series' affine map.
    let hover: Vec<(String, f64, f64, Option<String>)> = plot_def
        .series
        .iter()
        .enumerate()
        .map(|(row, s)| {
            let unit = s.unit.clone().filter(|u| !u.trim().is_empty());
            (series_label(s), xform[row].0, xform[row].1, unit)
        })
        .collect();

    // White canvas: light visuals give a white plot background plus readable
    // grid/tick/legend colors; the frame fill also covers the axis strips that
    // sit just outside egui_plot's inner (white) rect.
    let framed = egui::Frame::new()
        .fill(egui::Color32::WHITE)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.style_mut().visuals = egui::Visuals::light();

            let mut plot = egui_plot::Plot::new(("plot", plot_index))
                .legend(Legend::default())
                .allow_boxed_zoom(true)
                .allow_double_click_reset(true)
                .x_axis_formatter(move |mark, _range| fmt_time(mark.value, time_format, start_us))
                .label_formatter(move |pos| match pos {
                    HoverPosition::NearDataPoint { plot_name, position, .. } => {
                        let (scale, offset, unit) = hover
                            .iter()
                            .find(|(l, ..)| l.as_str() == *plot_name)
                            .map(|(_, s, o, u)| (*s, *o, u.clone()))
                            .unwrap_or((1.0, 0.0, None));
                        let real_y = (position.y - offset) / scale;
                        let unit = unit.map(|u| format!(" {u}")).unwrap_or_default();
                        Some(format!(
                            "{plot_name}\n{}\n{real_y:.3}{unit}",
                            fmt_time(position.x, time_format, start_us)
                        ))
                    }
                    HoverPosition::Elsewhere { position } => Some(format!(
                        "{}\n{:.3}",
                        fmt_time(position.x, time_format, start_us),
                        position.y
                    )),
                });

            // One Y axis per unit group when there is more than one; otherwise
            // just label the single axis with its unit (if any).
            if multi_axis {
                let hints: Vec<AxisHints> = groups
                    .iter()
                    .enumerate()
                    .map(|(gi, g)| {
                        let (scale, offset) = (g.scale, g.offset);
                        let placement =
                            if gi == 0 { HPlacement::Left } else { HPlacement::Right };
                        AxisHints::new_y()
                            .label(g.unit.clone().unwrap_or_default())
                            .placement(placement)
                            .formatter(move |mark, _range| {
                                let real = (mark.value - offset) / scale;
                                let real_step = (mark.step_size / scale).abs();
                                let decimals = if real_step > 0.0 {
                                    (-real_step.log10()).round().max(0.0) as usize
                                } else {
                                    0
                                };
                                egui_plot::format_number(real, decimals)
                            })
                    })
                    .collect();
                plot = plot.custom_y_axes(hints);
            } else if let Some(unit) = groups.first().and_then(|g| g.unit.clone()) {
                plot = plot.y_axis_label(unit);
            }

            let response = plot.show(ui, |plot_ui| {
                if reset {
                    plot_ui.set_auto_bounds(true);
                }
                for (row, (series, data)) in
                    plot_def.series.iter().zip(series_data).enumerate()
                {
                    let (scale, offset) = xform[row];
                    let points: Vec<[f64; 2]> = if scale == 1.0 && offset == 0.0 {
                        data.points.clone()
                    } else {
                        data.points.iter().map(|&[x, y]| [x, y * scale + offset]).collect()
                    };
                    let label = series_label(series);
                    // Explicit id: egui_plot derives item ids from names, and
                    // user-set series names can collide — a shared id would
                    // corrupt per-item hover state (same reason as the VLines).
                    plot_ui.line(
                        Line::new(label, PlotPoints::from(points))
                            .id(egui::Id::new(("plot_series", plot_index, row))),
                    );
                }
                for &(entry_index, x, _) in &marks {
                    // A fixed color: the default (transparent) would consume the
                    // next color from the same auto-color cycle the data series
                    // draw from, making marks look like just another series.
                    // An empty name keeps marks out of the legend (their labels
                    // are painted onto the plot below instead), which is also
                    // why the id must be explicit: egui_plot derives item ids
                    // from names, and identical names mean one shared id,
                    // corrupting per-item hover state.
                    plot_ui.vline(
                        VLine::new("", x)
                            .id(egui::Id::new(("mark", entry_index)))
                            .color(MARK_LINE)
                            .width(1.5)
                            .style(LineStyle::dashed_loose()),
                    );
                }
            });

            // Mark labels: vertical text rising from the bottom of the plot
            // beside each mark's line. Painted after the plot because
            // egui_plot's own Text item can't rotate; the PlotResponse
            // transform maps the mark's time to a screen x.
            let frame = *response.transform.frame();
            let painter = ui.painter().with_clip_rect(frame.intersect(ui.clip_rect()));
            let font = egui::TextStyle::Small.resolve(ui.style());
            for &(_, x, label) in &marks {
                if label.is_empty() {
                    continue;
                }
                let line_x = response.transform.position_from_point_x(x);
                if !frame.x_range().contains(line_x) {
                    continue;
                }
                let galley = painter.layout_no_wrap(label.to_string(), font.clone(), MARK_LINE);
                // TextShape rotates clockwise around `pos` (the galley's
                // unrotated top-left), so -90° makes the text read bottom-up,
                // its glyph column just right of the mark line.
                let pos = egui::pos2(line_x + 3.0, frame.bottom() - 4.0);
                painter.add(
                    egui::epaint::TextShape::new(pos, galley, MARK_LINE)
                        .with_angle(-std::f32::consts::FRAC_PI_2),
                );
            }
        });

    RenderResult { export_rect: framed.response.rect, export_clicked }
}

/// Trim `s`; return `None` if nothing is left, else the trimmed owned string.
fn none_if_blank(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn series_label(series: &SeriesDef) -> String {
    if let Some(name) = series.name.as_ref().filter(|n| !n.is_empty()) {
        return name.clone();
    }
    match series.sysid.zip(series.compid) {
        Some((s, c)) => format!("{s}:{c} {}.{}", series.msg_type, series.field),
        None => format!("{}.{}", series.msg_type, series.field),
    }
}
