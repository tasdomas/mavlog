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

| Keys | Action |
| --- | --- |
| Ctrl/Cmd+O | Open a tlog file |
| Ctrl/Cmd+S | Save the setup sidecar |
| Up / Down | Move the selection |
| Page Up / Page Down | Move the selection by a page |
| Home / End | Jump to the first / last message |
| Space | Toggle a mark on the selected message |
| Enter | Submit the jump-to-time or label box |
| Esc | Close the frontmost window |

Click a row to select it; right-click for a mark context menu; drag a
`.tlog` file onto the window to open it. The in-app Help window (toolbar)
lists the same shortcuts.

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
