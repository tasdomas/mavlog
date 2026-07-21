# mavlog

A viewer for MAVLink `.tlog` files: a scrollable, filterable message list
with a decoded detail pane, custom "last-seen value" columns, marks with
labels, jump-to-time, and interactive plots.

Two frontends share one core:

- **GUI** (default) — a native window built on [egui]/[eframe], for
  interactive plotting and mouse-driven browsing.
- **TUI** — a [ratatui] terminal UI with the same message list, filters,
  columns and marks, for headless/SSH use. Plotting is GUI-only.

[egui]: https://github.com/emilk/egui
[eframe]: https://github.com/emilk/egui/tree/master/crates/eframe
[ratatui]: https://ratatui.rs

## Usage

```sh
mavlog                  # GUI, empty — open a file from the toolbar or drag one in
mavlog FILE.tlog        # GUI, with FILE.tlog already loaded
mavlog -tui FILE.tlog   # terminal UI (a file is required)
```

Any per-file state you save (filters, columns, marks, time format, plots) is
written to a `FILE.tlog.mavlog.json` sidecar next to the log. It round-trips
between the GUI and the TUI — a setup saved in one loads correctly in the
other.

### GUI keyboard shortcuts

Every toolbar button has both a Cmd/Ctrl modifier shortcut (always active)
and, for TUI parity, a plain letter that only fires while no text box has
focus. Popups grab keyboard focus when they open, so Tab navigation starts
inside them immediately:

| Keys | Action |
| --- | --- |
| Ctrl/Cmd+O or `o` | Open a tlog file |
| Ctrl/Cmd+S or `w` | Save the setup sidecar |
| Ctrl/Cmd+J or `t` | Focus the jump-to-time box |
| Ctrl/Cmd+F or `f` | Add a new filter |
| Ctrl/Cmd+Shift+C or `c` | Toggle the Columns window |
| Ctrl/Cmd+P or `p` | Add a new plot |
| Ctrl/Cmd+, or `s` | Toggle Settings |
| F1 or `?` | Toggle the Help window |
| Ctrl/Cmd+N or `a` | Add a column, in the Columns window |
| Tab / Shift+Tab | Move focus between controls (stays inside an open popup) |
| Up / Down | Move the selection |
| Page Up / Page Down | Move the selection by a page |
| Home / End | Jump to the first / last message |
| Space | Toggle a mark on the selected message |
| Enter | Submit a box, save an open filter/column/plot editor, or dismiss an error |
| Esc | Release a text box's focus, then cancel an open editor, then close a window or panel |

Hovering any toolbar button shows its shortcut. Click a row to select it;
right-click for a mark context menu; drag a `.tlog` file onto the window to
open it. The in-app Help window (toolbar, or F1/`?`) lists the same
shortcuts.

### Filters

Press Ctrl/Cmd+F or `f` (or the toolbar's **Add filter**) to open the new-filter
popup and define one. The Filters side block, docked in the right column below
the message contents view, appears automatically whenever any filters are
configured and lists them; it hides itself once the last one is removed. Each
filter has a checkbox: untick it to disable the filter without removing it, and
the message list re-narrows immediately. A message is shown when it matches any
enabled filter (or when none are enabled). Disabled filters are remembered in
the saved setup, written with a leading `!` in the filter text (e.g.
`!1:1 =HEARTBEAT`).

### Plots

Press Ctrl/Cmd+P or `p` (or the toolbar's **Add plot**) to open the plot
dialog and define a plot — a name and one or more fields graphed against time.
Configured plots are listed in a sidebar in the right column, below the
message contents view; tick a plot's **Show** box to open it in its own movable
window, which updates as you browse.
Each plot can optionally overlay vertical lines at your marks — toggle "Show
markers" in the dialog. Zoom, pan and hover come from the plot widget itself.

## Building

```sh
cargo build --release
```

The first build compiles the full egui/eframe/wgpu stack and is slow
(~1-2 minutes); subsequent builds are incremental.

### Linux build dependencies

The GUI backend (via `eframe`/`winit`) needs `libxkbcommon` and either X11 or
Wayland development libraries; the native file-open dialog (via `rfd`) needs
either GTK3 or a working XDG desktop portal, depending on your desktop
environment. On Debian/Ubuntu, something like:

```sh
sudo apt install libxkbcommon-dev libx11-dev libgtk-3-dev
```

macOS and Windows need no extra system packages.

## Testing

```sh
cargo test
```

All domain logic (parsing, filters, columns, marks, plot extraction and
decimation, setup persistence) lives in `core/` and is unit-tested without
needing a display. The TUI can be smoke-tested headlessly via `tmux`; the
GUI needs a real display to exercise interactively.
