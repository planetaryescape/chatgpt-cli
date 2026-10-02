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

/// JS's `\s`: WhiteSpace and LineTerminator, without U+0085.
const JS_SPACE: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// A JS regex source (no `u` flag) rewritten for Rust's regex syntax, so
/// it matches what `new RegExp(source)` would: `\w`, `\d` and `\b` are
/// ASCII-only, `\s` is JS's whitespace set, and `.` stops at every JS line
/// terminator, not only `\n`. Everything else passes through.
fn regex_source(source: &str) -> String {
    const WORD: &str = "0-9A-Za-z_";
    let mut out = String::with_capacity(source.len() + 16);
    let mut chars = source.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(next) = chars.next() else {
                    out.push('\\');
                    break;
                };
                let replacement = match (next, in_class) {
                    ('w', false) => Some(format!("[{WORD}]")),
                    ('w', true) => Some(WORD.to_owned()),
                    ('W', false) => Some(format!("[^{WORD}]")),
                    ('d', false) => Some("[0-9]".to_owned()),
                    ('d', true) => Some("0-9".to_owned()),
                    ('D', false) => Some("[^0-9]".to_owned()),
                    // fancy-regex has no `(?-u:\b)`: spell the ASCII
                    // boundary out with lookarounds.
                    ('b', false) => Some(format!(
                        "(?:(?<={W})(?!{W})|(?<!{W})(?={W}))",
                        W = format!("[{WORD}]")
                    )),
                    ('B', false) => Some(format!(
                        "(?:(?<={W})(?={W})|(?<!{W})(?!{W}))",
                        W = format!("[{WORD}]")
                    )),
                    ('s', false) => Some(format!("[{JS_SPACE}]")),
                    ('s', true) => Some(JS_SPACE.to_owned()),
                    ('S', false) => Some(format!("[^{JS_SPACE}]")),
                    _ => None,
                };
                match replacement {
                    Some(text) => out.push_str(&text),
                    None => {
                        out.push('\\');
                        out.push(next);
                    }
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push('[');
                // A leading `^` negates the class: keep it ahead of any
                // translated escape.
                if chars.next_if_eq(&'^').is_some() {
                    out.push('^');
                }
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '.' if !in_class => out.push_str(r"[^\n\r\x{2028}\x{2029}]"),
            _ => out.push(c),
        }
    }
    out
}

/// `new RegExp(source, flags)` for flags `""` or `"i"`: every JS regex the
/// TS CLI builds goes through here, so none skips the translation.
pub fn regex(
    source: &str,
    case_insensitive: bool,
) -> Result<fancy_regex::Regex, fancy_regex::Error> {
    fancy_regex::RegexBuilder::new(&regex_source(source))
        .case_insensitive(case_insensitive)
        .build()
}

pub use chatgpt_core::js::{number, trim};

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
        let clock = parse_clock(clock)?;
        let with_seconds = format!(
            "{}T{}{offset}",
            date.format("%Y-%m-%d"),
            clock.format("%H:%M:%S%.f")
        );
        return DateTime::parse_from_rfc3339(&with_seconds)
            .ok()
            .map(|time| time.timestamp_millis());
    }
    let local = NaiveDateTime::new(date, parse_clock(time_part)?);
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

/// `HH:mm`, `HH:mm:ss` or `HH:mm:ss.sss`.
fn parse_clock(text: &str) -> Option<NaiveTime> {
    ["%H:%M:%S%.f", "%H:%M"]
        .iter()
        .find_map(|format| NaiveTime::parse_from_str(text, format).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn regexes_match_as_js_regexes_without_the_u_flag() {
        let js = |source: &str| regex(source, false).expect("valid");
        let word = js(r"\w+");
        assert_eq!(
            word.find("café").expect("ok").map(|m| m.as_str()),
            Some("caf")
        );
        assert!(
            js(r"\bé").is_match("café").expect("ok"),
            "é isn't a JS word character"
        );
        assert!(!js(r"\d").is_match("٣").expect("ok"), "only ASCII digits");
        assert!(js(r"a.b").is_match("a\u{85}b").expect("ok"));
        assert!(!js(r"a.b").is_match("a\u{2028}b").expect("ok"));
        assert!(js(r"\s").is_match("\u{FEFF}").expect("ok"));
        assert!(!js(r"\s").is_match("\u{85}").expect("ok"));
        assert!(js(r"[\w-]+$").is_match("a-b").expect("ok"));
        assert!(
            js(r"[^.]x").is_match("ax").expect("ok"),
            "a class keeps its own dot"
        );
        assert!(js(r"\.").is_match(".").expect("ok"));
    }
}
