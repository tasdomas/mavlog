//! Persisted per-file setup: the JSON sidecar stored next to each tlog.

use std::collections::BTreeMap;

use crate::core::time::TimeFormat;

/// Persisted per-file state, stored as JSON next to the tlog.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Setup {
    pub time_format: TimeFormat,
    pub filter: String,
    /// Marked messages by entry index, with optional labels.
    pub marks: BTreeMap<usize, String>,
    /// Entry index of the selected message.
    pub selected: usize,
    /// Custom column definitions in `parse_columns` text form.
    #[serde(default)]
    pub columns: String,
}

/// Path of the setup sidecar for a given log file.
pub fn setup_path(log_path: &str) -> String {
    format!("{log_path}.mavlog.json")
}
