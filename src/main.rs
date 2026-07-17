mod core;
mod tlog;
mod tui;

use std::{env, fs};

use anyhow::{bail, Context, Result};

use crate::core::session::Session;

fn main() -> Result<()> {
    let path = env::args()
        .nth(1)
        .context("usage: mavlog <file.tlog>")?;
    let data = fs::read(&path).with_context(|| format!("failed to read {path}"))?;
    let entries = tlog::parse(&data);
    if entries.is_empty() {
        bail!("no MAVLink messages found in {path}");
    }

    let session = Session::new(path, data, entries);
    tui::run(session)
}
