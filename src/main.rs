mod core;
mod dataflash;
mod gui;
mod tlog;
mod tui;

use std::{env, fs};

use anyhow::{bail, Context, Result};

use crate::core::session::Session;
use crate::core::sync::{self, SyncMethod};

fn main() -> Result<()> {
    let mut use_tui = false;
    let mut paths: Vec<String> = Vec::new();
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "-tui" | "--tui" => use_tui = true,
            _ => paths.push(arg),
        }
    }

    // The file is optional for the GUI (it can open one later) but required
    // for the terminal UI. Two files (one .tlog + one .bin) merge into a single
    // time-synchronized session.
    let session = match paths.as_slice() {
        [] => None,
        [p] => Some(load_session(p)?),
        [a, b] => Some(load_merged_session(a, b)?),
        _ => bail!("expected at most two log files, got {}", paths.len()),
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

/// Load one MAVLink `.tlog` and one ArduPilot `.bin` (in either argument
/// order) into a single time-synchronized session. The bin is aligned onto the
/// tlog's absolute clock via the tlog's `SYSTEM_TIME` messages; without those
/// the logs load unshifted and the user aligns them by hand.
pub(crate) fn load_merged_session(a: &str, b: &str) -> Result<Session> {
    let data_a = fs::read(a).with_context(|| format!("failed to read {a}"))?;
    let data_b = fs::read(b).with_context(|| format!("failed to read {b}"))?;
    // Normalize to (tlog, bin) regardless of argument order.
    let (tlog_path, tlog_data, bin_path, bin_data) = match (is_dataflash(a, &data_a), is_dataflash(b, &data_b)) {
        (false, true) => (a, data_a, b, data_b),
        (true, false) => (b, data_b, a, data_a),
        _ => bail!("merging logs needs exactly one .tlog and one .bin"),
    };

    let tlog_entries = tlog::parse(&tlog_data);
    let (bin_entries, bin_schema) = dataflash::parse(&bin_data);
    if tlog_entries.is_empty() || bin_entries.is_empty() {
        bail!("no log messages found in {tlog_path} or {bin_path}");
    }

    let (offset, method) = match sync::boot_to_unix_offset(&tlog_entries, &tlog_data) {
        Some(o) => (o, SyncMethod::SystemTime),
        None => (0, SyncMethod::ManualOnly),
    };
    Ok(Session::merge(
        tlog_path.to_string(),
        tlog_data,
        tlog_entries,
        bin_data,
        bin_entries,
        bin_schema,
        offset,
        method,
    ))
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
    use super::{is_dataflash, load_merged_session, load_session};
    use crate::core::entry::LogSourceId;
    use crate::core::sync::SyncMethod;
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

    #[test]
    fn merges_tlog_and_bin_time_synchronized() {
        let t0: u64 = 1_600_000_000_000_000;

        // --- tlog: SYSTEM_TIME (offset anchor) + two HEARTBEATs ---
        let tlog_record = |ts: u64, frame: &[u8]| {
            let mut r = ts.to_be_bytes().to_vec();
            r.extend_from_slice(frame);
            r
        };
        // SYSTEM_TIME (id 2): time_unix_usec=t0, time_boot_ms=0 -> offset = t0.
        let mut st_payload = t0.to_le_bytes().to_vec();
        st_payload.extend(0u32.to_le_bytes());
        let mut st = vec![0xFD, st_payload.len() as u8, 0, 0, 0, 1, 1, 2, 0, 0];
        st.extend(&st_payload);
        st.extend([0, 0]);
        let hb: &[u8] = &[0xFD, 9, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 0, 0];
        let mut tlog = tlog_record(t0, &st);
        tlog.extend(tlog_record(t0, hb));
        tlog.extend(tlog_record(t0 + 2_000_000, hb));

        // --- bin: FMT-of-FMT + ATT FMT + one ATT at boot 1_000_000 ---
        let brec = |t: u8, body: &[u8]| {
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
        let mut bin = brec(0x80, &fmt(0x80, 89, "FMT", "BBnNZ", "Type,Length,Name,Format,Columns"));
        bin.extend(brec(0x80, &fmt(2, 15, "ATT", "Qf", "TimeUS,Roll")));
        let mut att = 1_000_000u64.to_le_bytes().to_vec();
        att.extend(1.0f32.to_le_bytes());
        bin.extend(brec(2, &att));

        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let tpath = dir.join(format!("mavlog-merge-{pid}.tlog"));
        let bpath = dir.join(format!("mavlog-merge-{pid}.bin"));
        std::fs::write(&tpath, &tlog).unwrap();
        std::fs::write(&bpath, &bin).unwrap();
        // Argument order should not matter.
        let session = load_merged_session(bpath.to_str().unwrap(), tpath.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&tpath);
        let _ = std::fs::remove_file(&bpath);

        // Aligned via SYSTEM_TIME; both logs now on the absolute axis.
        assert_eq!(session.time_format, TimeFormat::DateTime);
        assert_eq!(session.sync.as_ref().unwrap().method, SyncMethod::SystemTime);

        // Timestamps are globally ascending after the merge sort.
        let ts: Vec<u64> = session.entries.iter().map(|e| e.timestamp_us).collect();
        assert!(ts.windows(2).all(|w| w[0] <= w[1]));

        // The ATT record (boot 1_000_000 + offset t0) interleaves between the
        // t0 group and the t0+2s HEARTBEAT, and decodes across the buffer join.
        let att = session.entries.iter().find(|e| e.name == "ATT").unwrap();
        assert_eq!(att.source, LogSourceId::Secondary);
        assert_eq!(att.timestamp_us, t0 + 1_000_000);
        assert!((session.decode_fields(att).unwrap()["Roll"].as_f64().unwrap() - 1.0).abs() < 1e-6);

        // A tlog entry still decodes via the mavlink dialect.
        let hb_entry = session.entries.iter().find(|e| e.name == "HEARTBEAT").unwrap();
        assert_eq!(hb_entry.source, LogSourceId::Primary);
        assert_eq!(session.decode_fields(hb_entry).unwrap()["mavlink_version"], 3);
    }
}
