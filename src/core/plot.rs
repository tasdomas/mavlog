//! Plot definitions and time-series extraction. There is no TUI equivalent —
//! plotting needs a graphical canvas — but the data model and extraction
//! logic are pure and unit-testable like the rest of `core`.

use crate::core::session::Session;

/// A named plot: one or more series drawn together on the same axes.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeriesDef {
    pub sysid: Option<u8>,
    pub compid: Option<u8>,
    pub msg_type: String,
    pub field: String,
    /// Optional user-chosen legend name; falls back to a `type.field` label
    /// when unset. Defaulted so sidecars saved before it existed still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional measurement unit (e.g. "m/s"). Series sharing a unit share a
    /// single Y axis; different units get their own axis (matplotlib twinx
    /// style). Defaulted so older sidecars still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Above this many extracted points, a series is min/max-bucket decimated so
/// plotting stays responsive.
const DECIMATE_ABOVE: usize = 200_000;

/// A series' extracted `[timestamp_us, value]` points plus its value range,
/// computed once at extraction time so the GUI doesn't rescan up to
/// `DECIMATE_ABOVE` points every frame to group axes by unit. `y_min`/`y_max`
/// are `+INFINITY`/`-INFINITY` for an empty series (no finite range).
pub struct SeriesData {
    pub points: Vec<[f64; 2]>,
    pub y_min: f64,
    pub y_max: f64,
}

/// Extract `[timestamp_us, value]` points for a series: scans entries
/// matching the series' id/type, decodes each, and coerces the named field
/// to `f64`. Entries where the field is missing or not numeric (e.g. enum
/// fields, which serialize as `{"type": "..."}`) are skipped.
pub fn extract(session: &Session, series: &SeriesDef) -> SeriesData {
    let points: Vec<[f64; 2]> = session
        .entries
        .iter()
        .filter(|e| {
            series.sysid.is_none_or(|s| Some(s) == e.sysid)
                && series.compid.is_none_or(|c| Some(c) == e.compid)
                && e.name.eq_ignore_ascii_case(&series.msg_type)
        })
        .filter_map(|e| {
            let value = session.decode_fields(e)?;
            let field = value
                .as_object()?
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&series.field))?
                .1;
            Some([e.timestamp_us as f64, field.as_f64()?])
        })
        .collect();
    let points = decimate(points);
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    for p in &points {
        if p[1].is_finite() {
            y_min = y_min.min(p[1]);
            y_max = y_max.max(p[1]);
        }
    }
    SeriesData { points, y_min, y_max }
}

/// One Y axis shared by every series carrying the same `unit`. Non-primary
/// groups are drawn rescaled into the primary group's coordinate space via
/// `displayed = real * scale + offset`, so a single plot transform can host
/// several units at once; the axis' tick formatter inverts the map to show
/// real values. The primary (first) group is the identity (`scale == 1`,
/// `offset == 0`).
pub struct AxisGroup {
    pub unit: Option<String>,
    /// Indices into the plot's `series`/`SeriesData` slice, in first-appearance
    /// order.
    pub series: Vec<usize>,
    pub scale: f64,
    pub offset: f64,
}

/// Combined finite value range over a set of series, or `None` when none of
/// them have finite data.
fn group_range(data: &[SeriesData], members: &[usize]) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &i in members {
        if data[i].y_min.is_finite() {
            lo = lo.min(data[i].y_min);
            hi = hi.max(data[i].y_max);
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// Group series by measurement unit into shared Y axes. The first unit to
/// appear is the primary axis (identity mapping); every other unit's group is
/// given an affine `scale`/`offset` mapping its value range onto the primary
/// group's range, so all groups can share one plot transform. A blank/absent
/// unit is treated as "no unit" (`None`) and grouped together.
///
/// Degenerate cases are handled so the mapping is always finite: a zero-width
/// or non-finite span (flat series, single point, empty group) substitutes a
/// span of `1.0`, collapsing that group onto the primary range's low end
/// rather than dividing by zero.
pub fn axis_groups(series: &[SeriesDef], data: &[SeriesData]) -> Vec<AxisGroup> {
    // Normalize unit strings: trim, and treat blank as "no unit".
    let unit_of = |i: usize| -> Option<String> {
        series[i].unit.as_ref().map(|u| u.trim().to_string()).filter(|u| !u.is_empty())
    };

    let mut groups: Vec<AxisGroup> = Vec::new();
    for i in 0..series.len() {
        let unit = unit_of(i);
        match groups.iter_mut().find(|g| g.unit == unit) {
            Some(g) => g.series.push(i),
            None => groups.push(AxisGroup { unit, series: vec![i], scale: 1.0, offset: 0.0 }),
        }
    }

    // Primary group's range anchors the shared coordinate space.
    let Some((pmin, pmax)) = groups.first().and_then(|g| group_range(data, &g.series)) else {
        // No finite data at all: leave every group as the identity mapping.
        return groups;
    };
    let pspan = finite_span(pmax - pmin);

    for g in groups.iter_mut().skip(1) {
        let (gmin, gmax) = group_range(data, &g.series).unwrap_or((0.0, 0.0));
        let gspan = finite_span(gmax - gmin);
        g.scale = pspan / gspan;
        g.offset = pmin - gmin * g.scale;
    }
    groups
}

/// A strictly-positive, finite span for use as a divisor: zero-width or
/// non-finite spans become `1.0`.
fn finite_span(span: f64) -> f64 {
    if span.is_finite() && span > 0.0 {
        span
    } else {
        1.0
    }
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
    use crate::tlog;

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
            name: None,
            unit: None,
        };
        assert_eq!(extract(&session, &series).points, vec![[1_000_000.0, 3.0]]);
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
            name: None,
            unit: None,
        };
        assert!(extract(&session, &series).points.is_empty());
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
    fn seriesdef_defaults_name_and_unit_to_none() {
        // A sidecar saved before name/unit existed omits those fields; they
        // must deserialize as None rather than failing.
        let json = r#"{"sysid":null,"compid":null,"msg_type":"VFR_HUD","field":"groundspeed"}"#;
        let series: SeriesDef = serde_json::from_str(json).unwrap();
        assert!(series.name.is_none());
        assert!(series.unit.is_none());
        // And unset fields are omitted on save (old builds still load them).
        let out = serde_json::to_string(&series).unwrap();
        assert!(!out.contains("name"));
        assert!(!out.contains("unit"));
    }

    fn series_with_unit(unit: Option<&str>) -> SeriesDef {
        SeriesDef {
            sysid: None,
            compid: None,
            msg_type: "M".to_string(),
            field: "f".to_string(),
            name: None,
            unit: unit.map(str::to_string),
        }
    }

    fn data(y_min: f64, y_max: f64) -> SeriesData {
        SeriesData { points: vec![[0.0, y_min], [1.0, y_max]], y_min, y_max }
    }

    #[test]
    fn axis_groups_single_unit_is_identity() {
        let series = vec![series_with_unit(Some("m")), series_with_unit(Some("m"))];
        let d = vec![data(0.0, 10.0), data(5.0, 20.0)];
        let groups = axis_groups(&series, &d);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].series, vec![0, 1]);
        assert_eq!(groups[0].scale, 1.0);
        assert_eq!(groups[0].offset, 0.0);
    }

    #[test]
    fn axis_groups_blank_unit_groups_with_none() {
        let series = vec![series_with_unit(None), series_with_unit(Some("  "))];
        let d = vec![data(0.0, 1.0), data(0.0, 1.0)];
        let groups = axis_groups(&series, &d);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].unit, None);
    }

    #[test]
    fn axis_groups_rescales_second_unit_invertibly() {
        // Primary "m" spans 0..100; secondary "deg" spans 0..360. The
        // secondary group's map must send its own range onto the primary's.
        let series = vec![series_with_unit(Some("m")), series_with_unit(Some("deg"))];
        let d = vec![data(0.0, 100.0), data(0.0, 360.0)];
        let groups = axis_groups(&series, &d);
        assert_eq!(groups.len(), 2);
        let deg = &groups[1];
        // Endpoints of the secondary map onto the primary endpoints.
        assert!((0.0 * deg.scale + deg.offset - 0.0).abs() < 1e-9);
        assert!((360.0 * deg.scale + deg.offset - 100.0).abs() < 1e-9);
        // And the inverse recovers a real value: displayed 50 -> real 180.
        let real = (50.0 - deg.offset) / deg.scale;
        assert!((real - 180.0).abs() < 1e-9);
    }

    #[test]
    fn axis_groups_flat_secondary_is_finite() {
        // A flat (zero-span) secondary series must not produce NaN/inf.
        let series = vec![series_with_unit(Some("m")), series_with_unit(Some("v"))];
        let d = vec![data(0.0, 100.0), data(7.0, 7.0)];
        let groups = axis_groups(&series, &d);
        let v = &groups[1];
        assert!(v.scale.is_finite());
        assert!(v.offset.is_finite());
    }

    #[test]
    fn axis_groups_tolerates_empty_series() {
        let series = vec![series_with_unit(Some("m")), series_with_unit(Some("deg"))];
        let empty = SeriesData { points: vec![], y_min: f64::INFINITY, y_max: f64::NEG_INFINITY };
        let d = vec![empty, data(0.0, 360.0)];
        let groups = axis_groups(&series, &d);
        // Primary has no finite range -> everything stays identity, no panic.
        assert!(groups.iter().all(|g| g.scale.is_finite() && g.offset.is_finite()));
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
