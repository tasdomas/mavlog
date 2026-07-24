mod core;
mod dataflash;
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

/// Read and parse a log file into a Session. Shared by both frontends: the
/// TUI needs a file up front, the GUI calls this again each time the user
/// opens or drops one. Picks the parser by extension, falling back to a
/// content sniff so mis-named or extension-less files still open.
pub(crate) fn load_session(path: &str) -> Result<Session> {
    let data = fs::read(path).with_context(|| format!("failed to read {path}"))?;
    let (entries, schema) = if is_dataflash(path, &data) {
        let (entries, schema) = dataflash::parse(&data);
        (entries, Some(schema))
    } else {
        (tlog::parse(&data), None)
    };
    if entries.is_empty() {
        bail!("no log messages found in {path}");
    }
    Ok(Session::new(path.to_string(), data, entries, schema))
}

/// Whether to parse `path` as an ArduPilot DataFlash `.bin` log: a `.bin`
/// extension, or the DataFlash record magic at the start of the file.
fn is_dataflash(path: &str, data: &[u8]) -> bool {
    let is_bin_ext = std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bin"));
    let is_tlog_ext = std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tlog"));
    is_bin_ext || (!is_tlog_ext && data.starts_with(&[0xA3, 0x95]))
}

#[cfg(test)]
mod tests {
    use super::is_dataflash;

    #[test]
    fn autodetects_format() {
        // Extension wins, regardless of contents.
        assert!(is_dataflash("log.bin", &[0xFD, 0, 0]));
        assert!(is_dataflash("LOG.BIN", &[]));
        assert!(!is_dataflash("flight.tlog", &[0xA3, 0x95]));
        // Otherwise sniff the DataFlash magic.
        assert!(is_dataflash("mystery", &[0xA3, 0x95, 0x80]));
        assert!(!is_dataflash("mystery", &[0xFD, 9, 0]));
    }
}
