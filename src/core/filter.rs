//! Message filtering: expressions matched against log entries, plus the
//! shared dropdown label matcher.

use crate::core::entry::{LogEntry, LogSourceId};

/// One filter expression; every part that is present must match. A message
/// is shown if any expression matches it.
pub struct FilterExpr {
    pub sysid: Option<u8>,
    pub compid: Option<u8>,
    /// Lowercase message-type pattern, may contain '*' wildcards.
    pub name: Option<String>,
    /// Match the type name exactly instead of substring/glob.
    pub exact: bool,
    /// Restrict to one log of a merged session. `None` matches either.
    pub source: Option<LogSourceId>,
    /// Disabled expressions are kept and persisted (with a '!' text prefix)
    /// but do not participate in filtering.
    pub enabled: bool,
}

/// Text token for a source qualifier (`@tlog` / `@bin`); also the label used in
/// merged-session editors. Primary is always the tlog, Secondary the bin.
pub fn source_token(source: LogSourceId) -> &'static str {
    match source {
        LogSourceId::Primary => "tlog",
        LogSourceId::Secondary => "bin",
    }
}

fn parse_source_token(s: &str) -> Option<LogSourceId> {
    match s.to_ascii_lowercase().as_str() {
        "tlog" => Some(LogSourceId::Primary),
        "bin" => Some(LogSourceId::Secondary),
        _ => None,
    }
}

impl FilterExpr {
    pub fn matches(&self, entry: &LogEntry) -> bool {
        self.source.is_none_or(|s| s == entry.source)
            && self.sysid.is_none_or(|s| Some(s) == entry.sysid)
            && self.compid.is_none_or(|c| Some(c) == entry.compid)
            && self.name.as_deref().is_none_or(|p| {
                if self.exact {
                    entry.name.eq_ignore_ascii_case(p)
                } else {
                    name_matches(p, &entry.name)
                }
            })
    }

    /// Text form accepted by `parse_filters`.
    pub fn to_text(&self) -> String {
        let mut parts = Vec::new();
        if let Some(source) = self.source {
            parts.push(format!("@{}", source_token(source)));
        }
        if self.sysid.is_some() || self.compid.is_some() {
            let part = |v: Option<u8>| v.map_or("*".to_string(), |v| v.to_string());
            parts.push(format!("{}:{}", part(self.sysid), part(self.compid)));
        }
        if let Some(name) = &self.name {
            let name = name.to_ascii_uppercase();
            parts.push(if self.exact { format!("={name}") } else { name });
        }
        let text = if parts.is_empty() {
            "*".to_string()
        } else {
            parts.join(" ")
        };
        if self.enabled {
            text
        } else {
            format!("!{text}")
        }
    }
}

/// Parse a comma-separated list of filter expressions. Each expression is an
/// optional `sys[:comp]` id spec and/or a message-type pattern, e.g.
/// "1:1 HEARTBEAT, GPS*, 255". An empty input clears the filter.
pub fn parse_filters(input: &str) -> Result<Vec<FilterExpr>, String> {
    let mut exprs = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // A leading '!' disables the expression; it is still kept and matched
        // against in the list, just excluded from filtering.
        let (enabled, part) = match part.strip_prefix('!') {
            Some(rest) => (false, rest.trim_start()),
            None => (true, part),
        };
        if part.is_empty() {
            return Err("empty filter expression after '!'".to_string());
        }
        let mut expr = FilterExpr {
            sysid: None,
            compid: None,
            name: None,
            exact: false,
            source: None,
            enabled,
        };
        for token in part.split_whitespace() {
            // A '@tlog' / '@bin' token restricts to one merged-session source.
            if let Some(name) = token.strip_prefix('@') {
                if expr.source.is_some() {
                    return Err(format!("more than one source in '{part}'"));
                }
                expr.source = Some(
                    parse_source_token(name)
                        .ok_or_else(|| format!("unknown source '@{name}' (use @tlog or @bin)"))?,
                );
                continue;
            }
            // An id spec is digits/':'/'*' only; a bare "*" is a type pattern.
            let is_id_spec = token != "*"
                && token
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ':' || c == '*');
            if is_id_spec {
                if expr.sysid.is_some() || expr.compid.is_some() {
                    return Err(format!("more than one id spec in '{part}'"));
                }
                let (sys, comp) = match token.split_once(':') {
                    Some((sys, comp)) => (sys, comp),
                    None => (token, ""),
                };
                let parse_id = |s: &str| -> Result<Option<u8>, String> {
                    if s.is_empty() || s == "*" {
                        return Ok(None);
                    }
                    s.parse()
                        .map(Some)
                        .map_err(|_| format!("bad id '{s}' in '{part}'"))
                };
                expr.sysid = parse_id(sys)?;
                expr.compid = parse_id(comp)?;
            } else {
                if expr.name.is_some() {
                    return Err(format!("more than one message type in '{part}'"));
                }
                // '=' prefix requires an exact type match.
                match token.strip_prefix('=') {
                    Some(rest) => {
                        expr.name = Some(rest.to_ascii_lowercase());
                        expr.exact = true;
                    }
                    None => expr.name = Some(token.to_ascii_lowercase()),
                }
            }
        }
        exprs.push(expr);
    }
    Ok(exprs)
}

/// Indices of labels containing the query, case-insensitive.
pub fn match_labels(labels: &[String], query: &str) -> Vec<usize> {
    let query = query.to_ascii_lowercase();
    (0..labels.len())
        .filter(|&i| labels[i].to_ascii_lowercase().contains(&query))
        .collect()
}

/// Case-insensitive message-type match: substring by default, glob when the
/// pattern contains '*'.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if !pattern.contains('*') {
        return name.contains(pattern);
    }
    let segments: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (segments[0], *segments.last().unwrap());
    if name.len() < first.len() + last.len()
        || !name.starts_with(first)
        || !name.ends_with(last)
    {
        return false;
    }
    // Middle segments must appear in order between the anchored ends.
    let mut rest = &name[first.len()..name.len() - last.len()];
    for seg in &segments[1..segments.len() - 1] {
        match rest.find(seg) {
            Some(i) => rest = &rest[i + seg.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sysid: u8, compid: u8, name: &str) -> LogEntry {
        use crate::core::entry::{EntryKind, LogSourceId};
        LogEntry {
            timestamp_us: 0,
            sysid: Some(sysid),
            compid: Some(compid),
            payload: 0..0,
            name: name.to_string(),
            kind: EntryKind::Mavlink { msg_id: 0, version: mavlink::MavlinkVersion::V2 },
            source: LogSourceId::Primary,
        }
    }

    #[test]
    fn parses_filter_expressions() {
        let exprs = parse_filters("1:1 HEARTBEAT, GPS*, 255,  :50").unwrap();
        assert_eq!(exprs.len(), 4);
        assert_eq!((exprs[0].sysid, exprs[0].compid), (Some(1), Some(1)));
        assert_eq!(exprs[0].name.as_deref(), Some("heartbeat"));
        assert_eq!((exprs[1].sysid, exprs[1].compid), (None, None));
        assert_eq!((exprs[2].sysid, exprs[2].compid), (Some(255), None));
        assert!(exprs[2].name.is_none());
        assert_eq!((exprs[3].sysid, exprs[3].compid), (None, Some(50)));
        assert!(exprs.iter().all(|e| e.enabled));

        assert!(parse_filters("").unwrap().is_empty());
        assert!(parse_filters("999 FOO").is_err());
        assert!(parse_filters("1:1 2:2").is_err());
        assert!(parse_filters("FOO BAR").is_err());
    }

    #[test]
    fn parses_disabled_expressions() {
        let exprs = parse_filters("=HEARTBEAT, !GPS*, !  1:1").unwrap();
        assert!(exprs[0].enabled);
        assert!(!exprs[1].enabled);
        assert_eq!(exprs[1].name.as_deref(), Some("gps*"));
        assert!(!exprs[2].enabled);
        assert_eq!((exprs[2].sysid, exprs[2].compid), (Some(1), Some(1)));

        // A '!' with nothing after it expresses intent that can't be honored.
        assert!(parse_filters("!").is_err());
        assert!(parse_filters("GPS, !").is_err());
    }

    #[test]
    fn exact_type_match() {
        let exprs = parse_filters("=ATTITUDE").unwrap();
        assert!(exprs[0].exact);
        assert!(exprs[0].matches(&entry(1, 1, "ATTITUDE")));
        assert!(!exprs[0].matches(&entry(1, 1, "ATTITUDE_TARGET")));

        // Substring filters (no '=') do match extensions of the name.
        let loose = parse_filters("ATTITUDE").unwrap();
        assert!(loose[0].matches(&entry(1, 1, "ATTITUDE_TARGET")));
    }

    #[test]
    fn filter_text_roundtrip() {
        for text in [
            "1:1 =HEARTBEAT",
            "GPS*",
            "255:*",
            "*:50 =VFR_HUD",
            "*",
            "!1:1 =HEARTBEAT",
            "!GPS*",
            "!*",
        ] {
            let exprs = parse_filters(text).unwrap();
            assert_eq!(exprs[0].to_text(), text, "roundtrip of '{text}'");
        }
    }

    #[test]
    fn filter_expressions_match() {
        let exprs = parse_filters("1:1 HEART, GPS*STATUS, 255").unwrap();
        let matches = |e: &LogEntry| exprs.iter().any(|f| f.matches(e));

        assert!(matches(&entry(1, 1, "HEARTBEAT")));
        assert!(!matches(&entry(2, 1, "HEARTBEAT")));
        assert!(matches(&entry(2, 1, "GPS_RAW_STATUS")));
        assert!(!matches(&entry(2, 1, "GPS_RAW_INT")));
        assert!(matches(&entry(255, 190, "PARAM_REQUEST_LIST")));
        assert!(!matches(&entry(1, 1, "ATTITUDE")));
    }

    #[test]
    fn source_qualifier_parses_and_matches() {
        let exprs = parse_filters("@bin GPS, @tlog").unwrap();
        assert_eq!(exprs[0].source, Some(LogSourceId::Secondary));
        assert_eq!(exprs[0].name.as_deref(), Some("gps"));
        assert_eq!(exprs[1].source, Some(LogSourceId::Primary));
        assert_eq!(exprs[0].to_text(), "@bin GPS"); // round-trips

        let mut e = entry(1, 1, "GPS");
        e.source = LogSourceId::Secondary;
        assert!(exprs[0].matches(&e));
        e.source = LogSourceId::Primary;
        assert!(!exprs[0].matches(&e)); // right type, wrong source

        assert!(parse_filters("@nope").is_err());
    }
}
