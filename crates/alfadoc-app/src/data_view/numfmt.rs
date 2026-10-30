//! Number formats for the Data tab, using GridCraft's Excel format codes (`#,##0.00`, `0%`,
//! `yyyy-mm-dd`, `[Red]-0`…) so what a person knows from Excel works here.

use alfadoc_model::Value;
use gridcraft_core::DateSystem;
use gridcraft_numfmt::NumberFormat;

/// Milliseconds in a day, and the Excel serial number of 1970-01-01.
const DAY_MS: f64 = 86_400_000.0;
const UNIX_EPOCH_SERIAL: f64 = 25_569.0;

/// How a value looks under a format code.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub text: String,
    /// A colour the format asks for (`[Red]`).
    pub color: Option<[u8; 3]>,
    /// Numbers line up to the right unless the format is for text.
    pub numeric: bool,
}

fn engine_value(v: &Value) -> Option<gridcraft_core::Value> {
    Some(match v {
        Value::Null => return None,
        Value::Int(i) => gridcraft_core::Value::Number(*i as f64),
        Value::Float(f) => gridcraft_core::Value::Number(*f),
        Value::Date(ms) => gridcraft_core::Value::Number(*ms as f64 / DAY_MS + UNIX_EPOCH_SERIAL),
        Value::Bool(b) => gridcraft_core::Value::Bool(*b),
        Value::Text(t) => gridcraft_core::Value::Text(t.as_str().into()),
    })
}

/// The value under `code`. `None` when the code is empty (General) or the cell is empty: the
/// caller then shows the value as it is.
pub fn display(value: &Value, code: &str) -> Option<Shown> {
    if code.trim().is_empty() {
        return None;
    }
    let v = engine_value(value)?;
    let fmt = NumberFormat::parse_cached(code);
    let out = gridcraft_numfmt::format_value(&v, &fmt, DateSystem::D1900);
    Some(Shown { text: out.text, color: out.color.and_then(gridcraft_numfmt::FormatColor::rgb), numeric: out.numeric })
}

/// The formats offered in the menus: (name, code).
pub const PRESETS: &[(&str, &str)] = &[
    ("General", ""),
    ("Number  1234.50", "0.00"),
    ("Thousands  1,234.50", "#,##0.00"),
    ("Integer  1,235", "#,##0"),
    ("Currency  $1,234.50", "$#,##0.00"),
    ("Accounting", "_($* #,##0.00_);_($* (#,##0.00);_($* \"-\"??_);_(@_)"),
    ("Percent  12.5%", "0.0%"),
    ("Scientific  1.23E+03", "0.00E+00"),
    ("Date  2026-03-01", "yyyy-mm-dd"),
    ("Long date  1 March 2026", "d mmmm yyyy"),
    ("Time  14:30", "hh:mm"),
    ("Date and time", "yyyy-mm-dd hh:mm"),
    ("Text", "@"),
];

/// The name of the preset for `code`, or "Custom".
pub fn name_of(code: &str) -> String {
    PRESETS.iter().find(|(_, c)| *c == code).map_or_else(|| "Custom".to_string(), |(n, _)| n.split("  ").next().unwrap_or(n).to_string())
}

/// `code` with `delta` more (or fewer) decimals in its first section; General becomes `0.0`.
pub fn with_decimals(code: &str, delta: i32) -> String {
    if code.trim().is_empty() {
        return if delta > 0 { "0.0".to_string() } else { String::new() };
    }
    let (first, rest) = match code.find(';') {
        Some(i) => (&code[..i], &code[i..]),
        None => (code, ""),
    };
    let chars: Vec<char> = first.chars().collect();
    let placeholder = |c: &char| *c == '0' || *c == '#';
    let out: String = if let Some(dot) = chars.iter().position(|c| *c == '.') {
        let mut end = dot + 1;
        while chars.get(end).is_some_and(|c| *c == '0' || *c == '#') {
            end += 1;
        }
        let have = i32::try_from(end - dot - 1).unwrap_or(0);
        let want = (have + delta).clamp(0, 15);
        let head: String = chars[..dot].iter().collect();
        let tail: String = chars[end..].iter().collect();
        if want == 0 { format!("{head}{tail}") } else { format!("{head}.{}{tail}", "0".repeat(usize::try_from(want).unwrap_or(0))) }
    } else if delta > 0 {
        match chars.iter().rposition(placeholder) {
            Some(last) => {
                let head: String = chars[..=last].iter().collect();
                let tail: String = chars[last + 1..].iter().collect();
                format!("{head}.{}{tail}", "0".repeat(usize::try_from(delta.min(15)).unwrap_or(1)))
            }
            None => first.to_string(),
        }
    } else {
        first.to_string()
    };
    format!("{out}{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_is_left_to_the_caller() {
        assert_eq!(display(&Value::Int(5), ""), None);
        assert_eq!(display(&Value::Null, "0.00"), None);
    }

    #[test]
    fn excel_codes_format_numbers_dates_and_percentages() {
        assert_eq!(display(&Value::Float(1234.5), "#,##0.00").unwrap().text, "1,234.50");
        assert_eq!(display(&Value::Float(0.125), "0.0%").unwrap().text, "12.5%");
        assert_eq!(display(&Value::Int(7), "0.00").unwrap().text, "7.00");
        // 2026-03-01 as milliseconds since 1970.
        let ms = 1_772_323_200_000i64;
        assert_eq!(display(&Value::Date(ms), "yyyy-mm-dd").unwrap().text, "2026-03-01");
        assert!(display(&Value::Int(5), "0.00").unwrap().numeric);
    }

    #[test]
    fn a_format_can_ask_for_a_colour() {
        let shown = display(&Value::Int(-3), "0;[Red]-0").unwrap();
        assert_eq!(shown.text, "-3");
        assert_eq!(shown.color, Some([255, 0, 0]));
    }

    #[test]
    fn decimals_are_added_and_removed_in_the_code() {
        assert_eq!(with_decimals("0.00", 1), "0.000");
        assert_eq!(with_decimals("0.00", -2), "0");
        assert_eq!(with_decimals("0.00", -5), "0");
        assert_eq!(with_decimals("#,##0", 1), "#,##0.0");
        assert_eq!(with_decimals("0%", 1), "0.0%");
        assert_eq!(with_decimals("", 1), "0.0");
        assert_eq!(with_decimals("", -1), "");
        assert_eq!(with_decimals("yyyy", 1), "yyyy");
        assert_eq!(with_decimals("0.0;[Red]-0.0", 1), "0.00;[Red]-0.0");
    }

    #[test]
    fn presets_have_names() {
        assert_eq!(name_of(""), "General");
        assert_eq!(name_of("0.0%"), "Percent");
        assert_eq!(name_of("0.000000"), "Custom");
    }
}
