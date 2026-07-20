//! Plot definitions and time-series extraction. There is no TUI equivalent —
//! plotting needs a graphical canvas — but the data model and extraction
//! logic are pure and unit-testable like the rest of `core`.

use crate::core::session::Session;
use crate::tlog;

/// A named plot: one or more series drawn together on the same axes.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PlotDef {
    pub name: String,
    pub series: Vec<SeriesDef>,
    /// Whether mark lines/labels are drawn on this plot. Defaulted to `true`
    /// so sidecars saved before the flag existed keep showing marks.
    #[serde(default = "default_true")]
    pub show_marks: bool,
}

fn default_true() -> bool {
    true
}

/// One series: values of `field` from messages matching `sysid:compid` (any
/// side that is `None` matches anything) and `msg_type`, plotted against
/// time.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SeriesDef {
    pub sysid: Option<u8>,
    pub compid: Option<u8>,
    pub msg_type: String,
    pub field: String,
}

/// Above this many extracted points, a series is min/max-bucket decimated so
/// plotting stays responsive.
const DECIMATE_ABOVE: usize = 200_000;

/// Extract `[timestamp_us, value]` points for a series: scans entries
/// matching the series' id/type, decodes each, and coerces the named field
/// to `f64`. Entries where the field is missing or not numeric (e.g. enum
/// fields, which serialize as `{"type": "..."}`) are skipped.
pub fn extract(session: &Session, series: &SeriesDef) -> Vec<[f64; 2]> {
    let points: Vec<[f64; 2]> = session
        .entries
        .iter()
        .filter(|e| {
            series.sysid.is_none_or(|s| s == e.sysid)
                && series.compid.is_none_or(|c| c == e.compid)
                && e.name.eq_ignore_ascii_case(&series.msg_type)
        })
        .filter_map(|e| {
            let msg = tlog::decode(&session.data, e).ok()?;
            let value = serde_json::to_value(&msg).ok()?;
            let field = value
                .as_object()?
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&series.field))?
                .1;
            Some([e.timestamp_us as f64, field.as_f64()?])
        })
        .collect();
    decimate(points)
}

/// Reduce `points` to at most ~`DECIMATE_ABOVE` points by splitting into
/// equal-size buckets and keeping each bucket's min and max (by value),
/// preserving spikes that plain downsampling would smooth away.
fn decimate(points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if points.len() <= DECIMATE_ABOVE {
        return points;
    }
    let bucket_count = DECIMATE_ABOVE / 2;
    let bucket_size = points.len().div_ceil(bucket_count);
    let mut out = Vec::with_capacity(bucket_count * 2);
    for chunk in points.chunks(bucket_size) {
        let (mut min, mut max) = (chunk[0], chunk[0]);
        for &p in chunk {
            if p[1] < min[1] {
                min = p;
            }
            if p[1] > max[1] {
                max = p;
            }
        }
        // Keep points ordered by x within the pair, regardless of which of
        // min/max came first in the source data.
        if min[0] <= max[0] {
            out.push(min);
            out.push(max);
        } else {
            out.push(max);
            out.push(min);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // v2 HEARTBEAT frames (msg id 0) differing only by sysid (byte index 4),
    // decoding to mavlink_version = 3 (see tlog::tests::decodes_payload_fields).
    const V2_HEARTBEAT_SYS1: &[u8] = &[
        0xFD, 9, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 0, 0,
    ];
    const V2_HEARTBEAT_SYS2: &[u8] = &[
        0xFD, 9, 0, 0, 1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 2, 3, 81, 4, 3, 0, 0,
    ];

    fn record(timestamp_us: u64, frame: &[u8]) -> Vec<u8> {
        let mut rec = timestamp_us.to_be_bytes().to_vec();
        rec.extend_from_slice(frame);
        rec
    }

    #[test]
    fn extract_filters_by_sysid_and_reads_the_field() {
        let mut data = record(1_000_000, V2_HEARTBEAT_SYS1);
        data.extend(record(2_000_000, V2_HEARTBEAT_SYS2));
        let entries = tlog::parse(&data);
        let session = Session::new("x".to_string(), data, entries);

        let series = SeriesDef {
            sysid: Some(1),
            compid: None,
            msg_type: "HEARTBEAT".to_string(),
            field: "mavlink_version".to_string(),
        };
        assert_eq!(extract(&session, &series), vec![[1_000_000.0, 3.0]]);
    }

    #[test]
    fn extract_skips_nonnumeric_fields() {
        let data = record(1_000_000, V2_HEARTBEAT_SYS1);
        let entries = tlog::parse(&data);
        let session = Session::new("x".to_string(), data, entries);

        // `mavtype` is an enum field, serializing as {"type": "..."}, not a
        // plain number — it should be skipped rather than coerced.
        let series = SeriesDef {
            sysid: None,
            compid: None,
            msg_type: "HEARTBEAT".to_string(),
            field: "mavtype".to_string(),
        };
        assert!(extract(&session, &series).is_empty());
    }

    #[test]
    fn plotdef_defaults_show_marks_to_true() {
        // A sidecar saved before `show_marks` existed omits the field; it must
        // deserialize with marks enabled to preserve the old behavior.
        let json = r#"{"name":"p","series":[]}"#;
        let plot: PlotDef = serde_json::from_str(json).unwrap();
        assert!(plot.show_marks);
    }

    #[test]
    fn decimate_keeps_short_series_untouched() {
        let points: Vec<[f64; 2]> = (0..10).map(|i| [i as f64, i as f64]).collect();
        assert_eq!(decimate(points.clone()), points);
    }

    #[test]
    fn decimate_shrinks_long_series_and_keeps_extremes() {
        let points: Vec<[f64; 2]> = (0..500_000)
            .map(|i| [i as f64, (i % 1000) as f64])
            .collect();
        let out = decimate(points);
        assert!(out.len() <= DECIMATE_ABOVE);
        assert!(!out.is_empty());
        // The global max (999) must survive decimation.
        assert!(out.iter().any(|p| p[1] == 999.0));
    }
}
