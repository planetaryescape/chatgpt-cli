//! JavaScript's text and number semantics the TS CLI's output depends on:
//! `String.prototype.trim`, `\s`, and `Number(text)`.

/// JS whitespace and line terminators: `str::trim` plus U+FEFF, minus
/// U+0085, which Unicode calls whitespace and JS doesn't.
pub fn is_space(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}')
}

/// `String.prototype.trim`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `Number(text)`: NaN when JS would give NaN.
pub fn number(text: &str) -> f64 {
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return radix_integer(digits, radix);
        }
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "Infinity" {
        return if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // Rust's parser also reads "inf", "nan" and "infinity"; JS's grammar
    // reads none of them.
    if !is_decimal_literal(unsigned) {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

/// `StrUnsignedDecimalLiteral` without `Infinity`: digits with an optional
/// fraction (either side of the point may be empty, not both) and an
/// optional exponent with digits.
fn is_decimal_literal(text: &str) -> bool {
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (text, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    mantissa_ok && exponent_ok
}

/// A `0x`/`0o`/`0b` literal's digits as JS reads them: no sign, at least
/// one digit, and any length, rounded to the nearest double (to infinity
/// past the largest).
fn radix_integer(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return f64::NAN;
    }
    if let Ok(value) = u128::from_str_radix(digits, radix) {
        return value as f64;
    }
    // Too long for u128: every radix here is a power of two, so the value
    // is its bits. Keep the top 64 and fold the rest into the lowest
    // (sticky) bit, which rounds the same way as the whole would.
    let bits_per_digit = radix.trailing_zeros();
    let bits: Vec<bool> = digits
        .trim_start_matches('0')
        .chars()
        .filter_map(|c| c.to_digit(radix))
        .flat_map(|digit| {
            (0..bits_per_digit)
                .rev()
                .map(move |bit| digit >> bit & 1 == 1)
        })
        .skip_while(|bit| !bit)
        .collect();
    let (top, rest) = bits.split_at(64);
    let mut leading = top
        .iter()
        .fold(0u64, |value, &bit| value << 1 | u64::from(bit));
    if rest.iter().any(|&bit| bit) {
        leading |= 1;
    }
    let scale = i32::try_from(rest.len()).unwrap_or(i32::MAX);
    leading as f64 * 2f64.powi(scale)
}

/// `text.replace(/\s+/g, " ")`: every run of JS whitespace as one space.
pub fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// `text.length`: UTF-16 code units, as JS counts them.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest prefix of `text` within `units` UTF-16 units (`slice(0,
/// units)`) that doesn't split a character.
pub fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (at, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &text[..at];
        }
    }
    text
}

/// `review`'s `clip`: `text.replace(/\s+/g, " ").trim()`, cut to `max`
/// UTF-16 units with `…` (never through a character).
pub fn clip(text: &str, max: usize) -> String {
    let collapsed = collapse_spaces(text);
    let one_line = trim(&collapsed);
    if utf16_len(one_line) > max {
        format!("{}…", utf16_prefix(one_line, max))
    } else {
        one_line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn clip_collapses_whitespace_and_cuts_in_utf16_units() {
        use super::clip;
        assert_eq!(clip("  a\n\n b\tc  ", 10), "a b c");
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("ab😀cd", 3), "ab…", "never half an emoji");
        assert_eq!(clip("abc", 3), "abc");
    }

    use super::*;

    #[test]
    fn utf16_prefixes_never_split_a_character() {
        let text = format!("{}😀tail", "a".repeat(249));
        assert_eq!(utf16_prefix(&text, 250), "a".repeat(249));
        assert_eq!(utf16_prefix("short", 250), "short");
        assert_eq!(utf16_len("😀a"), 3);
    }

    #[test]
    fn number_reads_what_js_reads() {
        assert_eq!(number("5"), 5.0);
        assert_eq!(number(" 5 "), 5.0);
        assert_eq!(number("1e2"), 100.0);
        assert_eq!(number("0x10"), 16.0);
        assert_eq!(number(""), 0.0);
        assert_eq!(number("2.5"), 2.5);
        assert!(number("abc").is_nan());
        assert!(number("inf").is_nan());
        assert!(number("5d").is_nan());
        assert_eq!(number("Infinity"), f64::INFINITY);
    }

    #[test]
    fn number_rejects_what_js_rejects() {
        for text in [
            "0x+1",
            "0x-1",
            "0x",
            "+0x1",
            "1e",
            "e5",
            ".",
            "+",
            "-",
            "1.2.3",
            "1e+",
            "--1",
            "+-1",
            "Infinityx",
            "1_000",
            "0x1g",
        ] {
            assert!(number(text).is_nan(), "{text}");
        }
        assert_eq!(number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(number("+Infinity"), f64::INFINITY);
        assert_eq!(number(".5"), 0.5);
        assert_eq!(number("5."), 5.0);
        assert_eq!(number("+1.e2"), 100.0);
        assert_eq!(number("-1E-2"), -0.01);
        assert_eq!(number("0b101"), 5.0);
        assert_eq!(number("0O17"), 15.0);
    }

    #[test]
    fn radix_literals_past_128_bits_round_as_js_does() {
        // Number("0x" + "f".repeat(40)) and friends, from node.
        assert_eq!(
            number(&format!("0x{}", "f".repeat(40))),
            1.461501637330903e48
        );
        assert_eq!(number(&format!("0x1{}", "0".repeat(32))), 2f64.powi(128));
        // 2^53 + 1 is a tie: the bits past it (sticky) round it up.
        assert_eq!(
            number(&format!("0x20000000000001{}1", "0".repeat(20))),
            9007199254740994.0 * 2f64.powi(84)
        );
        assert_eq!(
            number(&format!("0x20000000000001{}", "0".repeat(21))),
            9007199254740992.0 * 2f64.powi(84)
        );
        assert_eq!(number(&format!("0x{}", "f".repeat(300))), f64::INFINITY);
        assert_eq!(
            number(&format!("0b{}", "1".repeat(200))),
            1.6069380442589903e60
        );
    }

    #[test]
    fn trim_matches_js() {
        assert_eq!(trim("\u{FEFF} x \n"), "x");
        assert_eq!(trim("\u{85}x"), "\u{85}x");
    }

    #[test]
    fn whitespace_runs_collapse_as_js_replace_does() {
        assert_eq!(collapse_spaces("a \n\t b\u{FEFF}\u{85}c"), "a b \u{85}c");
    }
}
