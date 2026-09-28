import { requireSecret } from "../auth/secrets.ts";
import { priceOf } from "./costs.ts";

type OpenAIResponse = {
	status?: string;
	error?: { message?: string } | null;
	output?: { type: string; content?: { type: string; text?: string }[] }[];
	usage?: { input_tokens?: number; output_tokens?: number; input_tokens_details?: { cached_tokens?: number } };
};

async function postJson(url: string, headers: Record<string, string>, body: unknown): Promise<unknown> {
	const response = await fetch(url, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body) });
	if (!response.ok) throw new Error(`${new URL(url).hostname} returned ${response.status}: ${(await response.text()).slice(0, 300)}`);
	return await response.json();
}

export async function openAIText(instructions: string, input: string, schema?: object): Promise<{ text: string; usd: number; tokens: number }> {
	const response = await postJson("https://api.openai.com/v1/responses", {
		authorization: `Bearer ${requireSecret("OPENAI_API_KEY")}`,
	}, {
		model: "gpt-6-luna", reasoning: { effort: "medium" }, store: false,
		instructions, input,
		...(schema ? { text: { format: { type: "json_schema", name: "chatgpt_cli_result", strict: true, schema } } } : {}),
	}) as OpenAIResponse;
	if (response.status !== "completed") throw new Error(`OpenAI response ${response.status ?? "unknown"}: ${response.error?.message ?? "no completed output"}`);
	const text = (response.output ?? []).flatMap((item) => item.type === "message" ? item.content ?? [] : [])
		.filter((part) => part.type === "output_text").map((part) => part.text ?? "").join("");
	if (!text.trim()) throw new Error("OpenAI returned no text.");
	const inputTokens = response.usage?.input_tokens ?? 0;
	const outputTokens = response.usage?.output_tokens ?? 0;
	return { text, usd: priceOf("gpt-6-luna", { inputTokens,
		cachedInputTokens: response.usage?.input_tokens_details?.cached_tokens ?? 0, outputTokens }),
		tokens: inputTokens + outputTokens };
}

type AnthropicResponse = { content?: { type: string; text?: string }[]; usage?: { input_tokens?: number; output_tokens?: number }; stop_reason?: string };

export async function anthropicText(instructions: string, input: string): Promise<{ text: string; usd: number; tokens: number }> {
	const response = await postJson("https://api.anthropic.com/v1/messages", {
		"x-api-key": requireSecret("ANTHROPIC_API_KEY"), "anthropic-version": "2023-06-01",
	}, { model: "claude-haiku-4-5", max_tokens: 1800, system: instructions,
		messages: [{ role: "user", content: input }] }) as AnthropicResponse;
	if (response.stop_reason === "max_tokens") throw new Error("Anthropic summary was truncated at max_tokens.");
	const text = (response.content ?? []).filter((part) => part.type === "text").map((part) => part.text ?? "").join("");
	if (!text.trim()) throw new Error("Anthropic returned no text.");
	const inputTokens = response.usage?.input_tokens ?? 0;
	const outputTokens = response.usage?.output_tokens ?? 0;
	return { text, usd: (inputTokens + outputTokens * 5) / 1e6, tokens: inputTokens + outputTokens };
}
