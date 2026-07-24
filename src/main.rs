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
            session.context("the terminal UI needs a file: mavlog -tui <file.tlog|.bin>")?;
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
    use super::{is_dataflash, load_session};
    use crate::core::time::TimeFormat;

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

    #[test]
    fn loads_a_dataflash_file_end_to_end() {
        // A minimal .bin: FMT-of-FMT, a GPS FMT, and one GPS record.
        let rec = |t: u8, body: &[u8]| {
            let mut r = vec![0xA3, 0x95, t];
            r.extend_from_slice(body);
            r
        };
        let fixed = |s: &str, w: usize| {
            let mut v = s.as_bytes().to_vec();
            v.resize(w, 0);
            v
        };
        let fmt = |t: u8, len: u8, name: &str, format: &str, labels: &str| {
            let mut b = vec![t, len];
            b.extend(fixed(name, 4));
            b.extend(fixed(format, 16));
            b.extend(fixed(labels, 64));
            b
        };

        let mut data = rec(0x80, &fmt(0x80, 89, "FMT", "BBnNZ", "Type,Length,Name,Format,Columns"));
        data.extend(rec(0x80, &fmt(2, 20, "GPS", "QBLf", "TimeUS,Status,Lat,Alt")));
        let mut gps = 1_000_000u64.to_le_bytes().to_vec();
        gps.push(3);
        gps.extend((-350_000_000i32).to_le_bytes());
        gps.extend(12.5f32.to_le_bytes());
        data.extend(rec(2, &gps));

        let path = std::env::temp_dir().join(format!("mavlog-e2e-{}.bin", std::process::id()));
        std::fs::write(&path, &data).unwrap();
        let session = load_session(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);

        // Detected as DataFlash: relative time, no ids, GPS type present.
        assert_eq!(session.time_format, TimeFormat::OffsetSecs);
        assert!(session.id_options.is_empty());
        assert!(session.type_options.iter().any(|t| t == "GPS"));

        let gps_entry = session.entries.iter().find(|e| e.name == "GPS").unwrap();
        assert_eq!(gps_entry.timestamp_us, 1_000_000);
        let v = session.decode_fields(gps_entry).unwrap();
        assert!((v["Lat"].as_f64().unwrap() - (-35.0)).abs() < 1e-9);
        assert!((v["Alt"].as_f64().unwrap() - 12.5).abs() < 1e-6);
    }
}
