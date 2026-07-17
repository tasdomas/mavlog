//! Custom list columns: definitions and their text syntax.

/// A user-defined list column showing the last-seen value of one field of
/// one message type.
pub struct CustomColumn {
    pub name: String,
    pub sysid: Option<u8>,
    pub compid: Option<u8>,
    /// Exact message-type name, uppercase.
    pub msg_type: String,
    pub field: String,
    /// Entry indices of messages this column reads from, ascending.
    pub matches: Vec<usize>,
}

impl CustomColumn {
    pub fn to_text(&self) -> String {
        let mut expr = String::new();
        if self.sysid.is_some() || self.compid.is_some() {
            let part = |v: Option<u8>| v.map_or("*".to_string(), |v| v.to_string());
            expr = format!("{}:{} ", part(self.sysid), part(self.compid));
        }
        format!("{} = {expr}{}.{}", self.name, self.msg_type, self.field)
    }
}

/// Parse comma-separated column definitions: `NAME = [sys[:comp]] TYPE.FIELD`,
/// e.g. "alt = 1:1 GLOBAL_POSITION_INT.relative_alt". Empty input clears.
pub fn parse_columns(input: &str) -> Result<Vec<CustomColumn>, String> {
    let mut columns = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let usage = || format!("expected NAME = [sys[:comp]] TYPE.FIELD in '{part}'");
        let (name, expr) = part.split_once('=').ok_or_else(usage)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(usage());
        }
        let (mut sysid, mut compid, mut type_field) = (None, None, None);
        for token in expr.split_whitespace() {
            let is_id_spec = token
                .chars()
                .all(|c| c.is_ascii_digit() || c == ':' || c == '*');
            if is_id_spec {
                if sysid.is_some() || compid.is_some() {
                    return Err(format!("more than one id spec in '{part}'"));
                }
                let (sys, comp) = token.split_once(':').unwrap_or((token, ""));
                let parse_id = |s: &str| -> Result<Option<u8>, String> {
                    if s.is_empty() || s == "*" {
                        return Ok(None);
                    }
                    s.parse()
                        .map(Some)
                        .map_err(|_| format!("bad id '{s}' in '{part}'"))
                };
                sysid = parse_id(sys)?;
                compid = parse_id(comp)?;
            } else if let Some((msg_type, field)) = token.split_once('.') {
                if type_field.is_some() || msg_type.is_empty() || field.is_empty() {
                    return Err(usage());
                }
                type_field = Some((msg_type.to_ascii_uppercase(), field.to_string()));
            } else {
                return Err(usage());
            }
        }
        let (msg_type, field) = type_field.ok_or_else(usage)?;
        columns.push(CustomColumn {
            name: name.to_string(),
            sysid,
            compid,
            msg_type,
            field,
            matches: Vec::new(),
        });
    }
    Ok(columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_column_definitions() {
        let cols = parse_columns("alt = 1:1 GLOBAL_POSITION_INT.relative_alt, spd = vfr_hud.groundspeed").unwrap();
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "alt");
        assert_eq!((cols[0].sysid, cols[0].compid), (Some(1), Some(1)));
        assert_eq!(cols[0].msg_type, "GLOBAL_POSITION_INT");
        assert_eq!(cols[0].field, "relative_alt");
        assert_eq!(cols[0].to_text(), "alt = 1:1 GLOBAL_POSITION_INT.relative_alt");
        assert_eq!((cols[1].sysid, cols[1].compid), (None, None));
        assert_eq!(cols[1].to_text(), "spd = VFR_HUD.groundspeed");

        assert!(parse_columns("").unwrap().is_empty());
        assert!(parse_columns("noexpr").is_err());
        assert!(parse_columns("x = HEARTBEAT").is_err()); // missing .field
        assert!(parse_columns("x = 1:1").is_err()); // missing type
        assert!(parse_columns("= HEARTBEAT.custom_mode").is_err());
    }
}
