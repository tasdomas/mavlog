//! Time alignment between a MAVLink `.tlog` (absolute unix time) and an
//! ArduPilot `.bin` (`TimeUS` = microseconds since boot) when both are merged
//! into one session.
//!
//! The bin's `TimeUS` and the autopilot's `SYSTEM_TIME.time_boot_ms` count from
//! the same boot, so a single `SYSTEM_TIME` message — which pairs boot time with
//! `time_unix_usec` — gives the offset that lifts the bin onto the tlog's
//! absolute axis.

use crate::core::entry::LogEntry;
use crate::tlog;

/// How a merged session's bin offset was determined (surfaced in status text).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncMethod {
    /// Derived from the tlog's `SYSTEM_TIME` messages.
    SystemTime,
    /// No usable signal found; the user aligns the logs by hand.
    ManualOnly,
}

/// Median boot→unix microsecond offset from a tlog's `SYSTEM_TIME` messages, or
/// `None` when the tlog carries none with a locked clock.
///
/// `offset = time_unix_usec - time_boot_ms * 1000`, so a bin record at boot time
/// `t` maps to unix time `t + offset`. Messages with a zero unix time (before
/// GPS/RTC lock) are ignored, and the median tolerates the odd bad sample.
pub fn boot_to_unix_offset(entries: &[LogEntry], data: &[u8]) -> Option<i64> {
    let mut offsets: Vec<i64> = entries
        .iter()
        .filter(|e| e.name == "SYSTEM_TIME")
        .filter_map(|e| {
            let msg = tlog::decode(data, e)?;
            let v = serde_json::to_value(msg).ok()?;
            let unix = v.get("time_unix_usec")?.as_u64()?;
            let boot_ms = v.get("time_boot_ms")?.as_u64()?;
            (unix != 0).then(|| unix as i64 - boot_ms as i64 * 1000)
        })
        .collect();
    if offsets.is_empty() {
        return None;
    }
    offsets.sort_unstable();
    Some(offsets[offsets.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tlog record: 8-byte big-endian unix-micros timestamp + a MAVLink frame.
    fn record(ts_us: u64, frame: &[u8]) -> Vec<u8> {
        let mut r = ts_us.to_be_bytes().to_vec();
        r.extend_from_slice(frame);
        r
    }

    /// A v2 SYSTEM_TIME (msg id 2) frame: payload is time_unix_usec(u64) then
    /// time_boot_ms(u32), little-endian. CRC bytes are not checked.
    fn system_time(unix_us: u64, boot_ms: u32) -> Vec<u8> {
        let mut payload = unix_us.to_le_bytes().to_vec();
        payload.extend(boot_ms.to_le_bytes());
        let mut frame = vec![0xFD, payload.len() as u8, 0, 0, 0, 1, 1, 2, 0, 0];
        frame.extend(payload);
        frame.extend([0, 0]); // crc
        frame
    }

    #[test]
    fn offset_from_system_time() {
        let mut data = record(10, &system_time(1_600_000_000_000_000, 5_000));
        // A second sample and a zero-unix (pre-lock) sample that must be ignored.
        data.extend(record(20, &system_time(1_600_000_001_000_000, 6_000)));
        data.extend(record(30, &system_time(0, 7_000)));
        let entries = tlog::parse(&data);

        let offset = boot_to_unix_offset(&entries, &data).unwrap();
        // Both valid samples agree: unix - boot_ms*1000.
        assert_eq!(offset, 1_600_000_000_000_000 - 5_000 * 1_000);
    }

    #[test]
    fn no_system_time_yields_none() {
        // A lone HEARTBEAT (id 0), no SYSTEM_TIME.
        let hb = [0xFD, 9, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 0, 0];
        let data = record(1, &hb);
        let entries = tlog::parse(&data);
        assert_eq!(boot_to_unix_offset(&entries, &data), None);
    }
}
