mod core;
mod gui;
mod tlog;
mod tui;

use std::{env, fs};

use anyhow::{bail, Context, Result};

use crate::core::session::Session;

fn main() -> Result<()> {
    let mut use_tui = false;
    let mut path: Option<String> = None;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "-tui" | "--tui" => use_tui = true,
            _ => path = Some(arg),
        }
    }

    // The file is optional for the GUI (it can open one later) but required
    // for the terminal UI.
    let session = match &path {
        Some(p) => Some(load_session(p)?),
        None => None,
    };

    if use_tui {
        let session =
            session.context("the terminal UI needs a file: mavlog -tui <file.tlog>")?;
        tui::run(session)
    } else {
        gui::run(session)
    }
}

/// Read and parse a tlog file into a Session.
fn load_session(path: &str) -> Result<Session> {
    let data = fs::read(path).with_context(|| format!("failed to read {path}"))?;
    let entries = tlog::parse(&data);
    if entries.is_empty() {
        bail!("no MAVLink messages found in {path}");
    }
    Ok(Session::new(path.to_string(), data, entries))
}
