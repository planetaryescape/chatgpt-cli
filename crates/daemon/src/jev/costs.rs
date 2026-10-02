//! The cost lines the Jev guard prints, as the TS CLI's `CostMeter`
//! (`src/classify/costs.ts` @ 1b8c950) prints them. The guard only ever
//! calls Jev, which is billed to the user's TypeSafe account.

use crate::js::to_fixed;

/// Jev's published price, USD per million input tokens (output is free):
/// https://docs.typesafe.ai/models.md, 2026-09-27.
const JEV_INPUT_PER_MILLION: f64 = 0.042;

#[derive(Default)]
pub struct CostMeter {
    calls: u64,
    tokens: u64,
    usd: f64,
}

impl CostMeter {
    /// One Jev call (`meter.add("Jev", "api", priceOf("jev", …), tokens)`).
    pub fn add_jev(&mut self, input_tokens: u64) {
        let tokens = input_tokens as f64;
        // priceOf: (uncached × input + cached × cachedInput + output × 0) / 1e6.
        self.usd += (tokens * JEV_INPUT_PER_MILLION + 0.0 * JEV_INPUT_PER_MILLION + 0.0) / 1e6;
        self.calls += 1;
        self.tokens += input_tokens;
    }

    pub fn total(&self) -> f64 {
        self.usd
    }

    /// `report()`: nothing without a call.
    pub fn report(&self) -> Vec<String> {
        if self.calls == 0 {
            return Vec::new();
        }
        let total = format_usd(self.usd);
        vec![
            "Cost this run:".to_owned(),
            format!(
                "  {:<14} {total:>8}  {} call(s), {}k tokens (billed)",
                "Jev",
                self.calls,
                (self.tokens as f64 / 1000.0).round()
            ),
            format!("  {:<14} {total:>8}  of which billed: {total}", "total"),
        ]
    }
}

/// `formatUsd`: four places under a cent, else two.
pub fn format_usd(usd: f64) -> String {
    if usd < 0.01 && usd > 0.0 {
        format!("${}", to_fixed(usd, 4))
    } else {
        format!("${}", to_fixed(usd, 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_print_as_the_ts_meter_prints_them() {
        let mut meter = CostMeter::default();
        assert!(meter.report().is_empty());
        meter.add_jev(1500);
        meter.add_jev(2499);
        assert_eq!(
            meter.report(),
            [
                "Cost this run:",
                "  Jev             $0.0002  2 call(s), 4k tokens (billed)",
                "  total           $0.0002  of which billed: $0.0002",
            ]
        );
        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(0.125), "$0.13");
    }
}
