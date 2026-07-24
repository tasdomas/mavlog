//! Parser for ArduPilot DataFlash `.bin` logs.
//!
//! The format is self-describing: every record is `0xA3 0x95 <type:u8>`
//! followed by a fixed-length body. `FMT` records (type 128) declare each
//! other type's name, field labels and binary layout, and `FMTU`/`UNIT`/`MULT`
//! records refine per-field multipliers and unit labels. We parse the `FMT`
//! records into a [`Schema`], resolve units/multipliers from the metadata
//! records, then decode any record on demand via that schema.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::core::entry::{EntryKind, LogEntry};

const HEAD1: u8 = 0xA3;
const HEAD2: u8 = 0x95;
const FMT_TYPE: u8 = 0x80; // 128
/// `FMT` is fixed: 3-byte header + Type(1) + Length(1) + Name(4) + Format(16)
/// + Columns(64).
const FMT_LEN: usize = 89;

/// One field within a message type.
pub struct FieldFmt {
    pub label: String,
    pub code: u8,
    /// Byte offset within the record payload (i.e. after the 3-byte header).
    pub offset: usize,
    pub size: usize,
    /// Effective multiplier applied to the raw value (from `FMTU`/`MULT` when
    /// present, otherwise the format char's built-in scale).
    pub multiplier: f64,
    /// Unit label from `FMTU`/`UNIT`, if any.
    pub unit: Option<String>,
}

/// The layout of one DataFlash message type.
pub struct MsgFmt {
    pub name: String,
    pub fields: Vec<FieldFmt>,
    /// Total record length including the 3-byte header.
    pub length: u8,
}

impl MsgFmt {
    fn field(&self, label: &str) -> Option<&FieldFmt> {
        self.fields.iter().find(|f| f.label == label)
    }
}

/// Message layouts parsed from a log's `FMT` records, keyed by type id.
pub struct Schema {
    msgs: HashMap<u8, MsgFmt>,
}

impl Schema {
    pub fn get(&self, msg_type: u8) -> Option<&MsgFmt> {
        self.msgs.get(&msg_type)
    }

    /// The unit label for a field, looked up by message and field name
    /// (case-insensitive). `None` when unknown or the field has no unit.
    pub fn field_unit(&self, msg_name: &str, field: &str) -> Option<String> {
        let fmt = self.msgs.values().find(|m| m.name.eq_ignore_ascii_case(msg_name))?;
        fmt.fields
            .iter()
            .find(|f| f.label.eq_ignore_ascii_case(field))?
            .unit
            .clone()
    }
}

/// Byte width of a format code, or `None` if unrecognised.
fn code_size(code: u8) -> Option<usize> {
    Some(match code {
        b'b' | b'B' | b'M' => 1,
        b'h' | b'H' | b'c' | b'C' => 2,
        b'i' | b'I' | b'f' | b'e' | b'E' | b'L' => 4,
        b'd' | b'q' | b'Q' => 8,
        b'n' => 4,
        b'N' => 16,
        b'Z' => 64,
        b'a' => 64, // int16[32]
        _ => return None,
    })
}

/// The multiplier a format code implies on its own, used when no `FMTU`
/// overrides it.
fn code_scale(code: u8) -> f64 {
    match code {
        b'c' | b'C' | b'e' | b'E' => 0.01,
        b'L' => 1e-7,
        _ => 1.0,
    }
}

/// Parse a whole `.bin` buffer into entries plus the schema needed to decode
/// them. Resyncs a byte at a time on anything that isn't a valid header.
pub fn parse(data: &[u8]) -> (Vec<LogEntry>, Schema) {
    let mut msgs: HashMap<u8, MsgFmt> = HashMap::new();
    let mut entries = Vec::new();

    // Metadata gathered during the pass, resolved into the schema afterwards.
    let mut units: HashMap<u8, String> = HashMap::new();
    let mut mults: HashMap<u8, f64> = HashMap::new();
    let mut fmtus: HashMap<u8, (Vec<u8>, Vec<u8>)> = HashMap::new(); // type -> (unit ids, mult ids)

    let mut last_time_us = 0u64;
    let mut pos = 0usize;

    while pos + 3 <= data.len() {
        if data[pos] != HEAD1 || data[pos + 1] != HEAD2 {
            pos += 1;
            continue;
        }
        let msg_type = data[pos + 2];

        if msg_type == FMT_TYPE {
            if pos + FMT_LEN > data.len() {
                break; // truncated FMT at the tail
            }
            let body = &data[pos + 3..pos + FMT_LEN];
            if let Some(fmt) = parse_fmt(body) {
                msgs.insert(fmt_defined_type(body), fmt);
            }
            entries.push(LogEntry {
                timestamp_us: last_time_us,
                sysid: None,
                compid: None,
                payload: pos + 3..pos + FMT_LEN,
                name: "FMT".to_string(),
                kind: EntryKind::Dataflash { msg_type },
            });
            pos += FMT_LEN;
            continue;
        }

        let Some(fmt) = msgs.get(&msg_type) else {
            // Unknown type (its FMT hasn't been seen): length unknown, so we
            // can't skip the body reliably. Resync and let the next header
            // recover us.
            pos += 1;
            continue;
        };
        let length = fmt.length as usize;
        if length < 3 || pos + length > data.len() {
            break; // malformed length or truncated tail
        }
        let payload = &data[pos + 3..pos + length];

        // Collect unit/mult/FMTU metadata as we encounter it.
        match fmt.name.as_str() {
            "UNIT" => {
                if let (Some(id), Some(label)) = (u8_field(fmt, payload, "Id"), str_field(fmt, payload, "Label")) {
                    units.insert(id, label);
                }
            }
            "MULT" => {
                if let (Some(id), Some(m)) = (u8_field(fmt, payload, "Id"), f64_field(fmt, payload, "Mult")) {
                    mults.insert(id, m);
                }
            }
            "FMTU" => {
                if let (Some(t), Some(u), Some(m)) = (
                    u8_field(fmt, payload, "FmtType"),
                    bytes_field(fmt, payload, "UnitIds"),
                    bytes_field(fmt, payload, "MultIds"),
                ) {
                    fmtus.insert(t, (u, m));
                }
            }
            _ => {}
        }

        let timestamp_us = u64_field(fmt, payload, "TimeUS").unwrap_or(last_time_us);
        last_time_us = timestamp_us;
        entries.push(LogEntry {
            timestamp_us,
            sysid: None,
            compid: None,
            payload: pos + 3..pos + length,
            name: fmt.name.clone(),
            kind: EntryKind::Dataflash { msg_type },
        });
        pos += length;
    }

    resolve_units_and_mults(&mut msgs, &units, &mults, &fmtus);
    (entries, Schema { msgs })
}

/// Apply the `FMTU`/`UNIT`/`MULT` metadata onto each field. When a type has a
/// `FMTU` entry the referenced multiplier is authoritative (so we don't also
/// apply the format char's built-in scale); otherwise the built-in scale set
/// at parse time stands.
fn resolve_units_and_mults(
    msgs: &mut HashMap<u8, MsgFmt>,
    units: &HashMap<u8, String>,
    mults: &HashMap<u8, f64>,
    fmtus: &HashMap<u8, (Vec<u8>, Vec<u8>)>,
) {
    for (msg_type, (unit_ids, mult_ids)) in fmtus {
        let Some(fmt) = msgs.get_mut(msg_type) else {
            continue;
        };
        for (i, field) in fmt.fields.iter_mut().enumerate() {
            if let Some(&mid) = mult_ids.get(i) {
                field.multiplier = mults.get(&mid).copied().unwrap_or(1.0);
            }
            if let Some(&uid) = unit_ids.get(i) {
                field.unit = units.get(&uid).filter(|l| !l.is_empty()).cloned();
            }
        }
    }
}

/// The type id an `FMT` body declares (its first field).
fn fmt_defined_type(body: &[u8]) -> u8 {
    body[0]
}

/// Parse an `FMT` record body (everything after the 3-byte header) into a
/// [`MsgFmt`]. Layout: Type(1) Length(1) Name(4) Format(16) Columns(64).
fn parse_fmt(body: &[u8]) -> Option<MsgFmt> {
    if body.len() < 86 {
        return None;
    }
    let length = body[1];
    let name = trim_str(&body[2..6]);
    let format = &body[6..22];
    let labels = trim_str(&body[22..86]);
    let codes: Vec<u8> = format.iter().copied().take_while(|&c| c != 0).collect();
    let labels: Vec<&str> = labels.split(',').filter(|s| !s.is_empty()).collect();
    if codes.len() != labels.len() {
        return None; // malformed FMT; skip it rather than mis-decode
    }

    let mut fields = Vec::with_capacity(codes.len());
    let mut offset = 0usize;
    for (&code, label) in codes.iter().zip(labels) {
        let size = code_size(code)?;
        fields.push(FieldFmt {
            label: label.to_string(),
            code,
            offset,
            size,
            multiplier: code_scale(code),
            unit: None,
        });
        offset += size;
    }
    Some(MsgFmt { name, fields, length })
}

/// Read a field's raw payload bytes by label.
fn field_bytes<'a>(fmt: &MsgFmt, payload: &'a [u8], label: &str) -> Option<&'a [u8]> {
    let f = fmt.field(label)?;
    payload.get(f.offset..f.offset + f.size)
}

fn u8_field(fmt: &MsgFmt, payload: &[u8], label: &str) -> Option<u8> {
    field_bytes(fmt, payload, label).map(|b| b[0])
}

fn u64_field(fmt: &MsgFmt, payload: &[u8], label: &str) -> Option<u64> {
    let b = field_bytes(fmt, payload, label)?;
    Some(u64::from_le_bytes(b.try_into().ok()?))
}

fn f64_field(fmt: &MsgFmt, payload: &[u8], label: &str) -> Option<f64> {
    let b = field_bytes(fmt, payload, label)?;
    Some(f64::from_le_bytes(b.try_into().ok()?))
}

fn str_field(fmt: &MsgFmt, payload: &[u8], label: &str) -> Option<String> {
    field_bytes(fmt, payload, label).map(trim_str)
}

fn bytes_field(fmt: &MsgFmt, payload: &[u8], label: &str) -> Option<Vec<u8>> {
    field_bytes(fmt, payload, label).map(|b| b.to_vec())
}

/// A NUL-terminated, NUL-padded fixed field as a `String` (lossy UTF-8).
fn trim_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Decode a DataFlash record's payload into a flat `field -> value` object,
/// applying each field's resolved multiplier and rendering char arrays as
/// strings. Returns `None` if the type is unknown.
pub fn decode_fields(schema: &Schema, msg_type: u8, payload: &[u8]) -> Option<Value> {
    let fmt = schema.get(msg_type)?;
    let mut map = Map::new();
    for f in &fmt.fields {
        let Some(bytes) = payload.get(f.offset..f.offset + f.size) else {
            break; // truncated record
        };
        map.insert(f.label.clone(), decode_field(f.code, bytes, f.multiplier));
    }
    Some(Value::Object(map))
}

/// Render a record as `Label: value unit` lines in field order, for the
/// detail pane. Returns `None` if the type is unknown.
pub fn decode_detail(schema: &Schema, msg_type: u8, payload: &[u8]) -> Option<String> {
    let fmt = schema.get(msg_type)?;
    let mut out = String::new();
    for f in &fmt.fields {
        let Some(bytes) = payload.get(f.offset..f.offset + f.size) else {
            break; // truncated record
        };
        let value = match decode_field(f.code, bytes, f.multiplier) {
            Value::String(s) => s,
            other => other.to_string(),
        };
        let unit = f.unit.as_deref().map(|u| format!(" {u}")).unwrap_or_default();
        out.push_str(&format!("{}: {value}{unit}\n", f.label));
    }
    Some(out)
}

fn decode_field(code: u8, b: &[u8], mult: f64) -> Value {
    let i = |v: i64| int_value(v, mult);
    match code {
        b'b' => i(b[0] as i8 as i64),
        b'B' | b'M' => i(b[0] as i64),
        b'h' | b'c' => i(i16::from_le_bytes([b[0], b[1]]) as i64),
        b'H' | b'C' => i(u16::from_le_bytes([b[0], b[1]]) as i64),
        b'i' | b'e' => i(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64),
        b'I' | b'E' => i(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64),
        b'L' => Value::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 * mult),
        b'f' => Value::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64),
        b'd' => Value::from(f64::from_le_bytes(b.try_into().unwrap())),
        b'q' => i(i64::from_le_bytes(b.try_into().unwrap())),
        b'Q' => {
            let v = u64::from_le_bytes(b.try_into().unwrap());
            if mult == 1.0 { Value::from(v) } else { Value::from(v as f64 * mult) }
        }
        b'n' | b'N' | b'Z' => Value::String(trim_str(b)),
        b'a' => Value::Array(
            b.chunks_exact(2)
                .map(|c| int_value(i16::from_le_bytes([c[0], c[1]]) as i64, mult))
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// An integer value, kept integral when unscaled and promoted to float when a
/// multiplier applies.
fn int_value(v: i64, mult: f64) -> Value {
    if mult == 1.0 {
        Value::from(v)
    } else {
        Value::from(v as f64 * mult)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a DataFlash record: `A3 95 type` + body bytes.
    fn record(msg_type: u8, body: &[u8]) -> Vec<u8> {
        let mut r = vec![HEAD1, HEAD2, msg_type];
        r.extend_from_slice(body);
        r
    }

    /// A fixed-width NUL-padded field.
    fn fixed(s: &str, width: usize) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.resize(width, 0);
        v
    }

    /// An `FMT` record body defining `msg_type` with the given name/format/labels.
    fn fmt_body(def_type: u8, length: u8, name: &str, format: &str, labels: &str) -> Vec<u8> {
        let mut b = vec![def_type, length];
        b.extend(fixed(name, 4));
        b.extend(fixed(format, 16));
        b.extend(fixed(labels, 64));
        b
    }

    /// The self-describing FMT-of-FMT record every log opens with.
    fn fmt_of_fmt() -> Vec<u8> {
        record(FMT_TYPE, &fmt_body(FMT_TYPE, FMT_LEN as u8, "FMT", "BBnNZ", "Type,Length,Name,Format,Columns"))
    }

    #[test]
    fn parses_fmt_and_defines_schema() {
        let mut data = fmt_of_fmt();
        // Define GPS: TimeUS(Q), Status(B), Lat(L), Alt(f) -> length 3+8+1+4+4 = 20
        data.extend(record(FMT_TYPE, &fmt_body(2, 20, "GPS", "QBLf", "TimeUS,Status,Lat,Alt")));

        let (entries, schema) = parse(&data);
        assert_eq!(entries.len(), 2); // both FMT records are viewable
        assert!(entries.iter().all(|e| e.name == "FMT"));
        let gps = schema.get(2).expect("GPS defined");
        assert_eq!(gps.name, "GPS");
        assert_eq!(gps.length, 20);
        let labels: Vec<&str> = gps.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["TimeUS", "Status", "Lat", "Alt"]);
        // Offsets are consecutive within the payload.
        assert_eq!(gps.field("Lat").unwrap().offset, 9); // 8 (TimeUS) + 1 (Status)
    }

    #[test]
    fn decodes_message_with_timestamp_and_scaling() {
        let mut data = fmt_of_fmt();
        data.extend(record(FMT_TYPE, &fmt_body(2, 20, "GPS", "QBLf", "TimeUS,Status,Lat,Alt")));
        // One GPS record: TimeUS=5_000_000, Status=3, Lat=-350000000 (=-35.0 deg), Alt=12.5
        let mut body = 5_000_000u64.to_le_bytes().to_vec();
        body.push(3);
        body.extend((-350_000_000i32).to_le_bytes());
        body.extend(12.5f32.to_le_bytes());
        data.extend(record(2, &body));

        let (entries, schema) = parse(&data);
        let gps = entries.iter().find(|e| e.name == "GPS").expect("a GPS entry");
        assert_eq!(gps.timestamp_us, 5_000_000);
        assert_eq!(gps.sysid, None);

        let payload = &data[gps.payload.clone()];
        let v = decode_fields(&schema, 2, payload).unwrap();
        assert_eq!(v["Status"], serde_json::json!(3));
        // 'L' applies the built-in 1e-7 lat/lng scale.
        assert!((v["Lat"].as_f64().unwrap() - (-35.0)).abs() < 1e-9);
        assert!((v["Alt"].as_f64().unwrap() - 12.5).abs() < 1e-6);
    }

    #[test]
    fn fmtu_multiplier_and_unit_override() {
        // BARO.Alt stored as int32 'i' but scaled by MULT '2' (=0.01) and unit "m".
        let mut data = fmt_of_fmt();
        data.extend(record(FMT_TYPE, &fmt_body(3, 15, "BARO", "Qi", "TimeUS,Alt")));
        data.extend(record(FMT_TYPE, &fmt_body(4, 76, "UNIT", "QbZ", "TimeUS,Id,Label")));
        data.extend(record(FMT_TYPE, &fmt_body(5, 20, "MULT", "Qbd", "TimeUS,Id,Mult")));
        data.extend(record(FMT_TYPE, &fmt_body(6, 44, "FMTU", "QBNN", "TimeUS,FmtType,UnitIds,MultIds")));

        // UNIT 'm' = "m"
        let mut u = 0u64.to_le_bytes().to_vec();
        u.push(b'm');
        u.extend(fixed("m", 64));
        data.extend(record(4, &u));
        // MULT '2' = 0.01
        let mut m = 0u64.to_le_bytes().to_vec();
        m.push(b'2');
        m.extend(0.01f64.to_le_bytes());
        data.extend(record(5, &m));
        // FMTU for BARO (type 3): UnitIds "-m", MultIds "-2" (field 0 TimeUS none, field 1 Alt scaled)
        let mut f = 0u64.to_le_bytes().to_vec();
        f.push(3);
        f.extend(fixed("-m", 16));
        f.extend(fixed("-2", 16));
        data.extend(record(6, &f));

        // A BARO record: TimeUS=1_000_000, Alt=1234 -> 12.34 m
        let mut body = 1_000_000u64.to_le_bytes().to_vec();
        body.extend(1234i32.to_le_bytes());
        data.extend(record(3, &body));

        let (entries, schema) = parse(&data);
        let baro = entries.iter().find(|e| e.name == "BARO").unwrap();
        let v = decode_fields(&schema, 3, &data[baro.payload.clone()]).unwrap();
        assert!((v["Alt"].as_f64().unwrap() - 12.34).abs() < 1e-9);
        let alt = schema.get(3).unwrap().field("Alt").unwrap();
        assert_eq!(alt.unit.as_deref(), Some("m"));
        // Looked up by name (case-insensitive), as the plot editor does.
        assert_eq!(schema.field_unit("baro", "alt").as_deref(), Some("m"));
    }

    #[test]
    fn missing_timeus_carries_forward() {
        let mut data = fmt_of_fmt();
        data.extend(record(FMT_TYPE, &fmt_body(2, 15, "ATT", "Qf", "TimeUS,Roll")));
        data.extend(record(FMT_TYPE, &fmt_body(3, 6, "MODE", "Bh", "Mode,Reason"))); // no TimeUS
        // ATT at t=2_000_000
        let mut att = 2_000_000u64.to_le_bytes().to_vec();
        att.extend(1.0f32.to_le_bytes());
        data.extend(record(2, &att));
        // MODE with no timestamp inherits the last-seen time
        data.extend(record(3, &[1, 0, 0]));

        let (entries, _) = parse(&data);
        let mode = entries.iter().find(|e| e.name == "MODE").unwrap();
        assert_eq!(mode.timestamp_us, 2_000_000);
    }

    #[test]
    fn resyncs_and_tolerates_truncated_tail() {
        let mut data = vec![0x00, 0xFF]; // leading garbage
        data.extend(fmt_of_fmt());
        data.extend(record(FMT_TYPE, &fmt_body(2, 15, "ATT", "Qf", "TimeUS,Roll")));
        let mut att = 7u64.to_le_bytes().to_vec();
        att.extend(1.0f32.to_le_bytes());
        data.extend(record(2, &att));
        data.extend_from_slice(&[HEAD1, HEAD2, 2, 0x01]); // truncated final record

        let (entries, _) = parse(&data);
        assert_eq!(entries.iter().filter(|e| e.name == "ATT").count(), 1);
    }
}
