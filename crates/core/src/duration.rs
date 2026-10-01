//! `formatDuration` from the TS CLI's `src/progress.ts` @ 1b8c950, which
//! the daemon's progress lines and the CLI's sync summary both print.

/// `1.2s` under 10 seconds, `12s`, `1m05s`, `1h02m`.
pub fn format_duration(ms: f64) -> String {
    let seconds = (ms / 1000.0).round() as u64;
    if seconds < 60 {
        if ms < 10_000.0 {
            // `(ms / 1000).toFixed(1)`: halves round up, as JS does.
            let tenths = (ms / 100.0).round() as u64;
            return format!("{}.{}s", tenths / 10, tenths % 10);
        }
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m{:02}s", seconds % 60);
    }
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn durations_read_as_the_ts_clis() {
        assert_eq!(format_duration(1234.0), "1.2s");
        assert_eq!(format_duration(1250.0), "1.3s");
        assert_eq!(format_duration(12_345.0), "12s");
        assert_eq!(format_duration(65_000.0), "1m05s");
        assert_eq!(format_duration(3_725_000.0), "1h02m");
    }
}
