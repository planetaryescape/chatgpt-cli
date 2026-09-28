import { expect, test } from "bun:test";
import { anthropicText, openAIText } from "./model-api.ts";

test("configured OpenAI key uses Responses with schema and extracts text", async () => {
	const previousFetch = globalThis.fetch;
	const previousKey = process.env.OPENAI_API_KEY;
	let request: RequestInit | undefined;
	try {
		process.env.OPENAI_API_KEY = "test-openai-key";
		globalThis.fetch = Object.assign(async (_url: string | URL | Request, init?: RequestInit) => {
			request = init;
			return Response.json({ status: "completed", output: [{ type: "message", content: [{ type: "output_text", text: '{"ok":true}' }] }],
				usage: { input_tokens: 100, output_tokens: 10 } });
		}, { preconnect: previousFetch.preconnect });
		const result = await openAIText("Classify", "entry", { type: "object", properties: {} });
		expect(result.text).toBe('{"ok":true}');
		expect((request?.headers as Record<string, string>).authorization).toBe("Bearer test-openai-key");
		const body = JSON.parse(request?.body as string);
		expect(body.store).toBe(false);
		expect(body.text.format.type).toBe("json_schema");
	} finally {
		globalThis.fetch = previousFetch;
		if (previousKey === undefined) delete process.env.OPENAI_API_KEY;
		else process.env.OPENAI_API_KEY = previousKey;
	}
});

test("configured Anthropic key uses Messages and rejects truncated summaries", async () => {
	const previousFetch = globalThis.fetch;
	const previousKey = process.env.ANTHROPIC_API_KEY;
	let request: RequestInit | undefined;
	try {
		process.env.ANTHROPIC_API_KEY = "test-anthropic-key";
		globalThis.fetch = Object.assign(async (_url: string | URL | Request, init?: RequestInit) => {
			request = init;
			return Response.json({ stop_reason: "end_turn", content: [{ type: "text", text: "A summary" }],
				usage: { input_tokens: 100, output_tokens: 10 } });
		}, { preconnect: previousFetch.preconnect });
		expect((await anthropicText("Summarise", "entry")).text).toBe("A summary");
		expect((request?.headers as Record<string, string>)["x-api-key"]).toBe("test-anthropic-key");
		const body = JSON.parse(request?.body as string);
		expect(body.model).toBe("claude-haiku-4-5");
		globalThis.fetch = Object.assign(async () => Response.json({ stop_reason: "max_tokens", content: [{ type: "text", text: "Partial" }] }),
			{ preconnect: previousFetch.preconnect });
		await expect(anthropicText("Summarise", "entry")).rejects.toThrow("truncated");
	} finally {
		globalThis.fetch = previousFetch;
		if (previousKey === undefined) delete process.env.ANTHROPIC_API_KEY;
		else process.env.ANTHROPIC_API_KEY = previousKey;
	}
});
