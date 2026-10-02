//! JavaScript's behaviour where the TS CLI's output depends on it: number
//! formatting, `Number()`, `trim()`, `toISOString()` and `new Date(string)`.
//! Output that matches the TS CLI byte for byte depends on these.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

/// `Number.prototype.toFixed`. Rust rounds an exact tie to even
/// (`format!("{:.2}", 0.125)` is `0.12`); JS takes the larger magnitude
/// (`0.13`). Everything else rounds the same way.
pub fn to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return chatgpt_core::js_number_string(value);
    }
    if value.abs() >= 1e21 {
        return chatgpt_core::js_number_string(value);
    }
    let magnitude = value.abs();
    // A tie at `digits` places is a multiple of 2^-k for small k, so its
    // whole expansion fits in 30 places, while any other double near one
    // differs within its first 18.
    let exact = format!("{magnitude:.30}");
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let tail = fraction.get(digits..).unwrap_or("");
    let tie = tail.starts_with('5') && tail.bytes().skip(1).all(|byte| byte == b'0');
    let text = if tie {
        round_half_up(whole, &fraction[..digits.min(fraction.len())], digits)
    } else {
        format!("{magnitude:.digits$}")
    };
    if value < 0.0 {
        format!("-{text}")
    } else {
        text
    }
}

/// `whole.kept` plus one unit in the last kept place.
fn round_half_up(whole: &str, kept: &str, digits: usize) -> String {
    // Increment the decimal string from the right, carrying.
    let mut chars: Vec<u8> = format!("{whole}{kept}").into_bytes();
    let mut index = chars.len();
    loop {
        if index == 0 {
            chars.insert(0, b'1');
            break;
        }
        index -= 1;
        if chars[index] == b'9' {
            chars[index] = b'0';
        } else {
            chars[index] += 1;
            break;
        }
    }
    let text = String::from_utf8(chars).unwrap_or_default();
    if digits == 0 {
        return text;
    }
    let split = text.len() - digits;
    format!("{}.{}", &text[..split], &text[split..])
}

pub use crate::js_regex::{JsRegex, JsString, regex};
pub use chatgpt_core::js::{number, trim};

/// How a template literal prints a JSON value (`${value}`): `undefined` for
/// a missing one, `null`, strings as they are, numbers as JS prints them,
/// arrays joined with commas and objects as `[object Object]`.
pub fn template(value: Option<&serde_json::Value>) -> String {
    use serde_json::Value;
    match value {
        None => "undefined".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Number(number)) => number
            .as_f64()
            .map_or_else(|| number.to_string(), chatgpt_core::js_number_string),
        Some(Value::Bool(flag)) => flag.to_string(),
        // `Array.prototype.join`: null elements print as nothing.
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => template(Some(item)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `Boolean(value)`.
pub fn truthy(value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `Date.prototype.toISOString` for a time in milliseconds since 1970:
/// `YYYY-MM-DDTHH:MM:SS.sssZ`. `None` where JS throws "Invalid time value".
pub fn iso_from_millis(millis: i64) -> Option<String> {
    // JS dates span ±8.64e15 ms; toISOString only formats 4-digit years
    // that way, which covers every date a chat can have.
    let time = DateTime::<Utc>::from_timestamp_millis(millis)?;
    Some(time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// `new Date().toISOString()`.
pub fn now_iso() -> String {
    iso_from_millis(Utc::now().timestamp_millis()).unwrap_or_default()
}

/// `new Date(seconds * 1000)`: JS truncates a fractional millisecond.
pub fn iso_from_seconds(seconds: f64) -> Option<String> {
    let millis = (seconds * 1000.0).trunc();
    if !millis.is_finite() || millis.abs() > 8.64e15 {
        return None;
    }
    iso_from_millis(millis as i64)
}

/// `Date.parse` for the ISO 8601 forms JS defines: a date alone is UTC, a
/// date and time without an offset is local time. Other formats, which V8
/// guesses at, are refused. Milliseconds since 1970.
pub fn parse_date(text: &str) -> Option<i64> {
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Some(time.timestamp_millis());
    }
    let (date_part, time_part) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let date = parse_date_only(date_part)?;
    let Some(time_part) = time_part else {
        return Some(
            Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))
                .timestamp_millis(),
        );
    };
    // With an offset: let RFC 3339 parse it, padding missing seconds.
    if let Some(at) = time_part.find(['Z', '+', '-']) {
        let (clock, offset) = time_part.split_at(at);
        let (clock, date) = parse_clock(clock, date)?;
        let with_seconds = format!(
            "{}T{}{offset}",
            date.format("%Y-%m-%d"),
            clock.format("%H:%M:%S%.f")
        );
        return DateTime::parse_from_rfc3339(&with_seconds)
            .ok()
            .map(|time| time.timestamp_millis());
    }
    let (clock, date) = parse_clock(time_part, date)?;
    let local = NaiveDateTime::new(date, clock);
    Local
        .from_local_datetime(&local)
        .earliest()
        .map(|time| time.timestamp_millis())
}

/// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`. V8 accepts a day up to 31 in any
/// month and rolls it over (`2024-02-30` is 1 March).
fn parse_date_only(text: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = text.split('-').collect();
    let number = |part: &str, len: usize| {
        (part.len() == len && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let (year, month, day) = match parts.as_slice() {
        [year] => (number(year, 4)?, 1, 1),
        [year, month] => (number(year, 4)?, number(month, 2)?, 1),
        [year, month, day] => (number(year, 4)?, number(month, 2)?, number(day, 2)?),
        _ => return None,
    };
    if !(1..=31).contains(&day) {
        return None;
    }
    let first = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, 1)?;
    first.checked_add_days(chrono::Days::new(u64::from(day - 1)))
}

/// `HH:mm`, `HH:mm:ss` or `HH:mm:ss.sss` on `date`. V8 reads `24:00`
/// (seconds and fraction zero) as the next day's midnight.
fn parse_clock(text: &str, date: NaiveDate) -> Option<(NaiveTime, NaiveDate)> {
    if let Some(rest) = text.strip_prefix("24:00") {
        let zero = rest.is_empty()
            || rest.strip_prefix(":00").is_some_and(|fraction| {
                fraction.is_empty()
                    || fraction.strip_prefix('.').is_some_and(|digits| {
                        !digits.is_empty() && digits.bytes().all(|byte| byte == b'0')
                    })
            });
        if !zero {
            return None;
        }
        return Some((NaiveTime::MIN, date.succ_opt()?));
    }
    let clock = ["%H:%M:%S%.f", "%H:%M"]
        .iter()
        .find_map(|format| NaiveTime::parse_from_str(text, format).ok())?;
    Some((clock, date))
}

/// `JSON.stringify(value)`: compact, numbers as JS prints them (`1e-7`,
/// `0.000001`, `1e+21`) where serde_json would write others, strings
/// escaped alike.
pub fn stringify(value: &serde_json::Value) -> String {
    use serde::Serialize;
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsNumbers);
    // Writing a `Value` to memory can't fail.
    let _ = value.serialize(&mut serializer);
    String::from_utf8(out).unwrap_or_default()
}

/// serde_json's compact layout with JS's number formatting.
struct JsNumbers;

impl serde_json::ser::Formatter for JsNumbers {
    fn write_f64<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        value: f64,
    ) -> std::io::Result<()> {
        writer.write_all(chatgpt_core::js_number_string(value).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stringify_writes_numbers_as_json_stringify_does() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"b":{"x":1e-7,"y":0.000001,"z":1e21,"w":0.9400000000000001},"a":[1,2.5,"q\"\u0001"]}"#)
                .expect("json");
        assert_eq!(
            stringify(&value),
            r#"{"b":{"x":1e-7,"y":0.000001,"z":1e+21,"w":0.9400000000000001},"a":[1,2.5,"q\"\u0001"]}"#
        );
    }

    #[test]
    fn template_prints_values_as_a_js_template_literal_does() {
        let value: serde_json::Value =
            serde_json::from_str(r#"[1, [2, [null, 3]], {"a": 1}, true, "x", 1e21, 0.1, null]"#)
                .expect("json");
        // `${[1,[2,[null,3]],{a:1},true,"x",1e21,0.1,null]}` in node.
        assert_eq!(
            template(Some(&value)),
            "1,2,,3,[object Object],true,x,1e+21,0.1,"
        );
        assert_eq!(template(Some(&serde_json::json!({}))), "[object Object]");
        assert_eq!(template(Some(&serde_json::json!([[]]))), "");
        assert_eq!(template(None), "undefined");
    }

    #[test]
    fn to_fixed_rounds_ties_up_as_js_does() {
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(0.375, 2), "0.38");
        assert_eq!(to_fixed(0.25, 1), "0.3");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(9.995, 2), "9.99", "not a tie in binary");
        assert_eq!(to_fixed(0.995, 2), "0.99", "0.995 is below the tie");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(0.9999, 2), "1.00");
        assert_eq!(to_fixed(3.0, 1), "3.0");
        assert_eq!(to_fixed(0.0, 2), "0.00");
        assert_eq!(to_fixed(-0.125, 2), "-0.13");
        assert_eq!(to_fixed(99.5, 0), "100");
    }

    #[test]
    fn iso_strings_match_to_iso_string() {
        assert_eq!(
            iso_from_millis(1_735_689_600_000).as_deref(),
            Some("2025-01-01T00:00:00.000Z")
        );
        assert_eq!(
            iso_from_seconds(1_735_689_600.123_456).as_deref(),
            Some("2025-01-01T00:00:00.123Z")
        );
        assert_eq!(iso_from_seconds(f64::NAN), None);
    }

    #[test]
    fn dates_parse_as_js_does() {
        assert_eq!(parse_date("2025-01-01"), Some(1_735_689_600_000));
        assert_eq!(parse_date("2025-01"), Some(1_735_689_600_000));
        assert_eq!(parse_date("2025-01-01T00:00:00Z"), Some(1_735_689_600_000));
        assert_eq!(
            parse_date("2025-01-01T02:00+02:00"),
            Some(1_735_689_600_000)
        );
        assert_eq!(
            parse_date("2024-02-30"),
            parse_date("2024-03-01"),
            "V8 rolls the day over"
        );
        assert_eq!(parse_date("2024-02-32"), None);
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(parse_date("yesterday"), None);
        assert!(parse_date("2025-01-01T00:00").is_some(), "local time");
    }

    #[test]
    fn hour_24_is_the_next_midnight_as_in_v8() {
        // Date.parse values from node.
        assert_eq!(parse_date("2026-01-01T24:00Z"), Some(1_767_312_000_000));
        assert_eq!(
            parse_date("2026-01-01T24:00:00.000Z"),
            Some(1_767_312_000_000)
        );
        assert_eq!(
            parse_date("2026-01-01T24:00+02:00"),
            Some(1_767_304_800_000)
        );
        assert_eq!(parse_date("2026-12-31T24:00Z"), Some(1_798_761_600_000));
        assert_eq!(
            parse_date("2026-01-01T24:00"),
            parse_date("2026-01-02T00:00")
        );
        assert_eq!(parse_date("2026-01-01T24:01"), None);
        assert_eq!(parse_date("2026-01-01T24:00:01"), None);
        assert_eq!(parse_date("2026-01-01T24:00:00.001"), None);
    }
}
