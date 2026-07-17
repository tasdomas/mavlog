# mavlog GUI mode — session handoff

Self-contained pickup notes for continuing the graphical-UI work. If you are a
fresh session, read this top to bottom before touching code.

## What this project is

`mavlog` is a viewer for MAVLink `.tlog` files. Until recently it was a pure
ratatui **TUI**: scrollable message list, decoded detail pane, filters (text +
popup editor with autocomplete dropdowns), custom "last-seen value" columns,
marks with labels, jump-to-time, a settings menu, and per-file setup
persistence to a `<file>.mavlog.json` sidecar.

## The goal of this work

Add a **graphical, system-native GUI** (works on macOS/Linux/Windows), primarily
to enable interactive plotting (which a terminal can't do well). Locked-in
decisions (already agreed with the user — do not re-litigate):

- **Framework: egui/eframe** (pure Rust, native binary per OS; same ecosystem as
  `egui_plot` for plotting and `egui_extras` for virtualized tables).
- **GUI is the default mode**; the TUI stays available behind a `-tui`/`--tui`
  flag. Both frontends share one core.
- **Full parity first**: the GUI milestone must match everything the TUI does,
  plus plots.
- **Delivery: small, explainable commits on branch `feat/gui-mode`.** Every
  commit must build and keep `cargo test` green. Do not push or open PRs unless
  asked. Commit messages end with the `Co-Authored-By: Claude ...` trailer.

The full roadmap also lives at `~/.claude/plans/sunny-frolicking-mango.md` (same
content, plus the phase-by-phase TODO). This file is the authoritative state.

## Current git state

- Baseline: `master` = the original working TUI (commit `f65b883`).
- Working branch: **`feat/gui-mode`** (all new work here). Working tree clean.
- Commits so far (newest first):
  - `da77a4e feat(gui): add eframe app shell and mode dispatch`  ← **HEAD**
  - `7ab5211 refactor: move TUI into its own module`
  - `d1791e6 test: cover Session filter, jump and setup roundtrip`
  - `c25dc46 refactor: introduce core::session::Session owning domain state`
  - `1af3f11 refactor: extract core::setup module`
  - `5ef86f2 refactor: extract core::column module`
  - `bc57d11 refactor: extract core::filter module`
  - `c89b87b refactor: extract core::time module`
  - `cc1b379 chore: ignore setup sidecar files`
- `.gitignore` ignores `/target` and `*.mavlog.json`. `sample.tlog` is committed
  as a test fixture (500 synthetic messages: HEARTBEAT, ATTITUDE,
  GLOBAL_POSITION_INT, VFR_HUD, BATTERY_STATUS, STATUSTEXT; VFR_HUD.groundspeed
  and GLOBAL_POSITION_INT.relative_alt vary over time, good for plotting).

## Architecture (current source tree)

Core is frontend-agnostic — it must never import ratatui/crossterm/egui.

```
src/
  main.rs        — entry point: arg parsing (-tui/--tui, optional FILE),
                   loads a Session, dispatches to gui::run (default) or tui::run.
  tlog.rs        — tlog/MAVLink parser (unchanged from before this work).
  core/
    mod.rs       — declares submodules
    time.rs      — TimeFormat, format_datetime, format_offset, parse_jump (+tests)
    filter.rs    — FilterExpr {matches,to_text}, parse_filters, name_matches,
                   match_labels (+tests)
    column.rs    — CustomColumn {to_text}, parse_columns (+tests)
    setup.rs     — Setup (serde sidecar struct), setup_path
    session.rs   — Session: ALL per-file domain state + operations (+tests)
  tui/
    mod.rs       — ratatui frontend. `pub fn run(Session)`. App = Session +
                   terminal view state (offset, focus, detail_scroll, popups,
                   prompts, status). All key handling + drawing live here.
  gui/
    mod.rs       — eframe frontend. `pub fn run(Option<Session>)`. GuiApp so far
                   is just the shell (toolbar/status/central, empty state).
```

### `Session` (src/core/session.rs) — the shared heart

Public fields (all `pub`): `path, data, entries, start_us, time_format,
filters, filter_text, filtered, columns, columns_text, marks, id_options,
type_options, selected`.

Key methods the GUI should reuse (do NOT reimplement in the GUI):
- `Session::new(path, data, entries)` — builds id/type option lists, filtered=all.
- `selected_entry_index() -> Option<usize>`
- `apply_filter()` — rebuild `filtered`, keep selection near previous.
- `jump_to_time(target_us)` — select first visible entry at/after a time.
- `rebuild_filter_text()` — regenerate `filter_text` from `filters`.
- `set_columns(Vec<CustomColumn>)` — index each column's matching entries.
- `column_value(&col, entry_index) -> String` — last-seen field value.
- `field_options(msg_type) -> Vec<String>` — field names of a type (from first
  decodable sample; used to populate the column/plot field dropdowns).
- `id_option_text` / `type_option_text` / `filter_dropdown_labels` /
  `column_dropdown_labels` — dropdown label helpers.
- `format_list_time(ts) -> String` — per current time mode.
- `save_setup() -> Result<String,String>` (Ok=path) / `load_setup() -> Option<String>`
  (status message; None if no sidecar). Setup is the same JSON sidecar the TUI
  uses, so setups round-trip between GUI and TUI.

`core::filter::match_labels(labels, query)` is the case-insensitive substring
matcher the searchable dropdowns use — reuse it for GUI autocomplete.

## Status: what's done vs. remaining

### Phase 0 — DONE
Core fully extracted into `core::{time,filter,column,setup,session}`; `Session`
owns domain state/logic; the TUI wraps it (`App { session: Session, ...view }`,
reaching state through `self.session`). 15 unit tests pass. TUI behavior
verified unchanged (filter, custom column, offset time, mark+label, save,
auto-load) via tmux smoke tests.

### Phase 1 — PARTIALLY DONE (this is where you resume)
Done and committed (`da77a4e`):
- Deps added: `eframe` 0.35, `egui_extras` 0.35, `egui_plot` 0.36, `rfd` 0.17
  (all unify on a single `egui` 0.35).
- `src/gui/mod.rs`: minimal shell — top toolbar (a placeholder "Open…" button +
  file summary label), bottom status bar (shows setup-load result), central
  panel showing the loaded-file summary or an empty-state message. Auto-runs
  `Session::load_setup()` on start.
- `main.rs`: arg parsing + dispatch (GUI default; `-tui` requires a file).

**REMAINING for Phase 1 (do this first next session):**
1. Wire the toolbar **"Open…"** button to `rfd::FileDialog::new().pick_file()`,
   plus a Ctrl/Cmd+O shortcut.
2. **Drag-and-drop**: read `ui.ctx().input(|i| i.raw.dropped_files.clone())` and
   open the first dropped file.
3. On open: read+parse the file with the same logic as `main::load_session`
   (factor it into a reusable helper — currently in `main.rs`), build a
   `Session`, run `load_setup()`, replace `GuiApp::session`.
4. **Parse-error modal**: if load fails (unreadable / no MAVLink messages), show
   an `egui::Window` (or `egui::Modal`) with the error instead of crashing.
5. Empty-state polish: a centered "Open a .tlog file" with an Open button.
6. Phase 1 gate: `cargo run -- sample.tlog` opens on macOS; warning-free build.

### Phases 2–5 — NOT STARTED (summary; full detail in the ~/.claude plan)
- **Phase 2 — list & detail parity**: virtualized message table via
  `egui_extras::TableBuilder` (columns #/TIME/SYS:CMP/MESSAGE/custom/LABEL;
  keyboard+mouse selection; follow-scroll; marked rows red bg; TIME per format).
  Detail side panel (`egui::SidePanel`) with the same decoded `{msg:#?}` body +
  hex-dump fallback and Time/offset/Mark lines. Jump-to-time toolbar box wired to
  `parse_jump` + `Session::jump_to_time`.
- **Phase 3 — filters/columns/marks/settings parity**: a shared searchable
  dropdown widget (`gui/widgets.rs`, reuse `match_labels`); Filters window
  (list/editor + chips + "x of y" count); Columns window (name/id/type/field
  editor, field options from `Session::field_options`); marks (Space + label
  popup + right-click context menu); Settings (time format radio); Save setup
  (Ctrl/Cmd+S). Cross-check: a setup saved in the GUI loads in the TUI.
- **Phase 4 — plots (the motivating feature)**: add `core::plot.rs`
  (`PlotDef { name, series: Vec<SeriesDef> }`, `SeriesDef { sysid, compid,
  msg_type, field }`, extraction reusing the column `matches` scan → decode →
  f64 coercion, reject non-numeric, min/max-bucket decimation above ~200k
  pts/series; unit-test extraction + decimation). Add a `plots` field to
  `Session` and to `Setup` (`#[serde(default)]` so old sidecars still load).
  Plots-manager window + series editor (reuse dropdowns). Each open plot is an
  `egui::Window` with `egui_plot::Plot` (legend, zoom/pan/box-zoom, hover, time
  x-axis formatted per mode, marks as vertical lines). Multiple plots at once.
- **Phase 5 — cross-platform + polish**: keyboard-shortcut audit + Help window;
  README (usage, GUI/TUI modes, Linux build deps: `libxkbcommon`, X11/Wayland,
  GTK3 for `rfd`); build verification notes; final regression.

## egui/eframe 0.35 API notes (verified against vendored source — don't re-derive)

The eframe/egui 0.35 API differs from older tutorials. Getting this wrong cost a
build cycle already:

- Import egui via `use eframe::egui;` (eframe re-exports it; there is **no direct
  `egui` dependency** in Cargo.toml).
- **`eframe::App` trait method is `ui`, not `update`:**
  `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`. (Optional
  `fn logic(&mut self, ctx: &egui::Context, frame)` runs before `ui`.)
- **Panels are unified under `egui::Panel` and take a `&mut Ui`** (not a Context):
  `egui::Panel::top("id").show(ui, |ui| …)`, `Panel::bottom/left/right(...)`, and
  `egui::CentralPanel::default().show(ui, |ui| …)`. Inside `App::ui` you already
  hold the root `ui`, so pass it straight to `.show(ui, …)`. Get the context with
  `ui.ctx()` when you need input/repaint.
- `eframe::run_native(title, NativeOptions, Box::new(|_cc| Ok(Box::new(app))))` —
  `AppCreator` returns `Result<Box<dyn App>, DynError>`.
- Window size:
  `NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([w,h]), ..Default::default() }`.

## How to build / test / run

- Build: `cargo build` (first build of the egui stack is slow, ~1.5 min; cached after).
- Test: `cargo test` — expect **15 passed**. Core tests must not need a display.
- Run GUI: `cargo run -- sample.tlog` (or with no arg for the empty state).
  A native window opens — run interactively on macOS; it can't be exercised in a
  headless/tmux context.
- Run TUI: `cargo run -- -tui sample.tlog`. The TUI **can** be smoke-tested via
  tmux, e.g.:
  ```
  tmux new-session -d -s smoke -x 120 -y 24 './target/debug/mavlog -tui sample.tlog'
  sleep 1; tmux send-keys -t smoke j j; sleep 0.3; tmux capture-pane -t smoke -p
  tmux kill-session -t smoke
  ```
  Note: crossterm merges Esc immediately followed by another key into an
  Alt-chord, so scripted `send-keys` sequences need a small delay after Esc.
- Keep all unit-testable logic in `core/` (no display needed there).

## Working conventions

- One logical change per commit; build + `cargo test` green at each; Conventional
  Commits; `Co-Authored-By` trailer. New behavior lands in `core/` first, then a
  thin frontend layer.
- Match surrounding style: no speculative helpers until a frontend needs them
  (e.g., `set_mark`/`remove_mark` were intentionally deferred to Phase 3 when the
  GUI marks land).
