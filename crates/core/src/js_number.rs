//! `Number.prototype.toString` (ECMA-262 Number::toString), which
//! `JSON.stringify` uses. Output that must match the TS CLI byte for byte,
//! such as `list --json`, prints numbers this way: Rust's formatters and
//! serde_json write `1e-6` and `1e21` where JS writes `0.000001` and
//! `1e+21`.

/// The shortest round-trip digits, placed as JS places them: plain
/// decimals from 1e-6 up to but not including 1e21, exponent form outside
/// that (1e-7, 1e+21).
pub fn js_number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        // JS prints -0 as "0" too.
        return "0".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // `{:e}` gives Rust's shortest round-trip digits: `d.ddde±x`.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i64;
    // `n`: where the decimal point goes, counting from the first digit.
    let n = exponent.parse::<i64>().unwrap_or(0) + 1;
    let text = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let exponent = n - 1;
        let sign = if exponent < 0 { "-" } else { "+" };
        let mantissa = if k == 1 {
            digits.clone()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!("{mantissa}e{sign}{}", exponent.abs())
    };
    format!("{sign}{text}")
}

#[cfg(test)]
mod tests {
    use super::js_number_string;

    /// Each pair is what Bun 1.3 prints for the number.
    #[test]
    fn numbers_print_as_js_prints_them() {
        for (value, js) in [
            (0.94, "0.94"),
            (0.9400000000000001, "0.9400000000000001"),
            (3.0, "3"),
            (-2.5, "-2.5"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (0.000001234, "0.000001234"),
            (1.5e-10, "1.5e-10"),
            (1e21, "1e+21"),
            (123456789012345680000.0, "123456789012345680000"),
            (1.2345e25, "1.2345e+25"),
            (100.0, "100"),
            (0.1 + 0.2, "0.30000000000000004"),
        ] {
            assert_eq!(js_number_string(value), js, "{value:e}");
        }
    }
}
