//! Cells that only hold text.
//!
//! A Word table cell or a slide table cell stores text, not typed values. Linking such a cell to
//! a sheet needs one agreed way to turn a value into text and back, and the round trip must be
//! stable: if `12` is written as "12" and read back as the text "12", the sync layer would see a
//! change every time and overwrite the sheet's number with text.
//!
//! The rule: text is read back as a number, boolean or date **only if** writing that value would
//! produce exactly that text. Everything else stays text (`007`, `1,5`, `+1`, `1e3`, " 12").

use alfadoc_model::Value;

/// Value → the text a cell shows. `Null` is the empty cell.
pub fn to_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(t) => t.clone(),
        Value::Bool(true) => "TRUE".into(),
        Value::Bool(false) => "FALSE".into(),
        Value::Date(ms) => format_date(*ms),
    }
}

/// Cell text → value. Never fails: anything that is not an exact round trip stays text.
pub fn from_text(text: &str) -> Value {
    if text == "TRUE" {
        return Value::Bool(true);
    }
    if text == "FALSE" {
        return Value::Bool(false);
    }
    if let Ok(i) = text.parse::<i64>()
        && i.to_string() == text
    {
        return Value::Int(i);
    }
    if let Ok(f) = text.parse::<f64>()
        && f.is_finite()
        && f.to_string() == text
    {
        return Value::Float(f);
    }
    if let Some(ms) = parse_date(text)
        && format_date(ms) == text
    {
        return Value::Date(ms);
    }
    Value::Text(text.to_string())
}

const DAY_MS: i64 = 86_400_000;

/// `2026-01-01` at midnight UTC, `2026-01-01T13:45:09Z` otherwise (seconds resolution).
fn format_date(ms: i64) -> String {
    let days = ms.div_euclid(DAY_MS);
    let rest = ms.rem_euclid(DAY_MS) / 1000;
    let (y, m, d) = civil_from_days(days);
    if rest == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rest / 3600, (rest / 60) % 60, rest % 60)
    }
}

fn parse_date(text: &str) -> Option<i64> {
    let (date, time) = match text.split_once('T') {
        Some((d, t)) => (d, Some(t.strip_suffix('Z')?)),
        None => (text, None),
    };
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = 0;
    if let Some(t) = time {
        let mut p = t.split(':');
        let h: i64 = p.next()?.parse().ok()?;
        let mi: i64 = p.next()?.parse().ok()?;
        let s: i64 = p.next()?.parse().ok()?;
        if p.next().is_some() || !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..60).contains(&s) {
            return None;
        }
        secs = h * 3600 + mi * 60 + s;
    }
    days_from_civil(y, m, d).checked_mul(DAY_MS)?.checked_add(secs * 1000)
}

// Howard Hinnant's civil-calendar algorithms (public domain).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trips(v: Value) {
        assert_eq!(from_text(&to_text(&v)), v, "{v:?}");
    }

    #[test]
    fn typed_values_survive_a_trip_through_text() {
        round_trips(Value::Int(0));
        round_trips(Value::Int(-42));
        round_trips(Value::Int(9_007_199_254_740_993));
        round_trips(Value::Float(3.5));
        round_trips(Value::Float(-0.001));
        round_trips(Value::Bool(true));
        round_trips(Value::Bool(false));
        round_trips(Value::Text("hello".into()));
        round_trips(Value::Date(1_767_225_600_000)); // 2026-01-01
        round_trips(Value::Date(1_767_225_600_000 + 13 * 3_600_000 + 45 * 60_000 + 9_000));
    }

    #[test]
    fn text_that_only_looks_like_a_number_stays_text() {
        for t in ["007", "+1", "1e3", " 12", "12 ", "1,5", "05.04.2026", "1.50", "TRUE ", "true", "NaN", "inf"] {
            assert_eq!(from_text(t), Value::Text(t.into()), "{t}");
        }
    }

    #[test]
    fn empty_is_the_empty_text_and_null_writes_empty() {
        assert_eq!(to_text(&Value::Null), "");
        assert_eq!(from_text(""), Value::Text(String::new()));
    }

    #[test]
    fn impossible_dates_stay_text() {
        assert_eq!(from_text("2026-13-01"), Value::Text("2026-13-01".into()));
        assert_eq!(from_text("2026-01-01T25:00:00Z"), Value::Text("2026-01-01T25:00:00Z".into()));
    }

    #[test]
    fn calendar_helpers_agree_with_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 1, 1), 20_454);
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
