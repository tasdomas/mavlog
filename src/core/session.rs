//! The `Session`: all per-file domain state and the operations on it, shared
//! by the TUI and GUI frontends. It knows nothing about how it is displayed.

use std::collections::HashMap;

use crate::core::column::{parse_columns, CustomColumn};
use crate::core::entry::{EntryKind, LogEntry, LogSourceId};
use crate::core::filter::{parse_filters, FilterExpr};
use crate::core::plot::PlotDef;
use crate::core::setup::{setup_path, Setup};
use crate::core::sync::SyncMethod;
use crate::core::time::{format_datetime, format_offset, TimeFormat};
use crate::{dataflash, tlog};

pub struct Session {
    pub path: String,
    pub data: Vec<u8>,
    pub entries: Vec<LogEntry>,
    /// Timestamp of the first message; offsets are relative to it.
    pub start_us: u64,
    pub time_format: TimeFormat,
    pub filters: Vec<FilterExpr>,
    /// Raw filter text, kept so the raw-edit prompt can reopen it.
    pub filter_text: String,
    /// Indices into `entries` that pass the current filter.
    pub filtered: Vec<usize>,
    pub columns: Vec<CustomColumn>,
    /// Raw column definitions, kept for re-editing.
    pub columns_text: String,
    /// Marked messages by entry index; the value is an optional label.
    pub marks: HashMap<usize, String>,
    /// Distinct sys:comp pairs present in the file, sorted.
    pub id_options: Vec<(u8, u8)>,
    /// Distinct message-type names present in the file, sorted.
    pub type_options: Vec<String>,
    /// Index into `filtered` of the selected message.
    pub selected: usize,
    /// Saved plot definitions (GUI-only; the TUI never reads this).
    pub plots: Vec<PlotDef>,
    /// Snapshot of the setup as last saved to (or loaded from) the sidecar.
    /// `is_dirty` compares the live setup against this to decide whether there
    /// are unsaved changes worth warning about.
    saved_setup: Setup,
    /// DataFlash message schema; `None` for MAVLink logs, which decode via the
    /// `mavlink` dialect instead.
    schema: Option<dataflash::Schema>,
    /// Set when this session is a merge of two logs; carries the time-alignment
    /// offset applied to the secondary (bin) entries. `None` for single logs.
    pub sync: Option<SyncState>,
    /// Path of the secondary (bin) log in a merged session; `None` otherwise.
    secondary_path: Option<String>,
}

/// Time-alignment state for a merged session. The secondary log's entries were
/// placed at `boot + bin_offset_us + manual_offset_us` on the primary's unix
/// axis. Secondary entries are identified by their `LogEntry.source`, so no
/// buffer split index is needed here.
pub struct SyncState {
    pub bin_offset_us: i64,
    pub manual_offset_us: i64,
    pub method: SyncMethod,
}

impl Session {
    pub fn new(
        path: String,
        data: Vec<u8>,
        entries: Vec<LogEntry>,
        schema: Option<dataflash::Schema>,
    ) -> Self {
        // Formats without ids (DataFlash) contribute none; the dropdowns then
        // just offer "any".
        let mut id_options: Vec<(u8, u8)> =
            entries.iter().filter_map(|e| e.sysid.zip(e.compid)).collect();
        id_options.sort_unstable();
        id_options.dedup();
        let mut type_options: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        type_options.sort_unstable();
        type_options.dedup();

        Self {
            start_us: entries[0].timestamp_us,
            filtered: (0..entries.len()).collect(),
            entries,
            id_options,
            type_options,
            marks: HashMap::new(),
            columns: Vec::new(),
            columns_text: String::new(),
            path,
            data,
            // DataFlash timestamps are microseconds since boot, not wall clock,
            // so absolute dates would be meaningless — default to relative time.
            time_format: if schema.is_some() {
                TimeFormat::OffsetSecs
            } else {
                TimeFormat::DateTime
            },
            filters: Vec::new(),
            filter_text: String::new(),
            selected: 0,
            plots: Vec::new(),
            // A freshly opened file with no edits equals the empty setup; if a
            // sidecar exists, `load_setup` resets this baseline after applying.
            saved_setup: Setup::default(),
            schema,
            sync: None,
            secondary_path: None,
        }
    }

    /// Merge a primary MAVLink tlog with a secondary ArduPilot bin into one
    /// time-synchronized session. The two byte buffers are concatenated (so
    /// decoding is unchanged — payload ranges just index further in), the bin's
    /// boot-relative timestamps are shifted by `bin_offset_us` onto the tlog's
    /// absolute unix axis, and the combined entries are stable-sorted by time.
    pub fn merge(
        primary_path: String,
        secondary_path: String,
        tlog_data: Vec<u8>,
        tlog_entries: Vec<LogEntry>,
        bin_data: Vec<u8>,
        bin_entries: Vec<LogEntry>,
        bin_schema: dataflash::Schema,
        bin_offset_us: i64,
        method: SyncMethod,
    ) -> Self {
        let split = tlog_data.len();
        let mut data = tlog_data;
        data.extend_from_slice(&bin_data);

        let mut entries = tlog_entries; // already absolute unix, tagged Primary
        for mut e in bin_entries {
            e.payload.start += split;
            e.payload.end += split;
            e.source = LogSourceId::Secondary;
            e.timestamp_us = (e.timestamp_us as i64 + bin_offset_us).max(0) as u64;
            entries.push(e);
        }
        // Stable so entries with equal timestamps keep primary-before-secondary
        // order, which the partition_point users rely on.
        entries.sort_by_key(|e| e.timestamp_us);

        let mut session = Session::new(primary_path, data, entries, Some(bin_schema));
        // Both logs are on the absolute axis now, so show wall-clock time.
        session.time_format = TimeFormat::DateTime;
        session.sync = Some(SyncState { bin_offset_us, manual_offset_us: 0, method });
        session.secondary_path = Some(secondary_path);
        session
    }

    /// Whether this session is a merge of two logs.
    pub fn is_merged(&self) -> bool {
        self.sync.is_some()
    }

    /// A short format tag (`tlog`/`bin`) for a `source`, for the compact list
    /// column. `None` for a single-file session.
    pub fn source_tag(&self, source: LogSourceId) -> Option<&'static str> {
        self.is_merged().then(|| match source {
            LogSourceId::Primary => "tlog",
            LogSourceId::Secondary => "bin",
        })
    }

    /// The file name of the log a `source` came from, for UI labels. `None` for
    /// a single-file session.
    pub fn source_name(&self, source: LogSourceId) -> Option<&str> {
        if !self.is_merged() {
            return None;
        }
        let path = match source {
            LogSourceId::Primary => &self.path,
            LogSourceId::Secondary => self.secondary_path.as_deref()?,
        };
        Some(std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path))
    }

    /// A short description of how the merged logs were time-aligned, for the
    /// status bar. `None` for single-file sessions.
    pub fn sync_status(&self) -> Option<String> {
        let s = self.sync.as_ref()?;
        let secs = (s.bin_offset_us + s.manual_offset_us) as f64 / 1e6;
        Some(match s.method {
            SyncMethod::SystemTime => {
                format!("Merged: bin aligned via SYSTEM_TIME (offset {secs:.3}s)")
            }
            SyncMethod::ManualOnly => {
                "Merged: no SYSTEM_TIME found — align the logs manually".to_string()
            }
        })
    }

    /// Entry index of the selected message, if any are visible.
    pub fn selected_entry_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    /// Rebuild the visible index list, keeping the selection as close as
    /// possible to the previously selected message. Only enabled filters
    /// participate; when none are enabled, every message is shown.
    pub fn apply_filter(&mut self) {
        let current = self.filtered.get(self.selected).copied().unwrap_or(0);
        let has_enabled = self.filters.iter().any(|f| f.enabled);
        self.filtered = (0..self.entries.len())
            .filter(|&i| {
                !has_enabled
                    || self
                        .filters
                        .iter()
                        .any(|f| f.enabled && f.matches(&self.entries[i]))
            })
            .collect();
        self.selected = self
            .filtered
            .partition_point(|&i| i < current)
            .min(self.filtered.len().saturating_sub(1));
    }

    /// Select the first visible message at or after the target time.
    pub fn jump_to_time(&mut self, target_us: u64) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = self
            .filtered
            .partition_point(|&i| self.entries[i].timestamp_us < target_us)
            .min(self.filtered.len() - 1);
    }

    /// Select the visible message at or after the given entry index (the
    /// nearest visible message when the exact entry is filtered out).
    pub fn select_entry(&mut self, entry_index: usize) {
        self.selected = self
            .filtered
            .partition_point(|&i| i < entry_index)
            .min(self.filtered.len().saturating_sub(1));
    }

    /// Regenerate the editable filter text from the current expressions.
    pub fn rebuild_filter_text(&mut self) {
        self.filter_text = self
            .filters
            .iter()
            .map(FilterExpr::to_text)
            .collect::<Vec<_>>()
            .join(", ");
    }

    /// Install custom columns and index which entries each one reads from.
    pub fn set_columns(&mut self, mut columns: Vec<CustomColumn>) {
        for col in &mut columns {
            col.matches = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    col.sysid.is_none_or(|s| Some(s) == e.sysid)
                        && col.compid.is_none_or(|c| Some(c) == e.compid)
                        && e.name.eq_ignore_ascii_case(&col.msg_type)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.columns_text = columns
            .iter()
            .map(CustomColumn::to_text)
            .collect::<Vec<_>>()
            .join(", ");
        self.columns = columns;
    }

    /// Decode an entry's payload into a flat JSON object of `field -> value`.
    /// The single place that knows how each log format turns bytes into named
    /// fields, so columns, plots and field discovery stay format-neutral.
    pub fn decode_fields(&self, entry: &LogEntry) -> Option<serde_json::Value> {
        match entry.kind {
            EntryKind::Mavlink { .. } => {
                serde_json::to_value(tlog::decode(&self.data, entry)?).ok()
            }
            EntryKind::Dataflash { msg_type } => {
                dataflash::decode_fields(self.schema.as_ref()?, msg_type, &self.data[entry.payload.clone()])
            }
        }
    }

    /// The unit label a DataFlash schema records for a field, if any. Used to
    /// auto-fill a new plot series' unit. Always `None` for MAVLink logs.
    pub fn field_unit(&self, msg_type: &str, field: &str) -> Option<String> {
        self.schema.as_ref()?.field_unit(msg_type, field)
    }

    /// The detail-pane body for an entry: the decoded message fields, or a hex
    /// dump when the payload can't be decoded.
    pub fn decode_detail(&self, entry: &LogEntry) -> String {
        let hex = || tlog::hex_dump(&self.data[entry.payload.clone()]);
        match entry.kind {
            EntryKind::Mavlink { .. } => {
                tlog::decode(&self.data, entry).map(|m| format!("{m:#?}")).unwrap_or_else(hex)
            }
            EntryKind::Dataflash { msg_type } => self
                .schema
                .as_ref()
                .and_then(|s| dataflash::decode_detail(s, msg_type, &self.data[entry.payload.clone()]))
                .unwrap_or_else(hex),
        }
    }

    /// The column's field value from the last matching message at or before
    /// the given entry.
    pub fn column_value(&self, col: &CustomColumn, entry_index: usize) -> String {
        let pos = col.matches.partition_point(|&i| i <= entry_index);
        let Some(&source) = pos.checked_sub(1).map(|p| &col.matches[p]) else {
            return String::new(); // nothing seen yet
        };
        let Some(value) = self.decode_fields(&self.entries[source]) else {
            return "?".to_string();
        };
        let field = value
            .as_object()
            .and_then(|obj| obj.iter().find(|(k, _)| k.eq_ignore_ascii_case(&col.field)))
            .map(|(_, v)| v);
        match field {
            None => "?".to_string(),
            Some(serde_json::Value::String(s)) => s.clone(),
            // Enum fields serialize as {"type": "MAV_..."}.
            Some(v) => match v.get("type").and_then(|t| t.as_str()) {
                Some(name) => name.to_string(),
                None => v.to_string(),
            },
        }
    }

    /// Field names of a message type, from the first decodable sample in
    /// the file.
    pub fn field_options(&self, msg_type: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.name == msg_type)
            .find_map(|e| self.decode_fields(e))
            .and_then(|value| {
                value
                    .as_object()
                    .map(|obj| obj.keys().filter(|k| *k != "type").cloned().collect())
            })
            .unwrap_or_default()
    }

    /// Dropdown option label: 0 is "any", the rest map into id_options.
    pub fn id_option_text(&self, choice: usize) -> String {
        match choice.checked_sub(1) {
            None => "any".to_string(),
            Some(i) => format!("{}:{}", self.id_options[i].0, self.id_options[i].1),
        }
    }

    /// Dropdown option label: 0 is "any", the rest map into type_options.
    pub fn type_option_text(&self, choice: usize) -> String {
        match choice.checked_sub(1) {
            None => "any".to_string(),
            Some(i) => self.type_options[i].clone(),
        }
    }

    /// Dropdown option labels for a filter-editor field (0 = id, else type).
    pub fn filter_dropdown_labels(&self, field_row: usize) -> Vec<String> {
        match field_row {
            0 => (0..=self.id_options.len())
                .map(|i| self.id_option_text(i))
                .collect(),
            _ => (0..=self.type_options.len())
                .map(|i| self.type_option_text(i))
                .collect(),
        }
    }

    /// Dropdown option labels for a column-editor field (1 = id, 2 = type,
    /// else the fields of the chosen type).
    pub fn column_dropdown_labels(&self, field_row: usize, type_choice: usize) -> Vec<String> {
        match field_row {
            1 => (0..=self.id_options.len())
                .map(|i| self.id_option_text(i))
                .collect(),
            2 => self.type_options.clone(),
            _ => self
                .type_options
                .get(type_choice)
                .map(|t| self.field_options(t))
                .unwrap_or_default(),
        }
    }

    /// Format a timestamp for the list column per the current time mode.
    pub fn format_list_time(&self, timestamp_us: u64) -> String {
        match self.time_format {
            TimeFormat::DateTime => format_datetime(timestamp_us),
            TimeFormat::OffsetSecs => format_offset(timestamp_us, self.start_us),
        }
    }

    /// The live setup as it would be written to the sidecar right now.
    fn current_setup(&self) -> Setup {
        Setup {
            time_format: self.time_format,
            filter: self.filter_text.clone(),
            marks: self.marks.iter().map(|(&i, l)| (i, l.clone())).collect(),
            selected: self.filtered.get(self.selected).copied().unwrap_or(0),
            columns: self.columns_text.clone(),
            plots: self.plots.clone(),
        }
    }

    /// Whether the live setup differs from the last saved/loaded state, i.e.
    /// there are unsaved changes. The selected message is ignored: moving the
    /// cursor is navigation, not an edit worth warning about losing.
    pub fn is_dirty(&self) -> bool {
        let mut current = self.current_setup();
        current.selected = self.saved_setup.selected;
        current != self.saved_setup
    }

    /// Write the current state to the setup sidecar, returning the path on
    /// success or a human-readable error.
    pub fn save_setup(&mut self) -> Result<String, String> {
        let setup = self.current_setup();
        let path = setup_path(&self.path);
        let json = serde_json::to_string_pretty(&setup).expect("setup serializes");
        match std::fs::write(&path, json) {
            Ok(()) => {
                self.saved_setup = setup;
                Ok(path)
            }
            Err(err) => Err(format!("Failed to save {path}: {err}")),
        }
    }

    /// Restore a previously saved setup for this file. Returns a status
    /// message, or None if no sidecar exists.
    pub fn load_setup(&mut self) -> Option<String> {
        let path = setup_path(&self.path);
        let json = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<Setup>(&json) {
            Ok(setup) => {
                self.time_format = setup.time_format;
                if let Ok(filters) = parse_filters(&setup.filter) {
                    self.filters = filters;
                    self.filter_text = setup.filter.trim().to_string();
                }
                self.marks = setup
                    .marks
                    .into_iter()
                    .filter(|&(i, _)| i < self.entries.len())
                    .collect();
                if let Ok(columns) = parse_columns(&setup.columns) {
                    self.set_columns(columns);
                }
                self.plots = setup.plots;
                self.apply_filter();
                self.select_entry(setup.selected);
                // Baseline for `is_dirty`: the just-loaded state has no unsaved
                // changes. Rebuilt from the applied fields so it matches their
                // normalized form (trimmed filter, re-parsed columns).
                self.saved_setup = self.current_setup();
                Some(format!("Loaded setup from {path}"))
            }
            Err(err) => Some(format!("Failed to load {path}: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entry::{EntryKind, LogEntry, LogSourceId};

    fn entry(ts_us: u64, sysid: u8, name: &str) -> LogEntry {
        LogEntry {
            timestamp_us: ts_us,
            sysid: Some(sysid),
            compid: Some(1),
            payload: 0..0,
            name: name.to_string(),
            kind: EntryKind::Mavlink { msg_id: 0, version: mavlink::MavlinkVersion::V2 },
            source: LogSourceId::Primary,
        }
    }

    fn session(path: &str) -> Session {
        let entries = vec![
            entry(1_000_000, 1, "HEARTBEAT"),
            entry(2_000_000, 1, "ATTITUDE"),
            entry(3_000_000, 2, "HEARTBEAT"),
            entry(4_000_000, 1, "VFR_HUD"),
        ];
        Session::new(path.to_string(), Vec::new(), entries, None)
    }

    #[test]
    fn id_and_type_options_are_sorted_and_deduped() {
        let s = session("x");
        assert_eq!(s.id_options, vec![(1, 1), (2, 1)]);
        assert_eq!(
            s.type_options,
            vec!["ATTITUDE".to_string(), "HEARTBEAT".to_string(), "VFR_HUD".to_string()]
        );
    }

    #[test]
    fn apply_filter_narrows_and_preserves_selection() {
        let mut s = session("x");
        s.selected = 3; // VFR_HUD
        s.filters = parse_filters("=HEARTBEAT").unwrap();
        s.apply_filter();
        assert_eq!(s.filtered, vec![0, 2]);
        // Selection moves to the nearest surviving entry at or before the old.
        assert_eq!(s.selected_entry_index(), Some(2));
    }

    #[test]
    fn disabled_filters_do_not_narrow() {
        let mut s = session("x");
        // A single disabled filter matches nothing to hide: show everything.
        s.filters = parse_filters("!=HEARTBEAT").unwrap();
        s.apply_filter();
        assert_eq!(s.filtered, vec![0, 1, 2, 3]);

        // With one enabled and one disabled, only the enabled one applies.
        s.filters = parse_filters("=HEARTBEAT, !=ATTITUDE").unwrap();
        s.apply_filter();
        assert_eq!(s.filtered, vec![0, 2]);

        // Re-enabling the second filter widens the result.
        s.filters[1].enabled = true;
        s.apply_filter();
        assert_eq!(s.filtered, vec![0, 1, 2]);
    }

    #[test]
    fn disabled_filters_roundtrip_through_sidecar() {
        let path = std::env::temp_dir()
            .join(format!("mavlog-disabled-{}.tlog", std::process::id()));
        let path = path.to_str().unwrap().to_string();

        let mut s = session(&path);
        s.filters = parse_filters("=HEARTBEAT, !=ATTITUDE").unwrap();
        s.rebuild_filter_text();
        assert_eq!(s.filter_text, "=HEARTBEAT, !=ATTITUDE");
        s.apply_filter();
        s.save_setup().unwrap();

        let mut restored = session(&path);
        restored.load_setup().unwrap();
        assert_eq!(restored.filter_text, "=HEARTBEAT, !=ATTITUDE");
        assert!(restored.filters[0].enabled);
        assert!(!restored.filters[1].enabled);
        assert_eq!(restored.filtered, vec![0, 2]);

        let _ = std::fs::remove_file(setup_path(&path));
    }

    #[test]
    fn jump_to_time_selects_first_at_or_after() {
        let mut s = session("x");
        s.jump_to_time(2_500_000);
        assert_eq!(s.selected_entry_index(), Some(2)); // ts 3_000_000
    }

    #[test]
    fn select_entry_lands_on_nearest_visible() {
        let mut s = session("x");
        s.filters = parse_filters("=HEARTBEAT").unwrap();
        s.apply_filter();
        assert_eq!(s.filtered, vec![0, 2]);
        // Entry 1 (ATTITUDE) is filtered out; the nearest visible at-or-after
        // is entry 2.
        s.select_entry(1);
        assert_eq!(s.selected_entry_index(), Some(2));
        // An exact visible entry selects itself.
        s.select_entry(0);
        assert_eq!(s.selected_entry_index(), Some(0));
    }

    #[test]
    fn setup_roundtrips_through_sidecar() {
        let path = std::env::temp_dir()
            .join(format!("mavlog-test-{}.tlog", std::process::id()));
        let path = path.to_str().unwrap().to_string();

        let mut s = session(&path);
        s.time_format = TimeFormat::OffsetSecs;
        s.filters = parse_filters("=HEARTBEAT").unwrap();
        s.rebuild_filter_text();
        s.apply_filter();
        s.marks.insert(2, "mark2".to_string());
        s.marks.insert(999, "out of range".to_string()); // dropped on load
        s.selected = 1; // filtered[1] == entry 2
        s.save_setup().unwrap();

        let mut restored = session(&path);
        assert!(restored.load_setup().unwrap().starts_with("Loaded setup"));
        assert_eq!(restored.time_format, TimeFormat::OffsetSecs);
        assert_eq!(restored.filter_text, "=HEARTBEAT");
        assert_eq!(restored.filtered, vec![0, 2]);
        assert_eq!(restored.marks.get(&2).map(String::as_str), Some("mark2"));
        assert!(!restored.marks.contains_key(&999));
        assert_eq!(restored.selected_entry_index(), Some(2));

        let _ = std::fs::remove_file(setup_path(&path));
    }

    #[test]
    fn is_dirty_tracks_edits_and_saves() {
        let path = std::env::temp_dir()
            .join(format!("mavlog-dirty-{}.tlog", std::process::id()));
        let path = path.to_str().unwrap().to_string();

        // A freshly opened file with no edits is clean.
        let mut s = session(&path);
        assert!(!s.is_dirty());

        // Moving the selection is navigation, not an edit worth warning about.
        s.selected = 2;
        assert!(!s.is_dirty());

        // An actual edit (a mark) makes it dirty.
        s.marks.insert(1, String::new());
        assert!(s.is_dirty());

        // Saving clears the dirty flag.
        s.save_setup().unwrap();
        assert!(!s.is_dirty());

        // A further edit is dirty again; loading the saved setup restores clean.
        s.marks.insert(2, "note".to_string());
        assert!(s.is_dirty());
        s.load_setup().unwrap();
        assert!(!s.is_dirty());

        let _ = std::fs::remove_file(setup_path(&path));
    }
}
