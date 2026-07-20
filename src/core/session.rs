//! The `Session`: all per-file domain state and the operations on it, shared
//! by the TUI and GUI frontends. It knows nothing about how it is displayed.

use std::collections::HashMap;

use crate::core::column::{parse_columns, CustomColumn};
use crate::core::filter::{parse_filters, FilterExpr};
use crate::core::plot::PlotDef;
use crate::core::setup::{setup_path, Setup};
use crate::core::time::{format_datetime, format_offset, TimeFormat};
use crate::tlog;

pub struct Session {
    pub path: String,
    pub data: Vec<u8>,
    pub entries: Vec<tlog::LogEntry>,
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
}

impl Session {
    pub fn new(path: String, data: Vec<u8>, entries: Vec<tlog::LogEntry>) -> Self {
        let mut id_options: Vec<(u8, u8)> =
            entries.iter().map(|e| (e.sysid, e.compid)).collect();
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
            time_format: TimeFormat::DateTime,
            filters: Vec::new(),
            filter_text: String::new(),
            selected: 0,
            plots: Vec::new(),
        }
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
                    col.sysid.is_none_or(|s| s == e.sysid)
                        && col.compid.is_none_or(|c| c == e.compid)
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

    /// The column's field value from the last matching message at or before
    /// the given entry.
    pub fn column_value(&self, col: &CustomColumn, entry_index: usize) -> String {
        let pos = col.matches.partition_point(|&i| i <= entry_index);
        let Some(&source) = pos.checked_sub(1).map(|p| &col.matches[p]) else {
            return String::new(); // nothing seen yet
        };
        let Ok(msg) = tlog::decode(&self.data, &self.entries[source]) else {
            return "?".to_string();
        };
        let Ok(value) = serde_json::to_value(&msg) else {
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
            .find_map(|e| tlog::decode(&self.data, e).ok())
            .and_then(|msg| serde_json::to_value(&msg).ok())
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

    /// Write the current state to the setup sidecar, returning the path on
    /// success or a human-readable error.
    pub fn save_setup(&self) -> Result<String, String> {
        let setup = Setup {
            time_format: self.time_format,
            filter: self.filter_text.clone(),
            marks: self.marks.iter().map(|(&i, l)| (i, l.clone())).collect(),
            selected: self.filtered.get(self.selected).copied().unwrap_or(0),
            columns: self.columns_text.clone(),
            plots: self.plots.clone(),
        };
        let path = setup_path(&self.path);
        let json = serde_json::to_string_pretty(&setup).expect("setup serializes");
        match std::fs::write(&path, json) {
            Ok(()) => Ok(path),
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
                Some(format!("Loaded setup from {path}"))
            }
            Err(err) => Some(format!("Failed to load {path}: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tlog::LogEntry;

    fn entry(ts_us: u64, sysid: u8, name: &str) -> LogEntry {
        LogEntry {
            timestamp_us: ts_us,
            sysid,
            compid: 1,
            msg_id: 0,
            version: mavlink::MavlinkVersion::V2,
            payload: 0..0,
            name: name.to_string(),
        }
    }

    fn session(path: &str) -> Session {
        let entries = vec![
            entry(1_000_000, 1, "HEARTBEAT"),
            entry(2_000_000, 1, "ATTITUDE"),
            entry(3_000_000, 2, "HEARTBEAT"),
            entry(4_000_000, 1, "VFR_HUD"),
        ];
        Session::new(path.to_string(), Vec::new(), entries)
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
}
