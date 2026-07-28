//! Persisted per-file setup: the JSON sidecar stored next to each tlog.

use std::collections::BTreeMap;

use crate::core::plot::PlotDef;
use crate::core::time::TimeFormat;

/// Persisted per-file state, stored as JSON next to the tlog.
#[derive(Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
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
    /// Saved plot definitions. Defaulted so sidecars from before plots
    /// existed still load.
    #[serde(default)]
    pub plots: Vec<PlotDef>,
}

/// Path of the setup sidecar for a given log file.
pub fn setup_path(log_path: &str) -> String {
    format!("{log_path}.mavlog.json")
}

/// A saved *merged* session: the two source logs, the manual time offset, and
/// all settings. Unlike the per-log [`Setup`] sidecar this names its own logs,
/// so it is opened directly (its file extension is `SESSION_EXT`).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SessionFile {
    pub primary_path: String,
    pub secondary_path: String,
    pub manual_offset_us: i64,
    pub setup: Setup,
}

/// File extension for session files (JSON content).
pub const SESSION_EXT: &str = "mavses";
