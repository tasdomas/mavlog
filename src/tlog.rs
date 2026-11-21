//! Parser for tlog files: a stream of records, each an 8-byte big-endian
//! microsecond timestamp followed by a raw MAVLink v1 or v2 frame.

use std::ops::Range;

use mavlink::{dialects::ardupilotmega::MavMessage, MavlinkVersion, Message};

use mavlink_tlog::{TlogSlice, Version};

use crate::core::entry::{EntryKind, LogEntry, LogSourceId};

/// Decode the full MAVLink message for an entry parsed from `data`. Returns
/// `None` for non-MAVLink entries or payloads the dialect can't parse.
pub fn decode(data: &[u8], entry: &LogEntry) -> Option<MavMessage> {
    let EntryKind::Mavlink { msg_id, version } = entry.kind else {
        return None;
    };
    MavMessage::parse(version, msg_id, &data[entry.payload.clone()]).ok()
}

/// Fallback detail-pane body for messages the dialect can't decode.
pub fn hex_dump(payload: &[u8]) -> String {
    let mut out = String::from("undecodable payload:\n");
    for (i, chunk) in payload.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        out.push_str(&format!("{:04x}  {:<47}  {ascii}\n", i * 16, hex.join(" ")));
    }
    out
}

/// Parse an entire tlog buffer, resyncing on corrupt data by sliding
/// forward one byte at a time until a plausible record is found.
pub fn parse(data: &[u8]) -> Vec<LogEntry> {
    let mut entries = Vec::new();
    let mut records = TlogSlice::new(data);

    while let Some(rec) = records.next() {
        let frame_start = records.position() - rec.frame.len();
        let (version, start) = match rec.frame.version() {
            Version::V1 => (MavlinkVersion::V1, 6),
            Version::V2 => (MavlinkVersion::V2, 10),
        };
        let payload = start..start + rec.frame.payload_len() as usize;
        let mut entry = entry(
            version,
            rec.frame.sys_id(),
            rec.frame.comp_id(),
            rec.frame.msg_id(),
            rec.frame.as_bytes(),
            payload,
        );
        entry.timestamp_us = rec.timestamp_us;
        // Rebase the payload range from frame-relative to buffer-relative.
        entry.payload.start += frame_start;
        entry.payload.end += frame_start;
        entries.push(entry);
    }
    entries
}

fn entry(
    version: MavlinkVersion,
    sysid: u8,
    compid: u8,
    msg_id: u32,
    frame: &[u8],
    payload: Range<usize>,
) -> LogEntry {
    let name = match MavMessage::parse(version, msg_id, &frame[payload.clone()]) {
        Ok(msg) => msg.message_name().to_string(),
        Err(_) => format!("UNKNOWN({msg_id})"),
    };
    LogEntry {
        timestamp_us: 0,
        sysid: Some(sysid),
        compid: Some(compid),
        payload,
        name,
        kind: EntryKind::Mavlink { msg_id, version },
        source: LogSourceId::Primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(timestamp_us: u64, frame: &[u8]) -> Vec<u8> {
        let mut rec = timestamp_us.to_be_bytes().to_vec();
        rec.extend_from_slice(frame);
        rec
    }

    // HEARTBEAT (id 0, 9-byte payload) frames with valid checksums.
    const V1_HEARTBEAT: &[u8] = &[
        0xFE, 9, 0, 1, 1, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 125, 221,
    ];
    const V2_HEARTBEAT: &[u8] = &[
        0xFD, 9, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 247, 144,
    ];

    #[test]
    fn parses_v1_and_v2_records() {
        let mut data = record(1_000_000, V1_HEARTBEAT);
        data.extend(record(2_000_000, V2_HEARTBEAT));

        let entries = parse(&data);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].timestamp_us, 1_000_000);
        assert_eq!(entries[0].name, "HEARTBEAT");
        assert_eq!(entries[1].timestamp_us, 2_000_000);
        assert_eq!(entries[1].name, "HEARTBEAT");
        assert_eq!(entries[1].sysid, Some(1));
    }

    #[test]
    fn decodes_payload_fields() {
        let data = record(1_000_000, V2_HEARTBEAT);
        let entries = parse(&data);
        let msg = decode(&data, &entries[0]).expect("decodes");
        let MavMessage::HEARTBEAT(hb) = msg else {
            panic!("expected HEARTBEAT, got {msg:?}");
        };
        assert_eq!(hb.mavlink_version, 3);
    }

    #[test]
    fn resyncs_after_garbage() {
        let mut data = vec![0xAA; 5];
        data.extend(record(1_000_000, V2_HEARTBEAT));

        let entries = parse(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp_us, 1_000_000);
    }

    #[test]
    fn unknown_message_id_is_kept() {
        let frame = [0xFD, 1, 0, 0, 0, 1, 1, 0xFF, 0xFF, 0xFE, 0, 0, 0];
        let entries = parse(&record(1, &frame));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "UNKNOWN(16711679)");
    }

    #[test]
    fn skips_frame_with_bad_crc() {
        let mut damaged = V2_HEARTBEAT.to_vec();
        damaged[14] ^= 0x01;
        let mut data = record(1_000_000, &damaged);
        data.extend(record(2_000_000, V2_HEARTBEAT));

        let entries = parse(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp_us, 2_000_000);
    }
}
