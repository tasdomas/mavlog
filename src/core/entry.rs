//! The format-neutral log entry shared by the `tlog` (MAVLink) and
//! `dataflash` (ArduPilot `.bin`) parsers, and consumed by all of the
//! frontend-agnostic logic (filters, columns, plots, the session).
//!
//! An entry is just metadata: the bytes live in `Session::data` and `payload`
//! indexes into that shared buffer. `kind` carries the format-specific handle
//! needed to decode those bytes back into named fields.

use std::ops::Range;

use mavlink::MavlinkVersion;

pub struct LogEntry {
    pub timestamp_us: u64,
    /// MAVLink system/component ids. `None` for formats that have no such
    /// concept, such as ArduPilot DataFlash logs.
    pub sysid: Option<u8>,
    pub compid: Option<u8>,
    /// Byte range of the payload within the shared log buffer.
    pub payload: Range<usize>,
    pub name: String,
    pub kind: EntryKind,
    /// Which log a merged session drew this entry from. Always `Primary` for
    /// single-file sessions; a merge tags the second log's entries `Secondary`.
    pub source: LogSourceId,
}

/// Identifies which of a merged session's two logs an entry came from. Used to
/// disambiguate overlapping message-type names (both formats have `GPS`, `ATT`,
/// …) in the list and in source-scoped filters/columns/plots.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum LogSourceId {
    #[default]
    Primary,
    Secondary,
}

/// Format-specific data needed to decode an entry's payload.
#[derive(Clone, Copy)]
pub enum EntryKind {
    /// A MAVLink message; decoded via the `mavlink` dialect.
    Mavlink { msg_id: u32, version: MavlinkVersion },
    /// A DataFlash record; decoded via the log's embedded `FMT` schema, keyed
    /// by this message type id.
    Dataflash { msg_type: u8 },
}

impl LogEntry {
    /// A numeric id for display in the detail title: the MAVLink message id, or
    /// the DataFlash message type.
    pub fn msg_id(&self) -> u32 {
        match self.kind {
            EntryKind::Mavlink { msg_id, .. } => msg_id,
            EntryKind::Dataflash { msg_type } => msg_type as u32,
        }
    }

    /// The `sys:comp` id label, or empty when the format has no ids.
    pub fn id_label(&self) -> String {
        match (self.sysid, self.compid) {
            (Some(s), Some(c)) => format!("{s}:{c}"),
            _ => String::new(),
        }
    }
}
