//! Parser for tlog files: a stream of records, each an 8-byte big-endian
//! microsecond timestamp followed by a raw MAVLink v1 or v2 frame.

use std::ops::Range;

use mavlink::{dialects::ardupilotmega::MavMessage, error::ParserError, MavlinkVersion, Message};

pub const MAGIC_V1: u8 = 0xFE;
pub const MAGIC_V2: u8 = 0xFD;

pub struct LogEntry {
    pub timestamp_us: u64,
    pub sysid: u8,
    pub compid: u8,
    pub msg_id: u32,
    pub version: MavlinkVersion,
    /// Byte range of the payload within the log buffer.
    pub payload: Range<usize>,
    pub name: String,
}

/// Decode the full message for an entry parsed from `data`.
pub fn decode(data: &[u8], entry: &LogEntry) -> Result<MavMessage, ParserError> {
    MavMessage::parse(entry.version, entry.msg_id, &data[entry.payload.clone()])
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
    let mut pos = 0usize;

    while pos + 8 < data.len() {
        let frame = &data[pos + 8..];
        let parsed = match frame[0] {
            MAGIC_V1 => parse_v1(frame),
            MAGIC_V2 => parse_v2(frame),
            _ => None,
        };
        let Some((mut entry, frame_len)) = parsed else {
            pos += 1;
            continue;
        };
        entry.timestamp_us = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
        // Rebase the payload range from frame-relative to buffer-relative.
        entry.payload.start += pos + 8;
        entry.payload.end += pos + 8;
        entries.push(entry);
        pos += 8 + frame_len;
    }
    entries
}

/// v1 frame: STX len seq sys comp msgid payload[len] crc[2]
fn parse_v1(frame: &[u8]) -> Option<(LogEntry, usize)> {
    if frame.len() < 8 {
        return None;
    }
    let len = frame[1] as usize;
    let total = 6 + len + 2;
    if frame.len() < total {
        return None;
    }
    let msg_id = frame[5] as u32;
    Some((
        entry(MavlinkVersion::V1, frame[3], frame[4], msg_id, frame, 6..6 + len),
        total,
    ))
}

/// v2 frame: STX len incompat compat seq sys comp msgid[3] payload[len] crc[2] [sig[13]]
fn parse_v2(frame: &[u8]) -> Option<(LogEntry, usize)> {
    if frame.len() < 12 {
        return None;
    }
    let len = frame[1] as usize;
    let signed = frame[2] & 0x01 != 0;
    let total = 10 + len + 2 + if signed { 13 } else { 0 };
    if frame.len() < total {
        return None;
    }
    let msg_id = u32::from_le_bytes([frame[7], frame[8], frame[9], 0]);
    Some((
        entry(MavlinkVersion::V2, frame[5], frame[6], msg_id, frame, 10..10 + len),
        total,
    ))
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
        sysid,
        compid,
        msg_id,
        version,
        payload,
        name,
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

    // HEARTBEAT (id 0, 9-byte payload) frames; CRC bytes are not checked.
    const V1_HEARTBEAT: &[u8] = &[
        0xFE, 9, 0, 1, 1, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 0, 0,
    ];
    const V2_HEARTBEAT: &[u8] = &[
        0xFD, 9, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 0, 0,
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
        assert_eq!(entries[1].sysid, 1);
    }

    #[test]
    fn decodes_payload_fields() {
        let data = record(1_000_000, V2_HEARTBEAT);
        let entries = parse(&data);
        let msg = decode(&data, &entries[0]).unwrap();
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
}
