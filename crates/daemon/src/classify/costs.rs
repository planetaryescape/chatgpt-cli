//! What a run's model calls cost, as the TS CLI's `CostMeter`
//! (`src/classify/costs.ts` @ 1b8c950) adds it up and prints it: one line
//! per service, in the order each was first used, then the total and how
//! much of it was billed. Subscription-covered calls (Codex, Claude) are
//! counted at their API-equivalent price so the numbers compare.

use crate::js::to_fixed;

/// USD per million tokens. Jev: https://docs.typesafe.ai/models.md
/// (2026-09-27), input only, output free. gpt-6-luna: OpenAI's standard
/// rate via layer3labs.io and eesel.ai (2026-09-27).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
}

pub const JEV: Price = Price {
    input: 0.042,
    cached_input: 0.042,
    output: 0.0,
};

pub const GPT_6_LUNA: Price = Price {
    input: 0.1,
    cached_input: 0.01,
    output: 0.5,
};

/// claude-haiku-4-5, as `anthropicText` prices it: $1 in, $5 out.
pub const HAIKU: Price = Price {
    input: 1.0,
    cached_input: 1.0,
    output: 5.0,
};

/// `priceOf`: cached input at its own rate, the rest at the input rate.
pub fn price_of(price: Price, input: u64, cached_input: u64, output: u64) -> f64 {
    let uncached = input.saturating_sub(cached_input) as f64;
    (uncached * price.input
        + cached_input as f64 * price.cached_input
        + output as f64 * price.output)
        / 1e6
}

/// Who pays for a service's calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaidBy {
    /// Billed to the user's API account.
    Api,
    /// Covered by the user's ChatGPT or Claude subscription.
    Subscription,
}

#[derive(Debug)]
struct Line {
    service: String,
    calls: u64,
    tokens: u64,
    usd: f64,
    paid_by: PaidBy,
}

#[derive(Debug, Default)]
pub struct CostMeter {
    lines: Vec<Line>,
}

impl CostMeter {
    /// `add(service, paidBy, usd, tokens)`: a line keeps the payer it was
    /// first added with.
    pub fn add(&mut self, service: &str, paid_by: PaidBy, usd: f64, tokens: u64) {
        let index = match self.lines.iter().position(|line| line.service == service) {
            Some(index) => index,
            None => {
                self.lines.push(Line {
                    service: service.to_owned(),
                    calls: 0,
                    tokens: 0,
                    usd: 0.0,
                    paid_by,
                });
                self.lines.len() - 1
            }
        };
        let line = &mut self.lines[index];
        line.calls += 1;
        line.tokens += tokens;
        line.usd += usd;
    }

    /// One Jev call (`meter.add("Jev", "api", priceOf("jev", …), tokens)`).
    pub fn add_jev(&mut self, input_tokens: u64) {
        self.add(
            "Jev",
            PaidBy::Api,
            price_of(JEV, input_tokens, 0, 0),
            input_tokens,
        );
    }

    pub fn total(&self) -> f64 {
        self.lines.iter().map(|line| line.usd).sum()
    }

    /// What was billed to an API account.
    pub fn billed(&self) -> f64 {
        self.lines
            .iter()
            .filter(|line| line.paid_by == PaidBy::Api)
            .map(|line| line.usd)
            .sum()
    }

    /// `report()`: nothing without a call.
    pub fn report(&self) -> Vec<String> {
        if self.lines.is_empty() {
            return Vec::new();
        }
        let mut report = vec!["Cost this run:".to_owned()];
        for line in &self.lines {
            let who = match line.paid_by {
                PaidBy::Api => "billed",
                PaidBy::Subscription => "API-equivalent, covered by your subscription",
            };
            report.push(format!(
                "  {:<14} {:>8}  {} call(s), {}k tokens ({who})",
                line.service,
                format_usd(line.usd),
                line.calls,
                (line.tokens as f64 / 1000.0).round()
            ));
        }
        report.push(format!(
            "  {:<14} {:>8}  of which billed: {}",
            "total",
            format_usd(self.total()),
            format_usd(self.billed())
        ));
        report
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
        meter.add(
            "summaries (gpt-6-luna)",
            PaidBy::Subscription,
            price_of(GPT_6_LUNA, 40_000, 10_000, 1_000),
            41_000,
        );
        assert_eq!(
            meter.report(),
            [
                "Cost this run:",
                "  Jev             $0.0002  2 call(s), 4k tokens (billed)",
                "  summaries (gpt-6-luna)  $0.0036  1 call(s), 41k tokens (API-equivalent, covered by your subscription)",
                "  total           $0.0038  of which billed: $0.0002",
            ]
        );
        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(0.125), "$0.13");
    }

    #[test]
    fn cached_input_is_cheaper_and_never_negative() {
        assert!((price_of(GPT_6_LUNA, 1_000_000, 0, 0) - 0.1).abs() < 1e-12);
        assert!((price_of(GPT_6_LUNA, 1_000_000, 1_000_000, 0) - 0.01).abs() < 1e-12);
        // More cached than input (a provider quirk) never goes below zero.
        assert!(price_of(GPT_6_LUNA, 10, 20, 0) >= 0.0);
    }
}
