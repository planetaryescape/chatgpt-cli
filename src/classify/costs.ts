// Published prices, USD per million tokens. Check these when a bill looks off.
// Jev: https://docs.typesafe.ai/models.md (2026-09-27): input only, output free.
// gpt-6-luna: OpenAI standard rate via layer3labs.io and eesel.ai (2026-09-27).
export const PRICES = {
	jev: { input: 0.042, cachedInput: 0.042, output: 0 },
	"gpt-6-luna": { input: 0.1, cachedInput: 0.01, output: 0.5 },
} as const;

export type Usage = { inputTokens: number; cachedInputTokens: number; outputTokens: number };

export function priceOf(model: keyof typeof PRICES, u: Usage): number {
	const p = PRICES[model];
	const uncached = Math.max(0, u.inputTokens - u.cachedInputTokens);
	return (uncached * p.input + u.cachedInputTokens * p.cachedInput + u.outputTokens * p.output) / 1e6;
}

type Line = { calls: number; tokens: number; usd: number; paidBy: "api" | "subscription" };

// Costs for one run, grouped by service. Subscription-covered calls (Codex,
// Claude) are reported at their API-equivalent price so the numbers compare.
export class CostMeter {
	private readonly lines = new Map<string, Line>();

	add(service: string, paidBy: Line["paidBy"], usd: number, tokens: number) {
		const line = this.lines.get(service) ?? { calls: 0, tokens: 0, usd: 0, paidBy };
		line.calls++;
		line.tokens += tokens;
		line.usd += usd;
		this.lines.set(service, line);
	}

	total(): number {
		let sum = 0;
		for (const l of this.lines.values()) sum += l.usd;
		return sum;
	}

	report(): string[] {
		if (this.lines.size === 0) return [];
		const rows = [...this.lines.entries()].map(([service, l]) => {
			const who = l.paidBy === "api" ? "billed" : "API-equivalent, covered by your subscription";
			return `  ${service.padEnd(14)} ${formatUsd(l.usd).padStart(8)}  ${l.calls} call(s), ${Math.round(l.tokens / 1000)}k tokens (${who})`;
		});
		const billed = [...this.lines.values()].filter((l) => l.paidBy === "api").reduce((s, l) => s + l.usd, 0);
		return ["Cost this run:", ...rows, `  ${"total".padEnd(14)} ${formatUsd(this.total()).padStart(8)}  of which billed: ${formatUsd(billed)}`];
	}
}

export function formatUsd(usd: number): string {
	return usd < 0.01 && usd > 0 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(2)}`;
}
