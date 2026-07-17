//! Frontend-agnostic domain logic shared by the TUI and GUI. Nothing in
//! here may depend on ratatui, crossterm or egui.

pub mod filter;
pub mod time;
