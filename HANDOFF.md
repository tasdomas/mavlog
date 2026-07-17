# mavlog GUI mode — session handoff

Self-contained pickup notes for continuing the graphical-UI work. If you are a
fresh session, read this top to bottom before touching code.

**Status: all 5 planned phases (0–5) are done and committed.** What's left is
polish/verification, not missing features — see the end of each phase's entry
below for open follow-ups (mainly: no live display in this sandbox to visually
confirm the GUI, so a real macOS/Linux/Windows session should still eyeball it
before considering this "shipped").

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
  - `e939c62 docs: add README (Phase 5, part 2)`  ← **HEAD**
  - `3809459 feat(gui): Esc-to-close audit and Help window (Phase 5, part 1)`
  - `2d6387c feat: plots — data model, extraction and GUI (Phase 4)`
  - `9f36ad3 docs: mark Phase 3 done in handoff notes`
  - `61d7e53 refactor: reuse Session::filter_dropdown_labels/column_dropdown_labels`
  - `afe38b1 feat(gui): Columns window (Phase 3, part 4)`
  - `3289d77 feat(gui): Filters window (Phase 3, part 3)`
  - `6e2895f feat(gui): mark toggling, label editor and right-click menu (Phase 3, part 2)`
  - `532a76c feat(gui): settings window and save-setup shortcut (Phase 3, part 1)`
  - `1e9162f docs: mark Phase 2 done in handoff notes`
  - `9c8d751 feat(gui): message list, detail pane and jump-to-time (Phase 2)`
  - `a7cbb9d refactor: move hex_dump into tlog for GUI/TUI sharing`
  - `227f237 docs: mark Phase 1 done in handoff notes`
  - `6e477df feat(gui): open files via dialog, drag-and-drop, and error modal`
  - `50cf244 docs: add session handoff notes`
  - `da77a4e feat(gui): add eframe app shell and mode dispatch`
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
  tlog.rs        — tlog/MAVLink parser, plus hex_dump (moved here from the TUI
                   in a7cbb9d so the GUI detail pane can share it) (+tests).
  core/
    mod.rs       — declares submodules
    time.rs      — TimeFormat, format_datetime, format_offset, parse_jump (+tests)
    filter.rs    — FilterExpr {matches,to_text}, parse_filters, name_matches,
                   match_labels (+tests)
    column.rs    — CustomColumn {to_text}, parse_columns (+tests)
    plot.rs      — PlotDef/SeriesDef, extract() (scan+decode+f64 coerce+
                   decimate), GUI-only but pure/testable like the rest of
                   core (+tests)
    setup.rs     — Setup (serde sidecar struct incl. `plots`), setup_path
    session.rs   — Session: ALL per-file domain state + operations (+tests)
  tui/
    mod.rs       — ratatui frontend. `pub fn run(Session)`. App = Session +
                   terminal view state (offset, focus, detail_scroll, popups,
                   prompts, status). All key handling + drawing live here.
  gui/
    mod.rs       — eframe frontend. `pub fn run(Option<Session>)`. GuiApp owns
                   an `Option<Session>`, all GUI-only view state (jump/label
                   input buffers, scroll_to_selected, window-open flags), and
                   the toolbar/status/list/detail panels. Ctrl/Cmd+O opens,
                   Ctrl/Cmd+S saves the setup sidecar; drag-and-drop opens too.
    widgets.rs   — searchable_combo: an egui::ComboBox with a type-to-filter
                   search box in its popup (reuses core::filter::match_labels),
                   shared by the filter and column editors.
    filters.rs   — Filters window: list + dropdown-based editor (id/type) +
                   Save/Cancel, mirrors the TUI's FilterEditor.
    columns.rs   — Columns window: list + editor (name/id/type/field) +
                   Save/Cancel, mirrors the TUI's ColumnEditor.
    plots.rs     — Plots manager window (list/Show-toggle/Edit/Remove/Add,
                   variable-length series editor reusing searchable_combo)
                   + rendering of each shown plot as its own egui_plot::Plot
                   window. PlotsState::cache holds extracted points per open
                   plot so they aren't re-decoded every frame.
```

### `Session` (src/core/session.rs) — the shared heart

Public fields (all `pub`): `path, data, entries, start_us, time_format,
filters, filter_text, filtered, columns, columns_text, marks, id_options,
type_options, selected, plots`.

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

### Phase 1 — DONE
Done and committed (`da77a4e`, `6e477df`):
- Deps added: `eframe` 0.35, `egui_extras` 0.35, `egui_plot` 0.36, `rfd` 0.17
  (all unify on a single `egui` 0.35).
- `src/gui/mod.rs`: top toolbar ("Open…" button + file summary label), bottom
  status bar (shows setup-load result), central panel showing the loaded-file
  summary or an empty-state message with its own Open button. Auto-runs
  `Session::load_setup()` on start.
- `main.rs`: arg parsing + dispatch (GUI default; `-tui` requires a file);
  `load_session` is now `pub(crate)` so the GUI can call it too.
- Toolbar **"Open…"** button and a Ctrl/Cmd+O `KeyboardShortcut` both call
  `rfd::FileDialog::new().add_filter("MAVLink tlog", &["tlog"]).pick_file()`.
- **Drag-and-drop**: `ctx().input(|i| i.raw.dropped_files.clone())` is checked
  every frame; the first dropped file with a native `path` is opened.
- Both paths funnel through `GuiApp::open_path`, which calls
  `crate::load_session`, reloads the setup sidecar, and replaces
  `GuiApp::session` on success.
- **Parse-error modal**: on failure `open_path` sets `GuiApp::error`, shown next
  frame in an `egui::Window::new("Failed to open file")` with an OK button
  (no crash).
- Empty-state polish: centered hint text + an inline Open button.
- Phase 1 gate: `cargo build` is warning-free, `cargo test` passes (15/15), and
  `cargo run -- sample.tlog` runs without panicking. Note: this sandbox has no
  attached display (`screencapture` fails with "could not create image from
  display"), so the window's actual on-screen appearance is **not yet visually
  confirmed** — do that on a real macOS session before calling Phase 1 fully
  closed out.

### Phase 2 — DONE
Done and committed (`9c8d751`):
- `src/gui/mod.rs::list_panel`: virtualized message table via
  `egui_extras::TableBuilder` (columns #/TIME/SYS:CMP/MESSAGE/any custom
  columns already present in `session.columns`/LABEL). Row height from
  `ui.text_style_height`. Selection follows keyboard (arrows, Page Up/Down,
  Home/End — gated on `ctx.memory(|m| m.focused().is_none())` so they don't
  steal input from the jump box) and mouse clicks (via `row.response()` with
  `sense(egui::Sense::click())`). Marked rows get a red background painted
  per-cell with `ui.painter().rect_filled` (the `cell()` helper) since
  `egui_extras` only exposes boolean selected/hovered/striped flags, not
  arbitrary row colors; the selected row uses `row.set_selected(true)`
  (egui's built-in highlight) which always wins over the mark color, same
  precedence as the TUI.
- `GuiApp::scroll_to_selected` + `TableBuilder::scroll_to_row(_, Align::Center)`
  implement follow-scroll, but it's only set by keyboard nav / jump-to-time,
  never by a mouse click — a click should never fight the user for scroll
  position.
- `src/gui/mod.rs::detail_panel`: right `egui::Panel::right("detail")` with
  name/id header, `Time: … (offset)` line, a `Mark: ●` line when marked, then
  the decoded `{msg:#?}` (or `tlog::hex_dump` fallback) in a scrollable
  `egui::TextEdit::multiline` (kept interactive so text stays
  selectable/copyable; edits are harmless since the buffer is rebuilt from
  `Session` every frame and never written back).
- Toolbar jump-to-time box wired to `core::time::parse_jump` +
  `Session::jump_to_time`, Enter-to-submit, inline red error label on parse
  failure.
- `tlog::hex_dump` (moved out of `tui/mod.rs` in `a7cbb9d`) is now shared
  between both frontends.
- Not yet done, deferred to Phase 3 per the original plan: creating/editing
  filters, columns, or marks from the GUI — the table only *renders* whatever
  a loaded setup sidecar already populated. `cargo build`/`cargo test` verified
  after each commit; GUI window itself still not visually confirmed in this
  sandbox (no attached display) — check on a real macOS session.

### Phase 3 — DONE
Done and committed (`532a76c`, `6e2895f`, `3289d77`, `afe38b1`):
- **Settings + Save** (`532a76c`): toolbar "Settings" button opens an
  `egui::Window` with a Time-column radio group bound straight to
  `Session::time_format` (no apply step needed — the list/detail panes read
  it live every frame). "Save setup" button + Ctrl/Cmd+S both call
  `Session::save_setup`, reporting the outcome in the status bar.
- **Marks** (`6e2895f`): Space toggles the mark on the selected row (gated
  by `ctx.memory(|m| m.focused().is_none())`, same guard as list nav, so it
  doesn't fire while a text field has focus); adding a mark opens the "Mark
  label" window (`GuiApp::label_prompt`/`label_input`) prefilled empty,
  removing one discards the label — same semantics as the TUI's
  `toggle_mark`. Right-click any row for a context menu (unmarked: "Add
  mark"; marked: "Edit label" / "Remove mark") via `Response::context_menu`;
  the menu only records a `MarkAction` into a local, applied to
  `session.marks` after `TableBuilder::body()` returns (kept the
  nested-closure borrows simple rather than mutating `Session` from inside
  them).
- **Filters window** (`3289d77`, `src/gui/filters.rs`): lists filters as
  `to_text()` rows with Edit/Remove, an "x of y messages shown" count, and a
  dropdown-based editor (pick an id — any or a sysid:compid pair — and/or an
  exact type) with Save/Cancel, mirroring the TUI's `FilterEditor`. Saving
  calls `Session::rebuild_filter_text` + `apply_filter`.
- **Columns window** (`afe38b1`, `src/gui/columns.rs`): lists columns as
  `to_text()` rows with Edit/Remove, plus a Name/Id/Type/Field editor.
  Changing Type clears the chosen Field (fields belong to a type). Save is
  rejected with an inline error if no field is picked. Saving calls
  `Session::set_columns`.
- **Shared widget** (`src/gui/widgets.rs::searchable_combo`): an
  `egui::ComboBox` with a type-to-filter search box in its popup (reuses
  `core::filter::match_labels`), used by both the filter and column editors
  in place of hand-rolling the TUI's keyboard dropdown.
- Cross-check claim (a setup saved in the GUI loads in the TUI) holds by
  construction: the GUI editors mutate the same `Session.filters` /
  `Session.columns` / `Session.marks` / `Session.time_format` fields the TUI
  does, through the same `Session::save_setup`/`load_setup` and `Setup`
  sidecar format — not yet re-verified end-to-end with a live GUI session
  in this sandbox (no attached display; see the Phase 1 note).
- `cargo build`/`cargo test` verified green after each of the four commits
  above (15/15 at the time; more tests were added since, see Phase 4).

### Phase 4 — DONE
Done and committed (`2d6387c`):
- `core/plot.rs`: `PlotDef { name, series: Vec<SeriesDef> }`, `SeriesDef {
  sysid, compid, msg_type, field }`; `extract(session, series)` scans
  `session.entries` for the id/type match, decodes, coerces the named field
  to `f64` via the same "walk the serde_json object, skip non-numeric"
  approach as `Session::column_value` (enum fields serializing as `{"type":
  "..."}` are skipped, not coerced), then min/max-bucket decimates above
  ~200k points so a spike survives even when heavily downsampled. Unit-tested
  against real parsed HEARTBEAT frames (id filtering + numeric coercion) and
  both decimation branches (short series untouched, long series shrunk with
  the global max preserved).
- `Session` gained `plots: Vec<PlotDef>`; `Setup` gained `#[serde(default)]
  plots: Vec<PlotDef>` (old sidecars without it still load); `save_setup`/
  `load_setup` round-trip it like every other field — no new persistence
  mechanism.
- `gui/plots.rs`: a Plots manager window (per-plot Show checkbox/Edit/Remove
  rows, "Add plot") and an editor with a variable number of series rows, each
  an id/type/field `searchable_combo` trio built from
  `Session::filter_dropdown_labels`/`column_dropdown_labels` — no new dropdown
  plumbing needed, same reuse as the filters/columns editors. Each shown plot
  renders as its own `egui_plot::Plot` window: a `Line` per series, a legend,
  `VLine`s at every mark's timestamp, and an `x_axis_formatter` matching the
  current time mode (via `core::time::format_datetime`/`format_offset`).
  Zoom/pan/box-zoom/hover come for free from `egui_plot::Plot`.
- **Important perf note if you touch this file**: `PlotsState::cache` holds
  each open plot's already-extracted-and-decimated points, populated on Show
  or on Save-while-shown, keyed by plot index (with reindexing on Remove).
  `render_plot` reads from this cache instead of calling `plot::extract`
  directly — `Session`'s tlog data never changes after loading, so extraction
  is genuinely a one-time cost per plot, but `show_open_plots` runs every UI
  frame (~60/s) for every open plot window, and re-decoding+re-JSON-coercing
  every matching entry that often would have undermined the whole point of
  the decimation feature. Don't remove the cache to "simplify" this — it was
  added specifically because the first draft did exactly that and was
  measurably wasteful in reasoning about (not benchmarked in this sandbox,
  but the cost model is unambiguous: full linear scan + `serde_json::to_value`
  per matching entry, every frame, times however many plots are open).

### Phase 5 — DONE
Done and committed (`3809459`, `e939c62`):
- **Keyboard-shortcut audit**: the one real gap found was that no popup
  window (error modal, label editor, settings, filters, columns, plots
  manager) could be dismissed from the keyboard. `GuiApp::handle_escape` now
  closes whichever one is open on Esc, in a fixed precedence (egui doesn't
  expose real window stacking order, so this is a static list, not "topmost
  window" in a strict sense) — one press closes at most one window. Known
  minor rough edge: a `searchable_combo`'s own popup already closes itself on
  Esc; on that same keystroke, `handle_escape` may also close the popup's
  parent window (e.g. Filters) if nothing else takes precedence first. Not
  fixed — cosmetic, not a correctness bug.
- Toolbar **"Help"** button/window lists every keyboard shortcut and mouse
  interaction (Space to mark, arrow-key list nav, drag-and-drop, right-click
  — several of these had no other on-screen discovery path).
- **README.md**: usage for both frontends, the setup-sidecar round-trip
  guarantee, the shortcut table (mirrors the Help window), `cargo build
  --release` timing note, Linux system deps for `eframe`/`rfd`
  (`libxkbcommon-dev libx11-dev libgtk-3-dev` on Debian/Ubuntu — not verified
  by actually building on Linux in this sandbox, just standard guidance for
  this dependency stack), and how to run tests.
- **Final regression** (this sandbox, macOS host, no attached display):
  `cargo build` and `cargo build --release` both warning-free; `cargo test`
  green at **19/19**; `cargo run -- sample.tlog` runs without panicking for
  several seconds (can't interact with it headlessly); the TUI was
  re-smoke-tested via tmux after all the GUI-side churn (hex_dump move,
  Session gaining a `plots` field, etc.) and still renders/scrolls/decodes
  correctly — nothing on the TUI side regressed.
- **Not done, and can't be done in this sandbox**: actually clicking through
  the GUI on a real display (open a file, drag-and-drop, every window,
  filters/columns/plots editors, mark label popup, right-click menu, plot
  zoom/pan/hover). Every phase's notes above flag this same gap. Do this on a
  real macOS/Linux/Windows machine before treating the GUI as verified rather
  than "compiles and the core logic is unit-tested."

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
- Test: `cargo test` — expect **19 passed**. Core tests must not need a display.
- User-facing usage/build docs now live in `README.md` at the repo root — this
  file is for handoff/implementation state, README is for "how do I run this."
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
